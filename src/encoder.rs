//! Persistent FFmpeg pipeline writing short segments to a rolling window.
//!
//! One FFmpeg child process is alive the whole session. Screen frames are piped
//! in as rawvideo and FFmpeg segments them every `segment_time` seconds.
//! Architecture locks in absurdly small RAM: the only memory is FFmpeg's own
//! encode queues plus a couple of frame buffers. A clip is produced by
//! *copying* (remuxing) whole segments — never by re-encoding.

use crate::config::{Codec, EncoderBackend, EncoderConfig};
use crate::utils;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;

pub const SEGMENT_EXTENSION: &str = "mkv";

pub struct Encoder {
    child: Child,
    stdin: Option<std::process::ChildStdin>,
    _stderr_reader: thread::JoinHandle<()>,
    fps: u32,
    pub frame_bytes: usize,
    pub segment_time: f64,
    pub ffmpeg: PathBuf,
    pub total_frames: Arc<AtomicU64>,
    pub encoder_name: String,
    encode_opts: Vec<String>,
    broken: bool,
    last_restart: std::time::Instant,
}

impl Encoder {
    #[allow(clippy::too_many_arguments)]
    pub fn build_ffmpeg_args(
        ffmpeg: &Path,
        codec: Codec,
        backend: EncoderBackend,
        enc: &EncoderConfig,
        width: u32,
        height: u32,
        seg_dir: &Path,
        segment_time: f64,
        log_level: &str,
    ) -> Result<Vec<String>> {
        let encoder = utils::pick_encoder(codec, backend, ffmpeg)?;
        let mut args: Vec<String> = Vec::new();

        args.extend([
            "-hide_banner".into(),
            "-loglevel".into(),
            log_level.into(),
            "-y".into(),
            "-f".into(),
            "rawvideo".into(),
            "-pix_fmt".into(),
            "bgra".into(),
            "-s".into(),
            format!("{}x{}", width, height),
            "-r".into(),
            enc.fps.to_string(),
            "-i".into(),
            "pipe:0".into(),
        ]);

        args.push("-an".into());

        args.push("-c:v".into());
        args.push(encoder.clone());

        let gop = ((enc.fps * 2) as i64).max(24);
        let keyint = gop.to_string();
        if encoder.starts_with("libx264") {
            args.push("-preset".into());
            args.push(crate::config::sanitize_preset(&enc.preset));
            if enc.bitrate_kbps > 0 {
                args.push("-b:v".into());
                args.push(format!("{}k", enc.bitrate_kbps));
            }
            args.push("-crf".into());
            args.push(enc.crf.to_string());
            args.push("-x264-params".into());
            args.push(format!("keyint={}:min-keyint={}", keyint, keyint));
        } else if encoder.starts_with("libx265") {
            args.push("-preset".into());
            args.push(crate::config::sanitize_preset(&enc.preset));
            if enc.bitrate_kbps > 0 {
                args.push("-b:v".into());
                args.push(format!("{}k", enc.bitrate_kbps));
            }
            args.push("-crf".into());
            args.push(enc.crf.to_string());
            args.push("-x265-params".into());
            args.push(format!("keyint={}:min-keyint={}", keyint, keyint));
        } else if encoder.starts_with("libvpx-vp9") {
            args.push("-crf".into());
            args.push(enc.crf.to_string());
            if enc.bitrate_kbps > 0 {
                args.push("-b:v".into());
                args.push(format!("{}k", enc.bitrate_kbps));
            }
            args.push("-deadline".into());
            args.push("realtime".into());
            args.push("-cpu-used".into());
            args.push("6".into());
            args.push("-row-mt".into());
            args.push("1".into());
            args.push("-g".into());
            args.push(keyint.clone());
        } else if encoder.starts_with("libaom") {
            args.push("-crf".into());
            args.push(enc.crf.to_string());
            args.push("-cpu-used".into());
            args.push("8".into());
            args.push("-row-mt".into());
            args.push("1".into());
            args.push("-g".into());
            args.push(keyint.clone());
        } else if encoder.contains("nvenc") {
            args.push("-preset".into());
            match enc.preset.as_str() {
                "veryfast" | "ultrafast" | "superfast" => args.push("p1".into()),
                "fast" => args.push("p2".into()),
                "medium" | "" => args.push("p3".into()),
                "slow" => args.push("p4".into()),
                "slower" => args.push("p5".into()),
                "veryslow" | "placebo" => args.push("p6".into()),
                other => args.push(other.into()),
            }
            args.push("-rc".into());
            args.push("vbr".into());
            args.push("-cq".into());
            args.push(enc.crf.to_string());
            args.push("-b:v".into());
            args.push("0".into());
            if !encoder.contains("av1") {
                args.push("-tune".into());
                args.push("ll".into());
            }
            args.push("-g".into());
            args.push(keyint.clone());
        } else if encoder.ends_with("_amf") {
            args.push("-quality".into());
            args.push("speed".into());
            args.push("-usage".into());
            args.push("ultrafast".into());
            args.push("-rc".into());
            args.push("vbr".into());
            args.push("-qp_i".into());
            args.push(enc.crf.to_string());
            args.push("-qp_p".into());
            args.push(enc.crf.to_string());
            args.push("-g".into());
            args.push(keyint.clone());
        } else if encoder.ends_with("_qsv") {
            args.push("-preset".into());
            args.push("veryfast".into());
            args.push("-global_quality".into());
            args.push(enc.crf.to_string());
            args.push("-g".into());
            args.push(keyint.clone());
        }

        args.push("-pix_fmt".into());
        args.push(enc.pixel_format.clone());
        args.extend(enc.extra_encoder_args.iter().cloned());

        args.push("-f".into());
        args.push("segment".into());
        args.push("-segment_time".into());
        args.push(format!("{:.3}", segment_time));
        args.push("-reset_timestamps".into());
        args.push("1".into());
        args.push("-segment_format".into());
        args.push(SEGMENT_EXTENSION.into());
        args.push("-avoid_negative_ts".into());
        args.push("make_zero".into());

        let pattern = seg_dir.join(format!("seg_%05d.{}", SEGMENT_EXTENSION));
        args.push(pattern.to_string_lossy().into_owned());
        Ok(args)
    }

