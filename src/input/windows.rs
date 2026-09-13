//! Windows input capture: a polling thread (GetAsyncKeyState / GetCursorPos /
//! XInput) plus a GDI-based on-screen overlay compositor.

use super::{InputState, MouseButton, OverlayFrame, RING_LIFETIME};
use crate::config::OverlayPosition;
use anyhow::Context;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HANDLE, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

// ---------------------------------------------------------------------------
// Input polling thread
// ---------------------------------------------------------------------------

/// Virtual-key table we watch (name = display text for overlay/sidecar).
const KEYS: &[(u16, &str)] = &[
    (0x30, "0"), (0x31, "1"), (0x32, "2"), (0x33, "3"), (0x34, "4"),
    (0x35, "5"), (0x36, "6"), (0x37, "7"), (0x38, "8"), (0x39, "9"),
    (0x41, "A"), (0x42, "B"), (0x43, "C"), (0x44, "D"), (0x45, "E"),
    (0x46, "F"), (0x47, "G"), (0x48, "H"), (0x49, "I"), (0x4A, "J"),
    (0x4B, "K"), (0x4C, "L"), (0x4D, "M"), (0x4E, "N"), (0x4F, "O"),
    (0x50, "P"), (0x51, "Q"), (0x52, "R"), (0x53, "S"), (0x54, "T"),
    (0x55, "U"), (0x56, "V"), (0x57, "W"), (0x58, "X"), (0x59, "Y"),
    (0x5A, "Z"),
    (0x70, "F1"), (0x71, "F2"), (0x72, "F3"), (0x73, "F4"), (0x74, "F5"),
    (0x75, "F6"), (0x76, "F7"), (0x77, "F8"), (0x78, "F9"), (0x79, "F10"),
    (0x7A, "F11"), (0x7B, "F12"),
    (0x20, "SPACE"), (0x0D, "ENTER"), (0x09, "TAB"), (0x1B, "ESC"),
    (0x08, "BACKSPACE"), (0x25, "LEFT"), (0x26, "UP"), (0x27, "RIGHT"),
    (0x28, "DOWN"), (0x10, "SHIFT"), (0x11, "CTRL"), (0x12, "ALT"),
    (0x5B, "WIN"),
];

const M_BUTTONS: &[(i32, MouseButton)] = &[
    (0x01, MouseButton::Left),
    (0x02, MouseButton::Right),
    (0x04, MouseButton::Middle),
    (0x05, MouseButton::X1),
    (0x06, MouseButton::X2),
];

/// Poll mouse/keyboard/gamepad and stream events into `state`.
pub fn poll_loop(
    inner: &Arc<Mutex<InputState>>,
    stop: &Arc<AtomicBool>,
    monitor_offset: (i32, i32),
    monitor_size: (u32, u32),
) {
    let mut prev_keys = vec![false; KEYS.len()];
    let mut prev_buttons = [false; M_BUTTONS.len()];
    let mut last_pos = (i32::MIN, i32::MIN);
    let mut xinput = XInputProbe::new();

    let interval = Duration::from_millis(20);
    while !stop.load(Ordering::Relaxed) {
        let tick = Instant::now();
        if let Ok(mut s) = inner.lock() {
            // Keys (edge detection).
            for (i, (vk, name)) in KEYS.iter().enumerate() {
                let down = is_async_down(*vk);
                if down != prev_keys[i] {
                    s.record_key(name, down);
                    prev_keys[i] = down;
                }
            }

            // Mouse position → monitor-relative coords.
            if let Some((rx, ry)) = cursor_pos().map(|p| to_monitor(p.0, p.1, monitor_offset, monitor_size)) {
                if (rx - last_pos.0).abs() + (ry - last_pos.1).abs() >= 2 {
                    last_pos = (rx, ry);
                    s.record_move(rx, ry);
                }
            }

            // Mouse buttons.
            for (i, (vk, btn)) in M_BUTTONS.iter().enumerate() {
                let down = is_async_down(*vk as u16);
                if down != prev_buttons[i] {
                    let pos = cursor_pos()
                        .map(|p| to_monitor(p.0, p.1, monitor_offset, monitor_size))
                        .unwrap_or(last_pos);
                    s.record_click(*btn, pos.0, pos.1, down);
                    prev_buttons[i] = down;
                }
            }

            // Gamepad (XInput).
            xinput.poll(&mut s);
        }
        let elapsed = tick.elapsed();
        if elapsed < interval {
            std::thread::sleep(interval - elapsed);
        }
    }
    log::debug!("input poller stopped");
}

fn is_async_down(vk: u16) -> bool {
    let state = unsafe { GetAsyncKeyState(i32::from(vk)) };
    (state as u16) & 0x8000 != 0
}

