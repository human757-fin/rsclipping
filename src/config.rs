use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Codec {
    #[default]
    H264,
    H265,
    VP9,
    AV1,
}

impl Codec {
    pub fn all() -> &'static [Codec] {
        &[Codec::H264, Codec::H265, Codec::VP9, Codec::AV1]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Codec::H264 => "h264",
            Codec::H265 => "h265",
            Codec::VP9 => "vp9",
            Codec::AV1 => "av1",
        }
    }

    pub fn from_name(s: &str) -> Option<Codec> {
        let s = s.to_lowercase().replace(['.', '-'], "");
        match s.as_str() {
            "h264" | "avc" | "x264" => Some(Codec::H264),
            "h265" | "hevc" | "x265" => Some(Codec::H265),
            "vp9" => Some(Codec::VP9),
            "av1" | "aom" => Some(Codec::AV1),
            _ => None,
        }
    }

    /// FFmpeg encoder name for CPU (libx) encoders.
    pub fn ffmpeg_cpu(&self) -> &'static str {
        match self {
            Codec::H264 => "libx264",
            Codec::H265 => "libx265",
            Codec::VP9 => "libvpx-vp9",
            Codec::AV1 => "libaom-av1",
        }
    }

    /// Recommended GPU encoder name (NVENC on NVIDIA, AMF on AMD, QSV on Intel).
    /// Returns None when no reliable GPU encoder is known for the codec.
    pub fn ffmpeg_gpu(&self) -> Option<&'static str> {
        match self {
            Codec::H264 => Some("h264_nvenc"),
            Codec::H265 => Some("hevc_nvenc"),
            Codec::VP9 => None,
            Codec::AV1 => Some("av1_nvenc"),
        }
    }

    pub fn ffmpeg_gpu_fallback(&self) -> &'static str {
        match self {
            Codec::H264 => "h264_amf",
            Codec::H265 => "hevc_amf",
            Codec::VP9 => "libvpx-vp9",
            Codec::AV1 => "av1_qsv",
        }
    }

    /// Container extension for output files.
    pub fn extension(&self) -> &'static str {
        match self {
            Codec::H264 | Codec::H265 => "mp4",
            Codec::VP9 => "webm",
            Codec::AV1 => "mkv",
        }
    }

    pub fn is_gpu_encodable(&self) -> bool {
        !matches!(self, Codec::VP9)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum EncoderBackend {
    #[default]
    Auto,
    Cpu,
    Gpu,
}

/// Windows `RegisterHotKey` modifier flag bits (same values as `MOD_ALT` etc.).
pub const MOD_ALT: u8 = 0x01;
pub const MOD_CTRL: u8 = 0x02;
pub const MOD_SHIFT: u8 = 0x04;
pub const MOD_WIN: u8 = 0x08;

/// A global hotkey: zero or more modifier flags plus one base key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub mods: u8,
    pub key: HotkeyKey,
}

/// Base key (without modifiers) for a global hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyKey {
    /// `n` in 1..=12 maps to VK_F1..=VK_F12.
    F(u8),
    PrintScreen,
    /// Lowercase ascii letter.
    Letter(u8),
    /// Digit 0..=9.
    Digit(u8),
    Space,
}

impl Hotkey {
    pub const fn new(mods: u8, key: HotkeyKey) -> Hotkey {
        Hotkey { mods, key }
    }

    pub fn from_str(s: &str) -> Option<Hotkey> {
        let s = s.to_lowercase();
        let mut mods: u8 = 0;
        let mut key: Option<HotkeyKey> = None;
        for tok in s.split('+') {
            let tok = tok.replace(['_', '-', ' ', '\\'], "");
            match tok.as_str() {
                "alt" => mods |= MOD_ALT,
                "ctrl" | "control" | "ctl" => mods |= MOD_CTRL,
                "shift" => mods |= MOD_SHIFT,
                "win" | "windows" | "super" | "cmd" | "meta" => mods |= MOD_WIN,
                other => {
                    if key.is_none() {
                        key = HotkeyKey::from_str(other);
                    } else {
                        return None;
                    }
                }
            }
        }
        Some(Hotkey { mods, key: key? })
    }

