//! High-performance screen capture via DXGI Desktop Duplication.
//!
//! Frames are acquired as D3D11 textures, copied to a staging texture, and
//! memcpy'd into a caller-provided buffer. Zero per-frame allocations after
//! warm-up. The caller owns the buffer lifecycle — which keeps RAM minimal.

use anyhow::{Context, Result};
use std::time::Instant;

use windows::core::Interface;
use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

/// A frame is a borrow of the capture buffer (zero-copy for the consumer).
#[allow(dead_code)]
pub struct FrameView<'a> {
    pub data: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub row_stride: usize,
    pub timestamp: Instant,
    pub frame_num: u64,
}

pub struct CaptureSession {
    _factory: IDXGIFactory1,
    _adapter: IDXGIAdapter1,
    device: ID3D11Device,
    _context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    staging: ID3D11Texture2D,
    width: u32,
    height: u32,
    temp_w: u32,
    temp_h: u32,
    frame_num: u64,
    has_last: bool,
    _offset_x: i32,
    _offset_y: i32,
}

unsafe impl Send for CaptureSession {}

#[derive(Debug, Clone)]
pub struct MonitorInfo {
    pub width: u32,
    pub height: u32,
    pub offset_x: i32,
    pub offset_y: i32,
    pub adapter_index: usize,
    pub output_index: usize,
}

/// Enumerate all desktop monitors across all adapters.
pub fn enumerate_monitors() -> Result<Vec<MonitorInfo>> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }
        .context("CreateDXGIFactory failed")?;
    let mut out = Vec::new();
    let mut ai = 0usize;
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(ai as u32) } {
        let mut oi = 0usize;
        while let Ok(output) = unsafe { adapter.EnumOutputs(oi as u32) } {
            if let Ok(desc) = unsafe { output.GetDesc() } {
                let r = desc.DesktopCoordinates;
                out.push(MonitorInfo {
                    width: (r.right - r.left).max(1) as u32,
                    height: (r.bottom - r.top).max(1) as u32,
                    offset_x: r.left,
                    offset_y: r.top,
                    adapter_index: ai,
                    output_index: oi,
                });
            }
            oi += 1;
        }
        ai += 1;
    }
    if out.is_empty() {
        anyhow::bail!("no monitors found");
    }
    out.sort_by_key(|m| (m.offset_x, m.offset_y));
    Ok(out)
}