fn cursor_pos() -> Option<(i32, i32)> {
    let mut pt = POINT::default();
    unsafe { GetCursorPos(&mut pt) }
        .ok()
        .map(|_| (pt.x, pt.y))
}

/// Translate a desktop cursor position to monitor-relative capture coords.
fn to_monitor(x: i32, y: i32, off: (i32, i32), size: (u32, u32)) -> (i32, i32) {
    let rx = x - off.0;
    let ry = y - off.1;
    (
        rx.clamp(0, size.0.saturating_sub(1) as i32),
        ry.clamp(0, size.1.saturating_sub(1) as i32),
    )
}

// ---------------------------------------------------------------------------
// XInput (gamepad) probe — dynamically loaded, never a hard dependency.
// ---------------------------------------------------------------------------

const BTN_A: u16 = 0x1000;
const BTN_B: u16 = 0x2000;
const BTN_X: u16 = 0x4000;
const BTN_Y: u16 = 0x8000;
const BTN_DPAD_UP: u16 = 0x0001;
const BTN_DPAD_DOWN: u16 = 0x0002;
const BTN_DPAD_LEFT: u16 = 0x0004;
const BTN_DPAD_RIGHT: u16 = 0x0008;
const BTN_START: u16 = 0x0010;
const BTN_BACK: u16 = 0x0020;
const BTN_LSTICK: u16 = 0x0040;
const BTN_RSTICK: u16 = 0x0080;
const BTN_LB: u16 = 0x0100;
const BTN_RB: u16 = 0x0200;

const XINPUT_BUTTONS: &[(u16, &str)] = &[
    (BTN_A, "A"), (BTN_B, "B"), (BTN_X, "X"), (BTN_Y, "Y"),
    (BTN_DPAD_UP, "D-PAD UP"), (BTN_DPAD_DOWN, "D-PAD DOWN"),
    (BTN_DPAD_LEFT, "D-PAD LEFT"), (BTN_DPAD_RIGHT, "D-PAD RIGHT"),
    (BTN_START, "START"), (BTN_BACK, "BACK"),
    (BTN_LSTICK, "L-STICK"), (BTN_RSTICK, "R-STICK"),
    (BTN_LB, "LB"), (BTN_RB, "RB"),
];

#[repr(C)]
struct XInputGamepadDyn {
    w_buttons: u16,
    _pad0: u8,
    _pad1: u8,
    s_thumb_lx: i16,
    s_thumb_ly: i16,
    s_thumb_rx: i16,
    s_thumb_ry: i16,
}

#[repr(C)]
struct XInputStateDyn {
    dw_packet_number: u32,
    gamepad: XInputGamepadDyn,
}

type XGetStateFn = unsafe extern "system" fn(u32, *mut XInputStateDyn) -> u32;

struct XInputProbe {
    get_state: Option<XGetStateFn>,
    prev: [u16; 4],
}

impl XInputProbe {
    fn new() -> Self {
        use windows::core::{s, PCWSTR};
        use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

        let mut probe = XInputProbe {
            get_state: None,
            prev: [0; 4],
        };
        const LIBS: [&str; 3] = ["xinput1_4.dll", "xinput1_3.dll", "xinput9_1_0.dll"];
        for name in LIBS {
            let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let Ok(lib) = (unsafe { LoadLibraryW(PCWSTR(wide.as_ptr())) }) else {
                continue;
            };
            let Some(far) = (unsafe { GetProcAddress(lib, s!("XInputGetState")) }) else {
                continue;
            };
            probe.get_state = Some(unsafe {
                std::mem::transmute::<unsafe extern "system" fn() -> isize, XGetStateFn>(far)
            });
            break;
        }
        probe
    }

    fn poll(&mut self, s: &mut InputState) {
        let Some(f) = self.get_state else { return };
        for idx in 0..4u32 {
            let mut st = XInputStateDyn {
                dw_packet_number: 0,
                gamepad: XInputGamepadDyn {
                    w_buttons: 0,
                    _pad0: 0,
                    _pad1: 0,
                    s_thumb_lx: 0,
                    s_thumb_ly: 0,
                    s_thumb_rx: 0,
                    s_thumb_ry: 0,
                },
            };
            if unsafe { f(idx, &mut st) } != 0 {
                self.prev[idx as usize] = 0;
                continue;
            }
            let now = st.gamepad.w_buttons;
            let old = self.prev[idx as usize];
            for (bit, name) in XINPUT_BUTTONS {
                let was = old & *bit != 0;
                let is = now & *bit != 0;
                if was != is {
                    s.record_key(name, is);
                }
            }
            self.prev[idx as usize] = now;
        }
    }
}

// ---------------------------------------------------------------------------
// GDI overlay renderer
// ---------------------------------------------------------------------------