    /// Config string form, e.g. `"f8"` or `"alt+f11"`.
    pub fn as_str(&self) -> String {
        let mut out = String::new();
        if self.mods & MOD_ALT != 0 {
            out.push_str("alt+");
        }
        if self.mods & MOD_CTRL != 0 {
            out.push_str("ctrl+");
        }
        if self.mods & MOD_SHIFT != 0 {
            out.push_str("shift+");
        }
        if self.mods & MOD_WIN != 0 {
            out.push_str("win+");
        }
        out.push_str(self.key.as_str());
        out
    }

    /// Human display form, e.g. `"F8"` or `"Alt+F11"`.
    pub fn display(&self) -> String {
        let mut out = String::new();
        if self.mods & MOD_ALT != 0 {
            out.push_str("Alt+");
        }
        if self.mods & MOD_CTRL != 0 {
            out.push_str("Ctrl+");
        }
        if self.mods & MOD_SHIFT != 0 {
            out.push_str("Shift+");
        }
        if self.mods & MOD_WIN != 0 {
            out.push_str("Win+");
        }
        out.push_str(&self.key.display());
        out
    }

    /// Virtual-key code used by RegisterHotKey (Windows).
    #[allow(dead_code)]
    pub fn vk_code(&self) -> u32 {
        self.key.vk_code()
    }

    /// Windows `RegisterHotKey` modifier flags (`HOT_KEY_MODIFIERS` bit value).
    #[allow(dead_code)]
    pub fn mods_code(&self) -> u32 {
        self.mods as u32
    }
}

impl HotkeyKey {
    fn from_str(s: &str) -> Option<HotkeyKey> {
        if let Some(n) = s.strip_prefix('f') {
            if let Ok(n) = n.parse::<u8>() {
                if (1..=12).contains(&n) {
                    return Some(HotkeyKey::F(n));
                }
            }
        }
        match s {
            "print" | "printscreen" | "prtsc" | "prtscr" | "snapshot" => Some(HotkeyKey::PrintScreen),
            "space" => Some(HotkeyKey::Space),
            _ => {
                if s.len() == 1 {
                    let c = s.as_bytes()[0];
                    if c.is_ascii_lowercase() {
                        return Some(HotkeyKey::Letter(c));
                    }
                    if c.is_ascii_digit() {
                        return Some(HotkeyKey::Digit(c - b'0'));
                    }
                }
                None
            }
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            HotkeyKey::F(n) => match n {
                1 => "f1",
                2 => "f2",
                3 => "f3",
                4 => "f4",
                5 => "f5",
                6 => "f6",
                7 => "f7",
                8 => "f8",
                9 => "f9",
                10 => "f10",
                11 => "f11",
                12 => "f12",
                _ => "f12",
            },
            HotkeyKey::PrintScreen => "print_screen",
            HotkeyKey::Letter(c) => match c {
                b'a' => "a",
                b'b' => "b",
                b'c' => "c",
                b'd' => "d",
                b'e' => "e",
                b'f' => "f",
                b'g' => "g",
                b'h' => "h",
                b'i' => "i",
                b'j' => "j",
                b'k' => "k",
                b'l' => "l",
                b'm' => "m",
                b'n' => "n",
                b'o' => "o",
                b'p' => "p",
                b'q' => "q",
                b'r' => "r",
                b's' => "s",
                b't' => "t",
                b'u' => "u",
                b'v' => "v",
                b'w' => "w",
                b'x' => "x",
                b'y' => "y",
                b'z' => "z",
                _ => "key",
            },
            HotkeyKey::Digit(d) => match d {
                0 => "0",
                1 => "1",
                2 => "2",
                3 => "3",
                4 => "4",
                5 => "5",
                6 => "6",
                7 => "7",
                8 => "8",
                9 => "9",
                _ => "0",
            },
            HotkeyKey::Space => "space",
        }
    }

