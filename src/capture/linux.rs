//! Linux capture backend: X11 via x11rb (pure Rust, no C deps).
//!
//! Frames are pulled with XGetImage on the root window for the selected
//! monitor's region and converted to flat BGRA8 (matching the Windows B8G8R8A8
//! path) into a caller-owned buffer.
//!
//! Works on X11 desktop sessions and under Wayland compositors that run
//! XWayland. A pure-Wayland session without an X server yields a clear error.

use super::MonitorInfo;
use anyhow::{bail, Context, Result};
use std::sync::atomic::{AtomicU64, Ordering};

use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{ConnectionExt, ImageFormat};
use x11rb::rust_connection::RustConnection;

static FRAME_CTR: AtomicU64 = AtomicU64::new(0);

/// X11 client capturing one monitor's region of the root window.
pub struct CaptureSession {
    conn: RustConnection,
    root: x11rb::protocol::xproto::Window,
    width: u32,
    height: u32,
    offset_x: i32,
    offset_y: i32,
    // Bytes-per-pixel of the root visual (3 for 24-bit, 4 for 32-bit).
    bpp: u32,
    frame_num: u64,
}

/// Enumerate monitors via XRandR CRTCs when available.
pub fn enumerate_monitors() -> Result<Vec<MonitorInfo>> {
    let (conn, screen_num) = x11rb::connect(None)
        .context("cannot connect to X server (is DISPLAY set? are you in a Wayland session without XWayland?)")?;
    let screen = &conn.setup().roots[screen_num];

    // Prefer real RandR geometry (1.5+ `GetMonitors`).
    let rr_ok = conn
        .randr_query_version(1, 5)
        .ok()
        .and_then(|r| r.reply().ok())
        .map(|rr| (rr.major_version, rr.minor_version) >= (1, 5))
        .unwrap_or(false);
    if rr_ok {
        if let Some(reply) = conn
            .randr_get_monitors(screen.root, true)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        {
            let mut out: Vec<MonitorInfo> = Vec::new();
            for m in &reply.monitors {
                let w = m.width.max(1) as i32;
                let h = m.height.max(1) as i32;
                out.push(MonitorInfo {
                    width: w as u32,
                    height: h as u32,
                    offset_x: m.x as i32,
                    offset_y: m.y as i32,
                    adapter_index: 0,
                    output_index: out.len(),
                });
            }
            if !out.is_empty() {
                out.sort_by_key(|m| (m.offset_x, m.offset_y));
                return Ok(out);
            }
        }
    }

    // Fallback: one monitor spanning the whole screen.
    Ok(vec![MonitorInfo {
        width: screen.width_in_pixels as u32,
        height: screen.height_in_pixels as u32,
        offset_x: 0,
        offset_y: 0,
        adapter_index: 0,
        output_index: 0,
    }])
}

impl CaptureSession {
    pub fn new(monitor_index: usize) -> Result<CaptureSession> {
        let monitors = enumerate_monitors()?;
        let mon = monitors
            .get(monitor_index)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("monitor index {} out of range", monitor_index))?;

        let (conn, screen_num) = x11rb::connect(None)
            .context("cannot connect to X server (is DISPLAY set? are you in a Wayland session without XWayland?)")?;
        let (root, depth) = {
            let screen = &conn.setup().roots[screen_num];
            (screen.root, screen.root_depth)
        };

        // Root visual depth ⇒ bytes per pixel for GetImage (24-bit visuals
        // are almost always padded to 32 bits on modern servers).
        if depth < 15 {
            bail!("unsupported root visual depth {depth} (need 24 or 32-bit)");
        }
        let bpp = if depth == 24 { 4 } else { 4 };

        Ok(CaptureSession {
            conn,
            root,
            width: mon.width,
            height: mon.height,
            offset_x: mon.offset_x,
            offset_y: mon.offset_y,
            bpp,
            frame_num: 0,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn frame_size_bytes(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }

    /// Pull the monitor region with XGetImage and convert to BGRA8.
    pub fn acquire(&mut self, buffer: &mut [u8], _timeout_ms: u32) -> Result<bool> {
        let need = self.frame_size_bytes();
        if buffer.len() < need {
            bail!("capture buffer too small ({} < {})", buffer.len(), need);
        }

        let w = self.width as i16;
        let h = self.height as i16;
        let x = self.offset_x as i16;
        let y = self.offset_y as i16;

        // Sub-32-bit visuals: x11rb pads each pixel to bpp bytes already, so a
        // 24-bit ZPixmap pull arrives as 4 bytes/pixel (BGRX). Copy as-is.
        let reply = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, self.root, x, y, w as u16, h as u16, u32::MAX)
            .context("XGetImage failed")?
            .reply()
            .context("XGetImage reply failed")?;

        let src = &reply.data;
        let row_bytes_src = self.width as usize * self.bpp as usize;
        let row_bytes_dst = self.width as usize * 4;

        if row_bytes_src == row_bytes_dst && src.len() >= need {
            buffer[..need].copy_from_slice(&src[..need]);
        } else {
            // Expand 24→32 (BGR→BGRA) row by row; guard short replies.
            for row in 0..self.height as usize {
                let s = src
                    .get(row * row_bytes_src..(row + 1) * row_bytes_src)
                    .unwrap_or_default();
                let d = &mut buffer[row * row_bytes_dst..(row + 1) * row_bytes_dst];
                match self.bpp {
                    3 => {
                        for (i, px) in s.chunks_exact(3).enumerate() {
                            d[i * 4..i * 4 + 3].copy_from_slice(px);
                            d[i * 4 + 3] = 0xFF;
                        }
                    }
                    4 => {
                        let n = s.len().min(d.len());
                        d[..n].copy_from_slice(&s[..n]);
                        // Flatten any missing alpha to opaque.
                        if n < d.len() {
                            d[n..].fill(0xFF);
                        }
                    }
                    _ => bail!("unsupported bpp {}", self.bpp),
                }
            }
        }

        self.frame_num = FRAME_CTR.fetch_add(1, Ordering::Relaxed);
        Ok(true)
    }

    /// No-op for X11 (nothing to release).
    pub fn release(&mut self) -> Result<()> {
        Ok(())
    }

    #[allow(dead_code)]
    pub fn has_acquired(&self) -> bool {
        true
    }

    pub fn frame_view<'a>(&self, buffer: &'a [u8]) -> super::FrameView<'a> {
        let total = self.frame_size_bytes().min(buffer.len());
        super::FrameView {
            data: &buffer[..total],
            width: self.width,
            height: self.height,
            row_stride: self.width as usize * 4,
            timestamp: std::time::Instant::now(),
            frame_num: self.frame_num,
        }
    }
}