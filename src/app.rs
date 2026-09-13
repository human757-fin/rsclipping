//! Application runtime: daemon loop (capture + encode + hotkeys + output worker)
//! and one-shot CLI modes.

use crate::capture::{enumerate_monitors, CaptureSession};
use crate::config::{Action, AppConfig};
use crate::encoder::{run_concat, InputWindow, OutputJob, SegmentInfo, SEGMENT_EXTENSION};
use crate::hotkeys;
use crate::input;
use crate::storage;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

const SEG_TIMEOUT_MS: u32 = 500;

fn segment_path(seg_dir: &std::path::Path, seq: u64) -> PathBuf {
    seg_dir.join(format!("seg_{:05}.{}", seq, SEGMENT_EXTENSION))
}

/// Serialize the input events captured inside `window` next to `destination`.
fn write_input_sidecar(window: &InputWindow, destination: &Path) {
    let events = window
        .state
        .lock()
        .map(|s| input::events_in_window(&s, window.start_s * 1000.0, window.end_s * 1000.0))
        .unwrap_or_default();
    let clip_name = destination
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    let dur_ms = (window.end_s - window.start_s).max(0.0) * 1000.0;
    let json = input::export_json(&events, &clip_name, dur_ms);
    let dest = destination.with_extension("inputs.json");
    if let Some(parent) = dest.parent() {
        let _ = crate::utils::ensure_dir(parent);
    }
    match serde_json::to_string_pretty(&json) {
        Ok(s) => match std::fs::write(&dest, s) {
            Ok(()) => log::info!(
                "input events sidecar saved: {} ({} events)",
                dest.display(),
                events.len()
            ),
            Err(e) => log::warn!("failed writing input sidecar {}: {e}", dest.display()),
        },
        Err(e) => log::warn!("failed serializing input sidecar: {e}"),
    }
}

/// Spawn the single output worker: serializes concat jobs, then enforces retention.
fn spawn_output_worker(
    cfg: AppConfig,
    rx: Receiver<OutputJob>,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let job = match rx.recv_timeout(Duration::from_millis(300)) {
                Ok(job) => job,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            };
            match run_concat(&cfg.ffmpeg_path, &job.segments, &job.destination) {
                Ok(()) => {
                    let size = std::fs::metadata(&job.destination)
                        .map(|m| m.len())
                        .unwrap_or(0);
                    log::info!(
                        "saved {} ({:.1} MB, {} segments)",
                        job.destination.display(),
                        size as f64 / (1024.0 * 1024.0),
                        job.segments.len()
                    );
                    if let Some(window) = &job.input_window {
                        write_input_sidecar(window, &job.destination);
                    }
                }
                Err(e) => log::error!("concat failed for {}: {:#}", job.destination.display(), e),
            }
            if let Err(e) = storage::enforce_retention(&cfg.storage) {
                log::warn!("retention cleanup error: {e}");
            }
        }
    })
}

/// Wait until frame count has passed `needed_frames` (i.e. the tail segment is
/// closed by the ffmpeg segmuxer).
fn wait_for_frame_count(frames: &AtomicU64, needed_frames: u64, stop: &Arc<AtomicBool>) {
    const MAX_WAIT: Duration = Duration::from_secs(30);
    let start = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if frames.load(Ordering::Relaxed) >= needed_frames {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
        if start.elapsed() > MAX_WAIT {
            log::warn!("gave up waiting for segment closure (tail may be truncated)");
            return;
        }
    }
}