    pub fn spawn(
        ffmpeg: &Path,
        enc: &EncoderConfig,
        width: u32,
        height: u32,
        seg_dir: &Path,
        segment_time: f64,
    ) -> Result<Encoder> {
        utils::ensure_dir(seg_dir)?;
        if let Ok(entries) = std::fs::read_dir(seg_dir) {
            for e in entries.flatten() {
                let p = e.path();
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if name.starts_with("seg_") && name.ends_with(SEGMENT_EXTENSION) {
                    let _ = std::fs::remove_file(p);
                }
            }
        }

        let args = Encoder::build_ffmpeg_args(
            ffmpeg,
            enc.codec,
            enc.backend,
            enc,
            width,
            height,
            seg_dir,
            segment_time,
            "error",
        )?;

        log::debug!("ffmpeg args: {}", args.join(" "));
        let mut child = Command::new(ffmpeg)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn ffmpeg")?;
        let stdin = child.stdin.take().context("ffmpeg stdin missing")?;
        let stderr = child.stderr.take().context("ffmpeg stderr missing")?;

        let err_reader = thread::spawn(move || {
            let mut buf = [0u8; 2048];
            let mut reader = std::io::BufReader::new(stderr);
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        for line in String::from_utf8_lossy(&buf[..n]).lines() {
                            log::error!("[ffmpeg] {}", line.trim());
                        }
                    }
                }
            }
        });

        let total_frames = Arc::new(AtomicU64::new(0));
        let encoder_name = args
            .windows(2)
            .find(|w| w[0] == "-c:v")
            .and_then(|w| w.get(1).cloned())
            .unwrap_or_else(|| enc.codec.ffmpeg_cpu().to_string());
        Ok(Encoder {
            child,
            stdin: Some(stdin),
            _stderr_reader: err_reader,
            fps: enc.fps,
            frame_bytes: (width as usize) * (height as usize) * 4,
            segment_time,
            ffmpeg: ffmpeg.to_path_buf(),
            total_frames,
            encoder_name,
            encode_opts: args,
            broken: false,
            last_restart: std::time::Instant::now(),
        })
    }

    /// Write one raw frame (BGRA8, tightly packed rows). Returns the new total frame count.
    pub fn write_frame(&mut self, data: &[u8]) -> Result<u64> {
        if self.broken {
            return Err(anyhow::anyhow!("encoder is broken"));
        }
        if data.len() < self.frame_bytes {
            anyhow::bail!("frame data too small ({} < {})", data.len(), self.frame_bytes);
        }
        let stdin = self.stdin.as_mut().ok_or_else(|| anyhow::anyhow!("encoder already finished"))?;
        stdin.write_all(&data[..self.frame_bytes]).map_err(|e| {
            self.broken = true;
            anyhow::anyhow!("ffmpeg pipe write failed: {e}")
        })?;
        Ok(self.total_frames.fetch_add(1, Ordering::Relaxed) + 1)
    }

    /// Close stdin (EOF) and wait for ffmpeg to finish and close the final segment.
    pub fn finish(&mut self) -> Result<()> {
        self.stdin.take();
        let status = self.child.wait().context("ffmpeg did not exit cleanly")?;
        if !status.success() {
            anyhow::bail!("ffmpeg exited with status {:?}", status.code());
        }
        Ok(())
    }

    pub fn restart(&mut self) {
        if self.broken {
            const MIN_GAP: std::time::Duration = std::time::Duration::from_millis(1500);
            if self.last_restart.elapsed() < MIN_GAP {
                log::warn!("ffmpeg restart throttled (still broken, waited {:.1}s)", self.last_restart.elapsed().as_secs_f64());
                return;
            }
            self.last_restart = std::time::Instant::now();
            if let Ok(mut child) = Command::new(&self.ffmpeg)
                .args(&self.encode_opts)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
            {
                if let Some(stdin) = child.stdin.take() {
                    let stderr = child.stderr.take();
                    if let Some(stderr) = stderr {
                        thread::spawn(move || {
                            let mut buf = [0u8; 2048];
                            let mut reader = std::io::BufReader::new(stderr);
                            loop {
                                match reader.read(&mut buf) {
                                    Ok(0) | Err(_) => break,
                                    Ok(n) => {
                                        for line in String::from_utf8_lossy(&buf[..n]).lines() {
                                            log::error!("[ffmpeg] {}", line.trim());
                                        }
                                    }
                                }
                            }
                        });
                    }
                    self.child = child;
                    self.stdin = Some(stdin);
                    self.broken = false;
                    log::warn!("ffmpeg restarted");
                }
            }
        }
    }

    /// Frames per segment.
    pub fn segment_frames(&self) -> u64 {
        ((self.fps as f64) * self.segment_time).ceil().max(1.0) as u64
    }

    /// Resolved ffmpeg encoder name (e.g. libx264, h264_nvenc).
    pub fn encoder_name(&self) -> String {
        self.encoder_name.clone()
    }

    /// Index (seq) of the segment currently being written.
    pub fn open_segment_index(&self) -> u64 {
        self.total_frames.load(Ordering::Relaxed) / self.segment_frames().max(1)
    }

    pub fn is_broken(&self) -> bool {
        self.broken
    }

    pub fn kill_mut(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One finalized segment on disk.
#[derive(Debug, Clone)]
pub struct SegmentInfo {
    pub seq: u64,
    pub path: PathBuf,
}

/// Rolling-window segment manager. Called from the main loop; single-threaded.
pub struct SegmentManager {
    seg_dir: PathBuf,
    pub segment_time: f64,
    pub history_seconds: u32,
    cache: BTreeMap<u64, PathBuf>,
}

impl SegmentManager {
    pub fn new(seg_dir: PathBuf, segment_time: f64, history_seconds: u32) -> SegmentManager {
        SegmentManager {
            seg_dir,
            segment_time,
            history_seconds,
            cache: BTreeMap::new(),
        }
    }

    pub fn segment_frames(&self, fps: u32) -> u64 {
        ((fps as f64) * self.segment_time).ceil().max(1.0) as u64
    }

    /// Seq index that contains a given frame count.
    pub fn segment_of_frame(&self, frame: u64, fps: u32) -> u64 {
        frame / self.segment_frames(fps).max(1)
    }

    /// Number of segment files present on disk right now.
    pub fn count_segments(&self) -> u64 {
        std::fs::read_dir(&self.seg_dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| {
                        p.file_name()
                            .map(|n| n.to_string_lossy().starts_with("seg_"))
                            .unwrap_or(false)
                    })
                    .count() as u64
            })
            .unwrap_or(0)
    }

    /// All finalized segments (seq < open_seq) sorted ascending.
    pub fn finalized(&mut self, open_seq: u64) -> Vec<SegmentInfo> {
        self.refresh();
        let mut out: Vec<SegmentInfo> = self
            .cache
            .iter()
            .filter(|(&seq, p)| seq < open_seq && p.exists())
            .map(|(&seq, p)| SegmentInfo { seq, path: p.clone() })
            .collect();
        out.sort_by_key(|s| s.seq);
        out
    }

    /// Up to `n` most-recent finalized segments.
    pub fn take_last(&mut self, open_seq: u64, n: usize) -> Vec<SegmentInfo> {
        let all = self.finalized(open_seq);
        let skip = all.len().saturating_sub(n.max(1));
        all[skip..].to_vec()
    }

    /// Every segment currently on disk (used after encoder EOF, when the last
    /// segment is guaranteed closed even though its seq >= open_seq).
    pub fn all_on_disk(&mut self) -> Vec<SegmentInfo> {
        self.refresh();
        let mut out: Vec<SegmentInfo> = self
            .cache
            .iter()
            .filter(|(_, p)| p.exists())
            .map(|(&seq, p)| SegmentInfo { seq, path: p.clone() })
            .collect();
        out.sort_by_key(|s| s.seq);
        out
    }

    /// Segments overlapping a frame range [start, end) — plus one extra after
    /// so remuxers get a trailing keyframe boundary. Returns sorted.
    #[allow(dead_code)]
    pub fn in_range(&mut self, open_seq: u64, start_frame: u64, end_frame: u64, fps: u32) -> Vec<SegmentInfo> {
        let all = self.finalized(open_seq);
        let s_idx = self.segment_of_frame(start_frame, fps);
        let e_idx = self.segment_of_frame(end_frame, fps).min(open_seq.saturating_sub(1));
        all.into_iter().filter(|s| s.seq >= s_idx && s.seq <= e_idx).collect()
    }

    /// Delete segments that fall outside the rolling window / active recording.
    pub fn trim(&mut self, open_seq: u64, protected_from_seg: Option<u64>, _fps: u32) {
        self.refresh();
        let history_segs = ((self.history_seconds as f64) / self.segment_time).ceil() as u64;
        let oldest_keep = open_seq.saturating_sub(history_segs).max(1);
        let oldest_keep = oldest_keep.max(protected_from_seg.unwrap_or(0));

        let min_age = std::time::Duration::from_secs_f64(self.segment_time * 1.5);
        if let Ok(entries) = std::fs::read_dir(&self.seg_dir) {
            for e in entries.flatten() {
                let path = e.path();
                if !path.is_file() {
                    continue;
                }
                let name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n,
                    None => continue,
                };
                let Some(rest) = name
                    .strip_prefix("seg_")
                    .and_then(|r| r.strip_suffix(&format!(".{SEGMENT_EXTENSION}")))
                else {
                    continue;
                };
                let Ok(seq) = rest.parse::<u64>() else {
                    continue;
                };
                if seq >= oldest_keep {
                    continue;
                }
                // Only remove files old enough that they're definitely closed.
                let too_young = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|mt| std::time::SystemTime::now().duration_since(mt).ok())
                    .map(|age| age < min_age)
                    .unwrap_or(true);
                if !too_young {
                    let _ = std::fs::remove_file(&path);
                    self.cache.remove(&seq);
                }
            }
        }
        self.cache.retain(|seq, p| *seq >= oldest_keep || p.exists());
    }

    fn refresh(&mut self) {
        if let Ok(entries) = std::fs::read_dir(&self.seg_dir) {
            for e in entries.flatten() {
                let path = e.path();
                if !path.is_file() {
                    continue;
                }
                let name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n,
                    None => continue,
                };
                if let Some(rest) = name.strip_prefix("seg_") {
                    if let Some(rest) = rest.strip_suffix(&format!(".{}", SEGMENT_EXTENSION)) {
                        if let Ok(seq) = rest.parse::<u64>() {
                            self.cache.insert(seq, path);
                        }
                    }
                }
            }
        }
    }
}

