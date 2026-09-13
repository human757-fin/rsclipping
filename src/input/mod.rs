//! Input capture for clips: a background poller that records mouse,
//! keyboard and controller (gamepad) events with millisecond timestamps,
//! plus an optional on-screen "keys pressed / mouse clicked" overlay.
//!
//! Recorded events are exported as a `*.inputs.json` sidecar next to each
//! saved clip/recording. The overlay is composited into the video frames
//! before encoding (full key boxes + fading click rings).

use crate::config::{OverlayPosition, InputConfig};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[cfg(windows)]
pub mod windows;
#[cfg(not(windows))]
pub mod linux;

/// Cap the rolling event log to this many seconds of history. Clips can never
/// reach further back than the segment window, so a few minutes is generous.
const EVENT_HISTORY_SECS: f64 = 360.0;
/// Max number of click rings kept for the overlay at once.
#[allow(dead_code)] // only used by the Windows poller
const MAX_RINGS: usize = 16;
/// How long a click ring remains visible on screen (seconds).
const RING_LIFETIME: f32 = 0.7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

/// A single captured input event. Timestamps are stored separately in the
/// event queue (f64 milliseconds since capture start).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputEvent {
    KeyDown { name: String },
    KeyUp { name: String },
    MouseDown { button: MouseButton, x: i32, y: i32 },
    MouseUp { button: MouseButton, x: i32, y: i32 },
    Move { x: i32, y: i32 },
    Gamepad { name: String, pressed: bool },
}

struct EventRec {
    t_ms: f64,
    event: InputEvent,
}

/// One click ring for the overlay (screen coords relative to the capture
/// region, plus when it was born so the renderer can grow/fade it).
#[derive(Clone)]
pub struct RingVis {
    pub x: i32,
    pub y: i32,
    pub button: MouseButton,
    pub born: Instant,
}

/// Snapshot of what the overlay should draw this frame.
#[derive(Default, Clone)]
pub struct OverlayFrame {
    pub keys: Vec<String>,
    pub clicks: Vec<RingVis>,
}

impl OverlayFrame {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.clicks.is_empty()
    }
}

/// Shared, lock-guarded input state. Written by the polling thread, read by
/// the capture loop (overlay snapshot) and the output worker (sidecar).
#[allow(dead_code)] // methods/fields are exercised by the Windows-only poller
#[derive(Default)]
pub struct InputState {
    start: Option<Instant>,
    events: VecDeque<EventRec>,
    pressed: Vec<String>,
    rings: VecDeque<RingVis>,
    last_move: (i32, i32),
}

#[allow(dead_code)] // exercised by the Windows-only poller
impl InputState {
    fn now_ms(&mut self) -> f64 {
        let start = *self.start.get_or_insert_with(Instant::now);
        start.elapsed().as_secs_f64() * 1000.0
    }