/// Snapshot the segment paths covering frames [start, end), including the
/// possibly still-open tail (path is deterministic even before closure).
pub fn range_segments(seg_dir: &std::path::Path, start_frame: u64, end_frame: u64, fps: u32, segment_time: f64) -> Vec<SegmentInfo> {
    let seg_frames = ((fps as f64) * segment_time).ceil().max(1.0) as u64;
    let s_idx = start_frame / seg_frames;
    let e_idx = end_frame / seg_frames;
    let mut out = Vec::new();
    for seq in s_idx..=e_idx {
        out.push(SegmentInfo {
            seq,
            path: segment_path(seg_dir, seq),
        });
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OneShotKind {
    Clip,
    Record,
}

/// A saved clip or recording surfaced in the GUI.
#[derive(Clone)]
pub struct ClipInfo {
    pub name: String,
    pub size: u64,
    pub path: String,
}

/// Live stats published by the daemon for the GUI / status consumers.
#[derive(Default)]
pub struct DaemonMetrics {
    pub fps: AtomicU64,
    pub total_frames: AtomicU64,
    pub segments_on_disk: AtomicU64,
    pub clips_saved: AtomicU64,
    pub records_saved: AtomicU64,
    pub recording: AtomicBool,
    pub monitors: AtomicU64,
    /// Width x Height of the captured monitor.
    pub resolution: std::sync::Mutex<String>,
    /// Resolved ffmpeg encoder name (e.g. libx264 / h264_nvenc).
    pub encoder: std::sync::Mutex<String>,
    /// Most recently queued output path (usually while still being saved).
    pub last_saved: std::sync::Mutex<String>,
    /// Fatal daemon error, if any (surfaces in the GUI).
    pub last_error: std::sync::Mutex<String>,
    /// List of available monitors with their resolutions.
    pub monitor_list: std::sync::Mutex<Vec<crate::capture::MonitorInfo>>,
}

/// Handle to a backgrounded daemon so the GUI can send actions, read metrics
/// and ultimately stop it.
pub struct DaemonHandle {
    pub stop: Arc<AtomicBool>,
    pub action_tx: Sender<Action>,
    pub metrics: Arc<DaemonMetrics>,
    pub thread: std::thread::JoinHandle<Result<()>>,
}

/// Spawn the daemon on its own thread and hand back a control handle.
pub fn spawn_daemon(cfg: AppConfig) -> DaemonHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let metrics = Arc::new(DaemonMetrics::default());
    let (action_tx, action_rx) = channel::<Action>();
    let cfg_t = cfg.clone();
    let stop_t = stop.clone();
    let metrics_t = metrics.clone();
    let thread = std::thread::spawn(move || {
        let res = daemon_loop(cfg_t, stop_t, action_rx, metrics_t.clone());
        if let Err(e) = &res {
            metrics_t
                .last_error
                .lock()
                .unwrap()
                .clone_from(&format!("{e:#}"));
        }
        res
    });
    DaemonHandle {
        stop,
        action_tx,
        metrics,
        thread,
    }
}

/// Blocking wrapper used by the headless CLI daemon.
pub fn run_daemon(cfg: AppConfig) -> Result<()> {
    let handle = spawn_daemon(cfg);
    let res = handle.thread.join().map_err(|_| anyhow::anyhow!("daemon thread panicked"))?;
    res
}

pub fn run_one_shot(cfg: &AppConfig, kind: OneShotKind, seconds: u32) -> Result<()> {
    let monitors = enumerate_monitors().context("monitor enumeration failed")?;
    if cfg.monitor_index >= monitors.len() {
        anyhow::bail!("monitor index {} out of range", cfg.monitor_index);
    }
    let mut cap = CaptureSession::new(cfg.monitor_index)?;
    let mut buffer = vec![0u8; cap.frame_size_bytes()];

    let mut encoder = crate::encoder::Encoder::spawn(
        &cfg.ffmpeg_path,
        &cfg.encoding,
        cap.width(),
        cap.height(),
        &cfg.storage.segment_dir,
        cfg.segment_seconds,
    )?;

    let input = if cfg.input.capture_events {
        Some(input::InputCapture::start(
            &cfg.input,
            cap.offset(),
            (cap.width(), cap.height()),
        ))
    } else {
        None
    };
    let mut overlay = input::OverlayRenderer::new();

    let deadline = Instant::now() + Duration::from_secs(seconds.max(1) as u64);
    let interval = Duration::from_secs_f64(1.0 / cfg.encoding.fps.max(1) as f64);
    let mut next = Instant::now();

    while Instant::now() < deadline {
        let now = Instant::now();
        if now < next {
            std::thread::sleep(next - now);
            continue;
        }
        next = now + interval;
        if cap.acquire(&mut buffer, SEG_TIMEOUT_MS)? {
            if let Some(ic) = &input {
                if ic.overlay_enabled() {
                    let snap = ic.overlay_snapshot();
                    if !snap.is_empty() {
                        overlay.compose(
                            &mut buffer,
                            cap.width(),
                            cap.height(),
                            ic.position(),
                            &snap,
                        );
                    }
                }
            }
            let view = cap.frame_view(&buffer);
            if let Err(e) = encoder.write_frame(view.data) {
                log::error!("encode error: {e}");
                encoder.restart();
            }
            cap.release()?;
        }
    }

    // EOF → ffmpeg closes the final segment.
    encoder.finish()?;

    let mut manager = crate::encoder::SegmentManager::new(
        cfg.storage.segment_dir.clone(),
        cfg.segment_seconds,
        cfg.clip.history_seconds,
    );
    let segs = manager.all_on_disk();

    let kind_name = if kind == OneShotKind::Clip { "clip" } else { "record" };
    let dir = if kind == OneShotKind::Clip {
        &cfg.storage.clip_dir
    } else {
        &cfg.storage.record_dir
    };
    let dest = storage::destination(dir, kind_name, cfg.encoding.codec.extension())?;
    run_concat(&cfg.ffmpeg_path, &segs, &dest)?;
    log::info!("saved {} ({} segments)", dest.display(), segs.len());

    if let Some(ic) = &input {
        let events = ic
            .state_arc()
            .lock()
            .map(|s| input::events_in_window(&s, 0.0, seconds.max(1) as f64 * 1000.0))
            .unwrap_or_default();
        let json = input::export_json(
            &events,
            &dest
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default(),
            seconds.max(1) as f64 * 1000.0,
        );
        let sidecar = dest.with_extension("inputs.json");
        if let Ok(s) = serde_json::to_string_pretty(&json) {
            match std::fs::write(&sidecar, s) {
                Ok(()) => log::info!(
                    "input events sidecar saved: {} ({} events)",
                    sidecar.display(),
                    events.len()
                ),
                Err(e) => log::warn!("failed writing input sidecar {}: {e}", sidecar.display()),
            }
        }
    }
    Ok(())
}

/// Continuous daemon loop; driven from a background thread by `spawn_daemon`.
fn daemon_loop(
    cfg: AppConfig,
    stop_flag: Arc<AtomicBool>,
    ext_rx: Receiver<Action>,
    metrics: Arc<DaemonMetrics>,
) -> Result<()> {
    if cfg.capture_audio {
        log::warn!("audio capture is not implemented yet; continuing video-only");
    }

    let monitors = enumerate_monitors().context("monitor enumeration failed")?;
    log::info!("monitors detected: {}", monitors.len());
    for (i, m) in monitors.iter().enumerate() {
        log::info!("  [{}] {}x{}", i, m.width, m.height);
    }
    if cfg.monitor_index >= monitors.len() {
        anyhow::bail!("monitor index {} out of range", cfg.monitor_index);
    }
    metrics.monitors.store(monitors.len() as u64, Ordering::Relaxed);
    metrics.monitor_list.lock().unwrap().clone_from(&monitors);

    let mut cap = CaptureSession::new(cfg.monitor_index)?;
    let mut buffer = vec![0u8; cap.frame_size_bytes()];
    metrics.resolution.lock().unwrap().clone_from(&format!("{}x{}", cap.width(), cap.height()));
    let (hk_clip, hk_rec, hk_stop) = cfg.hotkey_names();
    log::info!(
        "capture: {}x{} @ {} fps ({} / {})",
        cap.width(),
        cap.height(),
        cfg.encoding.fps,
        cfg.encoding.codec.as_str(),
        cfg.encoding.backend_as_str()
    );
    log::info!(
        "hotkeys: clip={} record={} stop={} | clip length {:.0}s",
        hk_clip,
        hk_rec,
        hk_stop,
        cfg.clip.default_length_seconds
    );
    log::info!("clip dir: {}", cfg.storage.clip_dir.display());
    log::info!("record dir: {}", cfg.storage.record_dir.display());
    log::info!("segment dir: {}", cfg.storage.segment_dir.display());

    let mut encoder = crate::encoder::Encoder::spawn(
        &cfg.ffmpeg_path,
        &cfg.encoding,
        cap.width(),
        cap.height(),
        &cfg.storage.segment_dir,
        cfg.segment_seconds,
    )?;
    metrics.encoder.lock().unwrap().clone_from(&encoder.encoder_name());

    let segment_frames = encoder.segment_frames();
    let mut manager = crate::encoder::SegmentManager::new(
        cfg.storage.segment_dir.clone(),
        cfg.segment_seconds,
        cfg.clip.history_seconds,
    );

    // Input capture (mouse/keyboard/gamepad + optional on-screen overlay).
    let input = if cfg.input.capture_events {
        let ic = input::InputCapture::start(&cfg.input, cap.offset(), (cap.width(), cap.height()));
        log::info!(
            "input capture: events on, overlay {} @ {:?}",
            if ic.overlay_enabled() { "on" } else { "off" },
            cfg.input.overlay_position
        );
        Some(ic)
    } else {
        None
    };
    let mut overlay = input::OverlayRenderer::new();

    // Hotkey events (internal) merged with GUI actions (external).
    let (hotkey_tx, hotkey_rx) = channel::<Action>();
    let stop_hotkey = stop_flag.clone();
    let hk_cfg = cfg.clone();
    std::thread::spawn(move || {
        if let Err(e) = hotkeys::run(
            stop_hotkey,
            hotkey_tx,
            hk_cfg.hotkey_clip,
            hk_cfg.hotkey_record_start,
            hk_cfg.hotkey_record_stop,
        ) {
            log::error!("hotkey thread error: {e}");
        }
    });

    // Output worker
    let (job_tx, job_rx) = channel::<OutputJob>();
    let worker_cfg = cfg.clone();
    let worker_stop = stop_flag.clone();
    let _worker = spawn_output_worker(worker_cfg, job_rx, worker_stop);

    let mut recording_start: Option<u64> = None;
    let mut record_start_seq: Option<u64> = None;
    let mut last_poll = Instant::now();
    let mut fps_win_start = Instant::now();
    let mut fps_frames = 0u32;

    let interval = Duration::from_secs_f64(1.0 / cfg.encoding.fps.max(1) as f64);
    let mut next = Instant::now();

    log::info!("ready. press hotkeys to clip / record. Ctrl+C to exit.");

    let dispatch = |a: Action,
                    encoder: &mut crate::encoder::Encoder,
                    manager: &mut crate::encoder::SegmentManager,
                    job_tx: &Sender<OutputJob>,
                    recording_start: &mut Option<u64>,
                    record_start_seq: &mut Option<u64>,
                    input: &Option<Arc<input::InputCapture>>|
     -> Result<()> {
        let open_seq = encoder.open_segment_index();
        let fps = cfg.encoding.fps.max(1) as f64;
        match a {
            Action::Clip => {
                let need = ((cfg.clip.default_length_seconds as f64) / cfg.segment_seconds).ceil() as usize;
                let segs = manager.take_last(open_seq, need);
                if segs.is_empty() {
                    log::warn!("clip ignored: rolling buffer still warming up");
                    return Ok(());
                }
                let start_s = open_seq.saturating_sub(need as u64) as f64 * segment_frames as f64 / fps;
                let end_s = open_seq as f64 * segment_frames as f64 / fps;
                let iw = input.as_ref().map(|ic| InputWindow {
                    start_s,
                    end_s,
                    state: ic.state_arc(),
                });
                let dest = storage::destination(&cfg.storage.clip_dir, "clip", cfg.encoding.codec.extension())?;
                log::info!("clip queued: {}", dest.display());
                metrics.last_saved.lock().unwrap().clone_from(&dest.display().to_string());
                metrics.clips_saved.fetch_add(1, Ordering::Relaxed);
                let _ = job_tx.send(OutputJob { segments: segs, destination: dest, input_window: iw });
            }
            Action::RecordStart => {
                if recording_start.is_some() {
                    log::info!("recording already in progress");
                    return Ok(());
                }
                let frame = encoder.total_frames.load(Ordering::Relaxed);
                *recording_start = Some(frame);
                *record_start_seq = Some(manager.segment_of_frame(frame, cfg.encoding.fps).min(open_seq));
                metrics.recording.store(true, Ordering::Relaxed);
                log::info!("recording started (frame {})", frame);
            }
            Action::RecordStop => {
                let Some(start_frame) = recording_start.take() else {
                    log::info!("no recording in progress");
                    return Ok(());
                };
                *record_start_seq = None;
                metrics.recording.store(false, Ordering::Relaxed);
                let end_frame = encoder.total_frames.load(Ordering::Relaxed);
                if end_frame.saturating_sub(start_frame) < 2 {
                    log::warn!("recording too short to save");
                    return Ok(());
                }
                let segs = range_segments(
                    &cfg.storage.segment_dir,
                    start_frame,
                    end_frame,
                    cfg.encoding.fps,
                    cfg.segment_seconds,
                );
                let tail_seq = segs.last().map(|s| s.seq).unwrap_or(0);
                let dest = storage::destination(&cfg.storage.record_dir, "record", cfg.encoding.codec.extension())?;
                log::info!("finalizing recording: {} segments", segs.len());
                metrics.last_saved.lock().unwrap().clone_from(&dest.display().to_string());
                metrics.records_saved.fetch_add(1, Ordering::Relaxed);

                let iw = input.as_ref().map(|ic| InputWindow {
                    start_s: start_frame as f64 / fps,
                    end_s: end_frame as f64 / fps,
                    state: ic.state_arc(),
                });

                // Wait for the tail segment to close without stalling capture.
                let frames = encoder.total_frames.clone();
                let needed = (tail_seq + 1).saturating_mul(segment_frames);
                let tx2 = job_tx.clone();
                let stop_wait = stop_flag.clone();
                std::thread::spawn(move || {
                    wait_for_frame_count(&frames, needed, &stop_wait);
                    std::thread::sleep(Duration::from_millis(150));
                    let _ = tx2.send(OutputJob { segments: segs, destination: dest, input_window: iw });
                });
            }
        }
        Ok(())
    };

    while !stop_flag.load(Ordering::Relaxed) {
        let now = Instant::now();
        if now < next {
            std::thread::sleep(next - now);
            continue;
        }
        next = now + interval;

        // capture + encode
        match cap.acquire(&mut buffer, SEG_TIMEOUT_MS) {
            Ok(true) => {
                if let Some(ic) = &input {
                    if ic.overlay_enabled() {
                        let snap = ic.overlay_snapshot();
                        if !snap.is_empty() {
                            overlay.compose(
                                &mut buffer,
                                cap.width(),
                                cap.height(),
                                ic.position(),
                                &snap,
                            );
                        }
                    }
                }
                let view = cap.frame_view(&buffer);
                if let Err(e) = encoder.write_frame(view.data) {
                    log::error!("encode error: {e}");
                    encoder.restart();
                }
                fps_frames += 1;
                if let Err(e) = cap.release() {
                    log::warn!("release error: {e}");
                }
            }
            Ok(false) => {}
            Err(e) => log::warn!("capture error: {e}"),
        }

        // periodic maintenance (~2 Hz)
        if last_poll.elapsed() >= Duration::from_millis(500) {
            last_poll = Instant::now();
            let open_seq = encoder.open_segment_index();
            manager.trim(open_seq, record_start_seq, cfg.encoding.fps);
            if encoder.is_broken() {
                encoder.restart();
            }
            metrics.total_frames.store(encoder.total_frames.load(Ordering::Relaxed), Ordering::Relaxed);
            metrics.segments_on_disk.store(manager.count_segments(), Ordering::Relaxed);
        }

        // fps readout (1 Hz)
        if fps_win_start.elapsed() >= Duration::from_secs(1) {
            metrics.fps.store(fps_frames as u64, Ordering::Relaxed);
            fps_frames = 0;
            fps_win_start = Instant::now();
        }

        // commands from hotkeys and the GUI
        for rx in [&hotkey_rx, &ext_rx] {
            while let Ok(a) = rx.try_recv() {
                if let Err(e) = dispatch(
                    a,
                    &mut encoder,
                    &mut manager,
                    &job_tx,
                    &mut recording_start,
                    &mut record_start_seq,
                    &input,
                ) {
                    log::error!("action error: {e}");
                }
            }
        }
    }

    encoder.kill_mut();
    log::info!("shutdown complete");
    Ok(())
}