pub struct GdiOverlay {
    dc: HDC,
    bmp: HBITMAP,
    old_bmp: HGDIOBJ,
    bits: *mut u8,
    w: u32,
    h: u32,
    font: HFONT,
    brush_clear: HBRUSH,
    brush_box: HBRUSH,
    brush_border: HBRUSH,
}

unsafe impl Send for GdiOverlay {}

const BOX_H: i32 = 32;
const BOX_GAP: i32 = 6;
const MARGIN: i32 = 12;
const MAX_BOXES: usize = 6;

/// BGR packed colors (0xBBGGRR).
const BOX_BG: u32 = 0x10_12_24;
const BOX_BORDER: u32 = 0x5B_8C_FF;

impl GdiOverlay {
    /// Create the DIB section + GDI objects for a given frame size.
    pub fn new(w: u32, h: u32) -> anyhow::Result<GdiOverlay> {
        use windows::core::w;

        let screen = unsafe { GetDC(None) };
        let dc = unsafe { CreateCompatibleDC(screen) };
        let _ = unsafe { ReleaseDC(None, screen) };

        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w as i32;
        bmi.bmiHeader.biHeight = -(h as i32);
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB.0;

        let mut bits_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = unsafe {
            CreateDIBSection(
                dc,
                &bmi,
                DIB_RGB_COLORS,
                &mut bits_ptr,
                HANDLE(std::ptr::null_mut()),
                0,
            )
        }
        .context("CreateDIBSection")?;
        anyhow::ensure!(!bits_ptr.is_null(), "CreateDIBSection returned null bits");

        let old_bmp = unsafe { SelectObject(dc, bmp) };

        let font = unsafe {
            CreateFontW(
                -15,
                0,
                0,
                0,
                FW_BOLD.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET.0 as u32,
                OUT_DEFAULT_PRECIS.0 as u32,
                CLIP_DEFAULT_PRECIS.0 as u32,
                PROOF_QUALITY.0 as u32,
                FF_DONTCARE.0 as u32,
                w!("Segoe UI"),
            )
        };
        let _ = unsafe { SelectObject(dc, font) };

        let _ = unsafe { SetTextColor(dc, COLORREF(0xF0_EB_E8)) };
        let _ = unsafe { SetBkMode(dc, OPAQUE) };
        let _ = unsafe { SetBkColor(dc, COLORREF(BOX_BG)) };

        let brush_clear = unsafe { CreateSolidBrush(COLORREF(0)) };
        let brush_box = unsafe { CreateSolidBrush(COLORREF(BOX_BG)) };
        let brush_border = unsafe { CreateSolidBrush(COLORREF(BOX_BORDER)) };

        Ok(GdiOverlay {
            dc,
            bmp,
            old_bmp,
            bits: bits_ptr.cast::<u8>(),
            w,
            h,
            font,
            brush_clear,
            brush_box,
            brush_border,
        })
    }

    pub fn width(&self) -> u32 {
        self.w
    }

    pub fn height(&self) -> u32 {
        self.h
    }

    /// Draw the current snapshot into the DIB, then composite whatever was
    /// painted (non-black) onto `frame` (BGRA8).
    pub fn compose(
        &mut self,
        frame: &mut [u8],
        w: u32,
        h: u32,
        position: OverlayPosition,
        snap: &OverlayFrame,
    ) -> anyhow::Result<()> {
        if w != self.w || h != self.h {
            anyhow::bail!("overlay size mismatch");
        }

        let full = RECT { left: 0, top: 0, right: w as i32, bottom: h as i32 };
        let _ = unsafe { FillRect(self.dc, &full, self.brush_clear) };

        let mut dirty = RECT {
            left: i32::MAX,
            top: i32::MAX,
            right: i32::MIN,
            bottom: i32::MIN,
        };

        // Key boxes (right-corner rows run right → left).
        if !snap.keys.is_empty() {
            let widths: Vec<i32> = snap
                .keys
                .iter()
                .take(MAX_BOXES)
                .map(|name| self.text_width(name).max(14) + 16)
                .collect();
            let total: i32 = widths.iter().copied().sum::<i32>()
                + BOX_GAP * (widths.len() as i32 - 1).max(0);
            let (start_x, top) = self.anchor(position, total);
            let leftward = matches!(
                position,
                OverlayPosition::TopRight | OverlayPosition::BottomRight
            );
            let mut x = start_x;
            for (i, name) in snap.keys.iter().take(MAX_BOXES).enumerate() {
                let bw = widths[i];
                let bx = if leftward { x - bw } else { x };
                let r = RECT {
                    left: bx,
                    top,
                    right: bx + bw,
                    bottom: top + BOX_H,
                };
                let _ = unsafe { FillRect(self.dc, &r, self.brush_box) };
                let _ = unsafe { FrameRect(self.dc, &r, self.brush_border) };
                let wide: Vec<u16> = name.encode_utf16().collect();
                let _ = unsafe {
                    TextOutW(self.dc, bx + 8, top + 9, &wide)
                };
                if leftward {
                    x = bx - BOX_GAP;
                } else {
                    x = bx + bw + BOX_GAP;
                }
                union(&mut dirty, r);
            }
        }

        // Fading click rings.
        for ring in &snap.clicks {
            let age = ring.born.elapsed().as_secs_f32().clamp(0.0, RING_LIFETIME);
            let radius = (13.0 + age * 30.0) as i32;
            let pen = unsafe { CreatePen(PS_SOLID, 3, COLORREF(button_color(ring.button))) };
            let old_pen = unsafe { SelectObject(self.dc, pen) };
            let x0 = ring.x.saturating_sub(radius);
            let y0 = ring.y.saturating_sub(radius);
            let x1 = ring.x.saturating_add(radius.max(1));
            let y1 = ring.y.saturating_add(radius.max(1));
            let _ = unsafe { Ellipse(self.dc, x0, y0, x1, y1) };
            let _ = unsafe { SelectObject(self.dc, old_pen) };
            let _ = unsafe { DeleteObject(pen) };
            union(&mut dirty, RECT { left: x0, top: y0, right: x1, bottom: y1 });
        }

        if dirty.left <= dirty.right {
            self.blend(frame, w, h, dirty);
        }
        Ok(())
    }