    fn push(&mut self, event: InputEvent) {
        let t_ms = self.now_ms();
        self.events.push_back(EventRec { t_ms, event });
        // Drop events older than the rolling cap.
        let cutoff = t_ms - EVENT_HISTORY_SECS * 1000.0;
        while let Some(front) = self.events.front() {
            if front.t_ms < cutoff {
                self.events.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn record_key(&mut self, name: &str, down: bool) {
        if down {
            if !self.pressed.iter().any(|k| k == name) {
                self.pressed.push(name.to_string());
            }
            self.push(InputEvent::KeyDown { name: name.to_string() });
        } else {
            self.pressed.retain(|k| k != name);
            self.push(InputEvent::KeyUp { name: name.to_string() });
        }
    }

    pub fn record_click(&mut self, button: MouseButton, x: i32, y: i32, down: bool) {
        if down {
            self.rings.push_back(RingVis {
                x,
                y,
                button,
                born: Instant::now(),
            });
            if self.rings.len() > MAX_RINGS {
                self.rings.pop_front();
            }
            self.push(InputEvent::MouseDown { button, x, y });
        } else {
            self.push(InputEvent::MouseUp { button, x, y });
        }
    }

    pub fn record_move(&mut self, x: i32, y: i32) {
        if (x - self.last_move.0).abs() + (y - self.last_move.1).abs() >= 2 {
            self.last_move = (x, y);
            self.push(InputEvent::Move { x, y });
        }
    }

    /// Keys currently held, ordered for display (modifiers first).
    pub fn pressed_keys(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for k in &self.pressed {
            if is_modifier(k) {
                out.insert(0, k.clone());
            } else {
                out.push(k.clone());
            }
        }
        out
    }

    pub fn overlay_snapshot(&self) -> OverlayFrame {
        let now = Instant::now();
        let clicks = self
            .rings
            .iter()
            .map(|r| RingVis {
                x: r.x,
                y: r.y,
                button: r.button,
                born: r.born,
            })
            .filter(|r| now.duration_since(r.born).as_secs_f32() < RING_LIFETIME)
            .collect();
        OverlayFrame {
            keys: self.pressed_keys(),
            clicks,
        }
    }
}

fn is_modifier(k: &str) -> bool {
    matches!(k, "CTRL" | "SHIFT" | "ALT" | "WIN")
}

/// A runnable input capture session (thread + shared state + overlay policy).
pub struct InputCapture {
    inner: Arc<Mutex<InputState>>,
    stop: Arc<AtomicBool>,
    position: OverlayPosition,
    overlay_enabled: bool,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl InputCapture {
    /// Start the polling thread. Returns an `Arc` shared with the daemon loop
    /// and the output worker.
    #[cfg_attr(not(windows), allow(unused_variables))]
    pub fn start(cfg: &InputConfig, monitor_offset: (i32, i32), monitor_size: (u32, u32)) -> Arc<InputCapture> {
        let inner = Arc::new(Mutex::new(InputState::default()));
        let stop = Arc::new(AtomicBool::new(false));

        let (worker, run) = if cfg.capture_events {
            let i2 = inner.clone();
            let s2 = stop.clone();
            let handle = std::thread::Builder::new()
                .name("input-capture".to_string())
                .spawn(move || {
                    #[cfg(windows)]
                    windows::poll_loop(&i2, &s2, monitor_offset, monitor_size);
                    #[cfg(not(windows))]
                    {
                        log::info!("input capture is only available on Windows; events will be disabled");
                        std::thread::sleep(std::time::Duration::from_secs(1));
                        let _ = (i2, s2);
                    }
                })
                .ok();
            (handle, cfg.capture_events)
        } else {
            (None, false)
        };

        Arc::new(InputCapture {
            inner,
            stop,
            position: cfg.overlay_position,
            overlay_enabled: cfg.overlay_enabled && run,
            worker,
        })
    }

    pub fn overlay_enabled(&self) -> bool {
        self.overlay_enabled
    }

    pub fn position(&self) -> OverlayPosition {
        self.position
    }

    pub fn overlay_snapshot(&self) -> OverlayFrame {
        self.inner
            .lock()
            .map(|s| s.overlay_snapshot())
            .unwrap_or_default()
    }

    /// Shared state for the output worker (locked briefly to export a window).
    pub fn state_arc(&self) -> Arc<Mutex<InputState>> {
        self.inner.clone()
    }
}

impl Drop for InputCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

/// Events whose timestamps (ms since session start) fall inside [start_ms, end_ms).
pub fn events_in_window(state: &InputState, start_ms: f64, end_ms: f64) -> Vec<(f64, InputEvent)> {
    state
        .events
        .iter()
        .filter(|e| e.t_ms >= start_ms && e.t_ms <= end_ms)
        .map(|e| (e.t_ms - start_ms, e.event.clone()))
        .collect()
}

/// Build the sidecar JSON value for one saved output.
pub fn export_json(events: &[(f64, InputEvent)], clip_name: &str, duration_ms: f64) -> serde_json::Value {
    let mut out = Vec::with_capacity(events.len());
    for (t_ms, ev) in events {
        let mut v = serde_json::to_value(ev).unwrap_or(serde_json::Value::Null);
        if let serde_json::Value::Object(map) = &mut v {
            map.insert("t_ms".to_string(), serde_json::json!(t_ms));
        }
        out.push(v);
    }
    serde_json::json!({
        "app": "rsclipping",
        "version": 1,
        "clip": clip_name,
        "duration_ms": duration_ms,
        "events": out,
    })
}

/// Per-frame overlay renderer. Holds platform draw resources (Windows GDI).
#[derive(Default)]
pub struct OverlayRenderer {
    #[cfg(windows)]
    gdi: Option<windows::GdiOverlay>,
}

impl OverlayRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Composite the overlay onto the BGRA8 frame (monitor-relative coords).
    pub fn compose(
        &mut self,
        frame: &mut [u8],
        width: u32,
        height: u32,
        position: OverlayPosition,
        snap: &OverlayFrame,
    ) {
        if snap.is_empty() {
            return;
        }
        #[cfg(windows)]
        {
            if self.gdi.is_none() {
                self.gdi = windows::GdiOverlay::new(width, height).ok();
            }
            if let Some(g) = self.gdi.as_mut() {
                if g.width() != width || g.height() != height {
                    *g = match windows::GdiOverlay::new(width, height) {
                        Ok(g) => g,
                        Err(_) => return,
                    };
                }
                let _ = g.compose(frame, width, height, position, snap);
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (width, height, position, snap, frame);
        }
    }
}