    fn display(&self) -> String {
        match self {
            HotkeyKey::F(n) => format!("F{n}"),
            HotkeyKey::PrintScreen => "Print Screen".to_string(),
            HotkeyKey::Letter(c) => (*c as char).to_ascii_uppercase().to_string(),
            HotkeyKey::Digit(d) => d.to_string(),
            HotkeyKey::Space => "Space".to_string(),
        }
    }

    fn vk_code(&self) -> u32 {
        match self {
            HotkeyKey::F(n) => 0x70 + u32::from(*n) - 1,
            HotkeyKey::PrintScreen => 0x2C,
            HotkeyKey::Letter(c) => u32::from(*c - b'a') + 0x41,
            HotkeyKey::Digit(d) => 0x30 + u32::from(*d),
            HotkeyKey::Space => 0x20,
        }
    }
}

impl serde::Serialize for Hotkey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for Hotkey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Hotkey::from_str(&s).ok_or_else(|| serde::de::Error::custom(format!("invalid hotkey '{s}'")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Clip,
    RecordStart,
    RecordStop,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipConfig {
    /// How many seconds of history to keep in the rolling window.
    pub history_seconds: u32,
    /// Default clip length in seconds when the clip hotkey is pressed.
    pub default_length_seconds: u32,
}

impl Default for ClipConfig {
    fn default() -> Self {
        Self {
            history_seconds: 30,
            default_length_seconds: 15,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncoderConfig {
    pub codec: Codec,
    pub backend: EncoderBackend,
    pub fps: u32,
    /// Bitrate for recordings/clips. `0` -> use CRF only.
    pub bitrate_kbps: u32,
    /// CRF/quality (lower = better). 18-23 is typical.
    pub crf: u32,
    /// x264/x265 present: ultrafast..placebo. NVENC presets: p1..p7 (best..fastest).
    pub preset: String,
    /// Extra raw FFmpeg args appended to the encoder (e.g. `-aq-mode 1`).
    pub extra_encoder_args: Vec<String>,
    /// Pixel format passed to encoder (yuv420p universally safe).
    pub pixel_format: String,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            codec: Codec::H264,
            backend: EncoderBackend::Auto,
            fps: 60,
            bitrate_kbps: 0,
            crf: 20,
            preset: "veryfast".to_string(),
            extra_encoder_args: Vec::new(),
            pixel_format: "yuv420p".to_string(),
        }
    }
}

impl EncoderConfig {
    pub fn backend_as_str(&self) -> &'static str {
        match self.backend {
            EncoderBackend::Auto => "auto",
            EncoderBackend::Cpu => "cpu",
            EncoderBackend::Gpu => "gpu",
        }
    }
}

/// Valid x264/x265 preset names. Numeric NVENC presets (`p1..p7`) are only valid
/// for GPU encoders; CPU encoders get a sane named preset instead.
pub fn sanitize_preset(preset: &str) -> String {
    const NAMED: &[&str] = &[
        "ultrafast",
        "superfast",
        "veryfast",
        "faster",
        "fast",
        "medium",
        "slow",
        "slower",
        "veryslow",
        "placebo",
    ];
    if NAMED.contains(&preset) {
        return preset.to_string();
    }
    match preset {
        "p1" | "p2" => "slow",
        "p3" | "p4" => "medium",
        "p5" | "p6" => "fast",
        "p7" => "veryfast",
        _ => "medium",
    }
    .to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Storage {
    pub clip_dir: PathBuf,
    pub record_dir: PathBuf,
    /// Rolling window scratch dir where segments are written.
    pub segment_dir: PathBuf,
    /// Maximum combined clip+record storage before auto-cleanup kicks in (MB). 0 = unlimited.
    pub max_storage_mb: u64,
    /// Max number of clips retained. 0 = keep everything.
    pub max_clips: usize,
    pub auto_cleanup: bool,
}

impl Default for Storage {
    fn default() -> Self {
        let base = dirs::desktop_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")));
        Self {
            clip_dir: base.join("RSClipping"),
            record_dir: base.join("RSClippingRecordings"),
            segment_dir: std::env::temp_dir().join("rsclipping_segments"),
            max_storage_mb: 4096,
            max_clips: 500,
            auto_cleanup: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputConfig {
    /// Master switch: record mouse/keyboard/controller events for clips
    /// (saved as a `*.inputs.json` sidecar next to each output).
    pub capture_events: bool,
    /// Draw a key/mouse overlay into the recorded video frames.
    pub overlay_enabled: bool,
    /// Which corner the overlay anchors to.
    pub overlay_position: OverlayPosition,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            capture_events: true,
            overlay_enabled: false,
            overlay_position: OverlayPosition::BottomLeft,
        }
    }
}

/// Corner anchor for the on-screen key overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum OverlayPosition {
    #[default]
    BottomLeft,
    BottomRight,
    TopLeft,
    TopRight,
}

impl OverlayPosition {
    pub fn label(&self) -> &'static str {
        match self {
            OverlayPosition::BottomLeft => "Bottom-left",
            OverlayPosition::BottomRight => "Bottom-right",
            OverlayPosition::TopLeft => "Top-left",
            OverlayPosition::TopRight => "Top-right",
        }
    }

    pub fn all() -> &'static [OverlayPosition] {
        &[
            OverlayPosition::BottomLeft,
            OverlayPosition::BottomRight,
            OverlayPosition::TopLeft,
            OverlayPosition::TopRight,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    /// Path to ffmpeg binary. Empty string -> search PATH and common locations.
    pub ffmpeg_path: PathBuf,
    /// Master config file path used to reload.
    #[serde(skip)]
    pub config_path: PathBuf,
    pub hotkey_clip: Hotkey,
    pub hotkey_record_start: Hotkey,
    pub hotkey_record_stop: Hotkey,
    pub clip: ClipConfig,
    pub encoding: EncoderConfig,
    pub recording: RecordingConfig,
    pub storage: Storage,
    /// Include audio in recordings (experimental; requires system audio loopback).
    pub capture_audio: bool,
    /// Segment duration in seconds for the rolling buffer (lower = finer clip precision).
    pub segment_seconds: f64,
    /// Which monitor to capture (0 = primary).
    pub monitor_index: usize,
    /// Input capture (mouse/keyboard/controller) and key overlay options.
    #[serde(default)]
    pub input: InputConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingConfig {
    /// When recording via hotkey, split at these intervals (0 = single file).
    pub segment_seconds: u32,
    pub same_encoding_as_clip: bool,
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            segment_seconds: 0,
            same_encoding_as_clip: true,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            ffmpeg_path: PathBuf::new(),
            config_path: PathBuf::new(),
            hotkey_clip: Hotkey::new(0, HotkeyKey::F(8)),
            hotkey_record_start: Hotkey::new(0, HotkeyKey::F(9)),
            hotkey_record_stop: Hotkey::new(0, HotkeyKey::F(10)),
            clip: ClipConfig::default(),
            encoding: EncoderConfig::default(),
            recording: RecordingConfig::default(),
            storage: Storage::default(),
            capture_audio: false,
            segment_seconds: 2.0,
            monitor_index: 0,
            input: InputConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn hotkey_names(&self) -> (String, String, String) {
        (
            self.hotkey_clip.as_str(),
            self.hotkey_record_start.as_str(),
            self.hotkey_record_stop.as_str(),
        )
    }

    pub fn default_config_dir() -> PathBuf {
        let base = dirs::config_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")));
        base.join("rsclipping")
    }

    pub fn default_config_path() -> PathBuf {
        Self::default_config_dir().join("config.json")
    }

    pub fn load() -> anyhow::Result<AppConfig> {
        let path = Self::default_config_path();
        if !path.exists() {
            let cfg = AppConfig::default();
            log::warn!("No config at {} — writing defaults", path.display());
            try_write(&path, &cfg).ok();
            return Ok(cfg);
        }
        let mut cfg: AppConfig = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        cfg.config_path = path;
        Ok(cfg)
    }

    /// Persist to disk (used by daemon-facing tooling).
    #[allow(dead_code)]
    pub fn save(&self) -> anyhow::Result<()> {
        try_write(&self.config_path, self)
    }
}

fn try_write(path: &PathBuf, cfg: &AppConfig) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(cfg)?;
    std::fs::write(path, raw)?;
    Ok(())
}