/// An output job dispatched to the worker thread (concatenation = cheap copy).
#[derive(Debug, Clone)]
pub struct OutputJob {
    pub segments: Vec<SegmentInfo>,
    pub destination: PathBuf,
}

/// ffmpeg concat remux (no re-encode → low CPU/RAM).
pub fn run_concat(ffmpeg: &Path, segments: &[SegmentInfo], destination: &Path) -> Result<()> {
    if segments.is_empty() {
        anyhow::bail!("no segments to concatenate");
    }
    if let Some(parent) = destination.parent() {
        utils::ensure_dir(parent)?;
    }
    let list_path = destination.with_extension("concatlist.txt");
    let mut list = String::new();
    for s in segments {
        list.push_str(&format!(
            "file '{}'\n",
            s.path.to_string_lossy().replace('\\', "/")
        ));
    }
    std::fs::write(&list_path, list)?;

    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
        "-f".into(),
        "concat".into(),
        "-safe".into(),
        "0".into(),
        "-i".into(),
        list_path.to_string_lossy().into_owned(),
        "-c".into(),
        "copy".into(),
    ];
    let ext = destination
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if ext == "mp4" {
        args.push("-movflags".into());
        args.push("+faststart".into());
    }
    args.push(destination.to_string_lossy().into_owned());

    let status = Command::new(ffmpeg)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .context("failed to run ffmpeg concat")?;
    let _ = std::fs::remove_file(&list_path);
    if !status.success() {
        anyhow::bail!("ffmpeg concat failed with status {:?}", status.code());
    }
    Ok(())
}