    /// Starting x (and top y) for the row of key boxes.
    fn anchor(&self, position: OverlayPosition, total_width: i32) -> (i32, i32) {
        match position {
            OverlayPosition::TopLeft => (MARGIN, MARGIN),
            OverlayPosition::TopRight => (self.w as i32 - MARGIN - total_width, MARGIN),
            OverlayPosition::BottomLeft => (MARGIN, self.h as i32 - MARGIN - BOX_H),
            OverlayPosition::BottomRight => (
                self.w as i32 - MARGIN - total_width,
                self.h as i32 - MARGIN - BOX_H,
            ),
        }
    }

    fn text_width(&self, name: &str) -> i32 {
        let wide: Vec<u16> = name.encode_utf16().collect();
        let mut sz = SIZE::default();
        let _ = unsafe { GetTextExtentPoint32W(self.dc, &wide, &mut sz) };
        sz.cx + 6
    }

    fn blend(&self, frame: &mut [u8], w: u32, h: u32, dirty: RECT) {
        let wu = w as usize;
        let left = dirty.left.max(0) as usize;
        let top = dirty.top.max(0) as usize;
        let right = (dirty.right as usize).min(wu);
        let bottom = (dirty.bottom as usize).min(h as usize);
        if left >= right || top >= bottom {
            return;
        }
        for row in top..bottom {
            let row_base = row * wu;
            let dst = &mut frame[(row_base + left) * 4..(row_base + right) * 4];
            for col in left..right {
                let idx = row_base + col;
                let p = unsafe { self.bits.add(idx * 4) };
                let b = unsafe { *p };
                let g = unsafe { *p.add(1) };
                let r = unsafe { *p.add(2) };
                if r | g | b != 0 {
                    let d = (col - left) * 4;
                    dst[d] = b;
                    dst[d + 1] = g;
                    dst[d + 2] = r;
                    dst[d + 3] = 0xFF;
                }
            }
        }
    }
}

impl Drop for GdiOverlay {
    fn drop(&mut self) {
        let _ = unsafe { SelectObject(self.dc, self.old_bmp) };
        let _ = unsafe { DeleteObject(self.bmp) };
        let _ = unsafe { DeleteObject(self.font) };
        let _ = unsafe { DeleteObject(self.brush_clear) };
        let _ = unsafe { DeleteObject(self.brush_box) };
        let _ = unsafe { DeleteObject(self.brush_border) };
        let _ = unsafe { DeleteDC(self.dc) };
    }
}

fn union(dirty: &mut RECT, r: RECT) {
    dirty.left = dirty.left.min(r.left);
    dirty.top = dirty.top.min(r.top);
    dirty.right = dirty.right.max(r.right);
    dirty.bottom = dirty.bottom.max(r.bottom);
}

/// BGR packed color per button: 0xBBGGRR.
fn button_color(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => 0xFF_8C_5B,
        MouseButton::Right => 0x45_7A_FF,
        MouseButton::Middle => 0x4F_D2_FF,
        MouseButton::X1 => 0xA9_A9_3F,
        MouseButton::X2 => 0xFF_7A_C0,
    }
}