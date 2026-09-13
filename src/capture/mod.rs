//! Cross-platform screen capture.
//!
//! **Windows**: DXGI Desktop Duplication — frames arrive as D3D11 textures,
//! staged and memcpy'd into a caller-owned buffer. Zero per-frame allocations.
//!
//! **Linux (X11 / XWayland)**: MIT-SHM accelerated XGetImage via `x11rb`.
//! Native Wayland desktops run their XWayland server, so capture works through
//! it; a pure-Wayland session without XWayland is detected and reported clearly.

use std::time::Instant;

#[cfg(windows)]
#[path = "windows.rs"]
mod imp;

#[cfg(not(windows))]
#[path = "linux.rs"]
mod imp;

pub use imp::{enumerate_monitors, CaptureSession};

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

#[derive(Debug, Clone)]
pub struct MonitorInfo {
    pub width: u32,
    pub height: u32,
    pub offset_x: i32,
    pub offset_y: i32,
    #[allow(dead_code)]
    pub adapter_index: usize,
    #[allow(dead_code)]
    pub output_index: usize,
}