impl CaptureSession {
    pub fn new(monitor_index: usize) -> Result<CaptureSession> {
        let monitors = enumerate_monitors()?;
        let mon = monitors
            .get(monitor_index)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("monitor index {} out of range", monitor_index))?;

        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }
            .context("CreateDXGIFactory failed")?;
        let adapter = unsafe { factory.EnumAdapters1(mon.adapter_index as u32) }
            .context("EnumAdapters1 failed")?;
        let output = unsafe { adapter.EnumOutputs(mon.output_index as u32) }
            .context("EnumOutputs failed")?;

        let feature_levels = [D3D_FEATURE_LEVEL_11_0];
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        let hr = unsafe {
            D3D11CreateDevice(
                None::<&IDXGIAdapter>,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE(std::ptr::null_mut()),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        };
        hr.context("D3D11CreateDevice failed")?;
        let device = device.context("d3d11 device missing")?;
        let context = context.context("device context missing")?;

        let output1: IDXGIOutput1 = output.cast().context("IDXGIOutput1 cast failed")?;
        let duplication = unsafe { output1.DuplicateOutput(&device) }
            .context("DuplicateOutput failed — running over RDP or on a protected display?")?;

        let desc = unsafe { output.GetDesc() }?;
        let (w, h) = rect_dims(desc.DesktopCoordinates);

        let td = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging: Option<ID3D11Texture2D> = None;
        unsafe {
            device
                .CreateTexture2D(&td, None, Some(&mut staging))
                .context("CreateTexture2D (staging) failed")?;
        }
        let staging = staging.context("staging texture missing")?;

        Ok(CaptureSession {
            _factory: factory,
            _adapter: adapter,
            device,
            _context: context,
            duplication,
            staging,
            width: w,
            height: h,
            temp_w: w,
            temp_h: h,
            frame_num: 0,
            has_last: false,
            _offset_x: desc.DesktopCoordinates.left,
            _offset_y: desc.DesktopCoordinates.top,
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

    /// Acquire the next frame into `buffer` (sized to `frame_size_bytes`).
    /// Returns Ok(true) on a fresh frame — caller must then call `release()`.
    pub fn acquire(&mut self, buffer: &mut [u8], timeout_ms: u32) -> Result<bool> {
        let mut fi = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        // Auto-coerced &mut -> *mut like the Win32 convention.
        if unsafe { self.duplication.AcquireNextFrame(timeout_ms, &mut fi, &mut resource) }
            .is_err()
        {
            self.has_last = false;
            return Ok(false);
        }
        let _ = fi.LastPresentTime;
        self.has_last = true;
        let res = resource.context("Acquired frame with null resource")?;

        let texture: ID3D11Texture2D = res
            .cast()
            .context("IDXGIResource -> ID3D11Texture2D cast failed")?;
        let mut tex_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut tex_desc) };
        self.width = tex_desc.Width;
        self.height = tex_desc.Height;

        // Ensure staging texture matches the frame size (recreate on change).
        if self.staging_size_mismatch(tex_desc.Width, tex_desc.Height) {
            let td = D3D11_TEXTURE2D_DESC {
                Width: tex_desc.Width,
                Height: tex_desc.Height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut new_staging: Option<ID3D11Texture2D> = None;
            unsafe {
                self.device
                    .CreateTexture2D(&td, None, Some(&mut new_staging))
                    .context("CreateTexture2D (staging resize) failed")?;
            }
            self.staging = new_staging.context("staging texture missing")?;
            self.temp_w = tex_desc.Width;
            self.temp_h = tex_desc.Height;
        }

        unsafe {
            self._context_unwrap().CopyResource(&self.staging, &texture);
        }

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self._context_unwrap()
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .context("Map (staging) failed")?;
        }

        let row_stride = mapped.RowPitch as usize;
        let width_bytes = self.width as usize * 4;
        let total = width_bytes * self.height as usize;
        if buffer.len() < total {
            unsafe { self._context_unwrap().Unmap(&self.staging, 0) };
            anyhow::bail!("capture buffer too small ({} < {})", buffer.len(), total);
        }

        if row_stride == width_bytes {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    mapped.pData.cast::<u8>(),
                    buffer.as_mut_ptr(),
                    total,
                );
            }
        } else {
            let mut src = mapped.pData.cast::<u8>();
            let mut dst = buffer.as_mut_ptr();
            for _ in 0..self.height {
                unsafe {
                    std::ptr::copy_nonoverlapping(src, dst, width_bytes);
                    src = src.add(row_stride);
                    dst = dst.add(width_bytes);
                }
            }
        }
        unsafe {
            self._context_unwrap().Unmap(&self.staging, 0);
        }

        self.frame_num += 1;
        Ok(true)
    }

    /// True while an acquired frame is pending release.
    #[allow(dead_code)]
    pub fn has_acquired(&self) -> bool {
        self.has_last
    }

    /// Release the duplicated frame. Must follow a successful `acquire`.
    pub fn release(&mut self) -> Result<()> {
        self.has_last = false;
        unsafe { self.duplication.ReleaseFrame().context("ReleaseFrame failed") }
    }

    pub fn frame_view<'a>(&self, buffer: &'a [u8]) -> FrameView<'a> {
        let total = self.frame_size_bytes().min(buffer.len());
        FrameView {
            data: &buffer[..total],
            width: self.width,
            height: self.height,
            row_stride: self.width as usize * 4,
            timestamp: Instant::now(),
            frame_num: self.frame_num,
        }
    }

    fn staging_size_mismatch(&self, w: u32, h: u32) -> bool {
        self.temp_w != w || self.temp_h != h
    }

    fn _context_unwrap(&self) -> &ID3D11DeviceContext {
        &self._context
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        if self.has_last {
            let _ = unsafe { self.duplication.ReleaseFrame() };
        }
    }
}

fn rect_dims(r: RECT) -> (u32, u32) {
    (
        (r.right - r.left).max(1) as u32,
        (r.bottom - r.top).max(1) as u32,
    )
}

// Silence unused import lint for Windows crate features when some are pulled
// in under certain feature combinations.
#[allow(dead_code)]
fn _keep(_d: DXGI_FORMAT) {
    let _ = DXGI_FORMAT_B8G8R8A8_UNORM;
}
