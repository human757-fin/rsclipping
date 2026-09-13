#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
mod capture;
mod config;
mod encoder;
mod gui;
mod hotkeys;
mod input;
mod storage;
mod utils;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::{Codec, EncoderBackend, Hotkey};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "rsclipping",
    version,
    about = "Screen clipper/recorder in Rust — minimal RAM, max throughput",
    long_about = "Records a rolling video window into tiny disk segments and turns\n\
        them into clips/recordings with an instant copy, never a re-encode."
)]
struct Cli {
    /// Path to config file (defaults to %APPDATA%/rsclipping/config.json).
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// ffmpeg binary path (overrides config).
    #[arg(long, global = true)]
    ffmpeg: Option<PathBuf>,

    /// Codec: h264, h265, vp9, av1 (overrides config).
    #[arg(long, global = true)]
    codec: Option<String>,

    /// Encoder backend: auto, cpu, gpu (overrides config).
    #[arg(long, global = true)]
    backend: Option<String>,

    /// Capture fps (overrides config).
    #[arg(long, global = true)]
    fps: Option<u32>,

    /// Quality: lower = better (CRF/CQ, overrides config).
    #[arg(long, global = true)]
    crf: Option<u32>,

    /// Bitrate in kbps (0 = CRF only).
    #[arg(long, global = true)]
    bitrate: Option<u32>,

    /// Encoder preset (e.g. veryfast/p2).
    #[arg(long, global = true)]
    preset: Option<String>,

    /// Rolling history depth in seconds for clips.
    #[arg(long, global = true)]
    history: Option<u32>,

    /// Default clip length in seconds.
    #[arg(long, global = true)]
    clip_length: Option<u32>,

    /// Clip output directory.
    #[arg(long, global = true)]
    clip_dir: Option<PathBuf>,

    /// Recording output directory.
    #[arg(long, global = true)]
    record_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Open the desktop dashboard (live status, actions and settings).
    Gui,
    /// Run as daemon with global hotkeys (headless).
    Daemon,
    /// Capture a one-shot clip (captures the next N seconds, then saves).
    Clip {
        #[arg(default_value_t = 10)]
        seconds: u32,
    },
    /// Capture a one-shot recording for N seconds, then save.
    Record {
        #[arg(default_value_t = 30)]
        seconds: u32,
    },
    /// Point a keyboard button for a single action, then exit.
    /// Example: `rsclipping bind F11 --action clip`.
    Bind {
        key: String,
        #[arg(long)]
        action: String,
    },
    /// Print the effective config as JSON.
    ShowConfig,
    /// List available codecs.
    ListCodecs,
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .init();

    let cli = Cli::parse();
    let mut cfg = config::AppConfig::load()?;

    // apply CLI overrides
    if let Some(p) = cli.config {
        if p.exists() {
            cfg = serde_json::from_str(&std::fs::read_to_string(&p)?)?;
            cfg.config_path = p;
        } else {
            log::warn!("config file {} not found", p.display());
        }
    }
    if let Some(p) = cli.ffmpeg {
        cfg.ffmpeg_path = p;
    }
    if let Some(c) = cli.codec {
        let c = Codec::from_name(&c)
            .ok_or_else(|| anyhow::anyhow!("unknown codec '{}' (h264|h265|vp9|av1)", c))?;
        cfg.encoding.codec = c;
    }
    if let Some(b) = cli.backend {
        cfg.encoding.backend = match b.to_lowercase().as_str() {
            "auto" => EncoderBackend::Auto,
            "cpu" => EncoderBackend::Cpu,
            "gpu" => EncoderBackend::Gpu,
            _ => anyhow::bail!("unknown backend '{}' (auto|cpu|gpu)", b),
        };
    }
    if let Some(f) = cli.fps {
        cfg.encoding.fps = f;
    }
    if let Some(c) = cli.crf {
        cfg.encoding.crf = c;
    }
    if let Some(b) = cli.bitrate {
        cfg.encoding.bitrate_kbps = b;
    }
    if let Some(p) = cli.preset {
        cfg.encoding.preset = p;
    }
    if let Some(h) = cli.history {
        cfg.clip.history_seconds = h;
    }
    if let Some(l) = cli.clip_length {
        cfg.clip.default_length_seconds = l;
    }
    if let Some(d) = cli.clip_dir {
        cfg.storage.clip_dir = d;
    }
    if let Some(d) = cli.record_dir {
        cfg.storage.record_dir = d;
    }

    // Resolve ffmpeg early so all modes benefit from a single failure point.
    let best_ffmpeg = utils::find_ffmpeg(&cfg.ffmpeg_path)?;
    if cfg.ffmpeg_path.as_os_str().is_empty() {
        cfg.ffmpeg_path = best_ffmpeg.clone();
        log::info!("using ffmpeg at {}", best_ffmpeg.display());
    }
    // Note: CLI overrides intentionally NOT persisted here — one-shot flags
    // only affect the current run; the saved config is only ever written by
    // `AppConfig::load` when no config file exists (clean defaults).

    // Double-clicking the app (or launching from a Start Menu shortcut)
    // opens the dashboard; headless modes must be requested explicitly.
    let cmd = cli.command.unwrap_or(Commands::Gui);
    match cmd {
        Commands::Gui => gui::run(cfg),
        Commands::Daemon => app::run_daemon(cfg),
        Commands::Clip { seconds } => app::run_one_shot(&cfg, app::OneShotKind::Clip, seconds),
        Commands::Record { seconds } => app::run_one_shot(&cfg, app::OneShotKind::Record, seconds),
        Commands::Bind { key, action } => {
            let key = Hotkey::from_str(&key)
                .ok_or_else(|| anyhow::anyhow!("unsupported key '{}'", key))?;
            let action = match action.to_lowercase().as_str() {
                "clip" => config::Action::Clip,
                "record" => config::Action::RecordStart,
                "stop" => config::Action::RecordStop,
                _ => anyhow::bail!("unsupported action '{}' (clip|record|stop)", action),
            };
            let (tx, rx) = std::sync::mpsc::channel::<config::Action>();
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let t = std::thread::spawn(move || {
                let _ = hotkeys::run(stop, tx, key, key, key);
            });
            match rx.recv() {
                Ok(a) => println!("pressed -> {:?}", a),
                Err(_) => println!("failed to receive hotkey event"),
            }
            let _ = action;
            t.join().ok();
            Ok(())
        }
        Commands::ShowConfig => {
            println!("{}", serde_json::to_string_pretty(&cfg)?);
            Ok(())
        }
        Commands::ListCodecs => {
            for c in Codec::all() {
                println!("{}  (ffmpeg {} / extension .{})", c.as_str(), c.ffmpeg_cpu(), c.extension());
            }
            Ok(())
        }
    }
}