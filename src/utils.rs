use anyhow::{bail, Context};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Base name for clip files, e.g. `clip_2025-01-01_12-00-00`.
pub fn timestamped_base(prefix: &str, kind: &str) -> String {
    let now = chrono::Local::now().format("%Y-%m-%d_%H%M%S");
    if prefix.is_empty() {
        format!("{}_{}", kind, now)
    } else {
        format!("{}_{}_{}", prefix, kind, now)
    }
}

/// Locate a usable ffmpeg binary.
pub fn find_ffmpeg(configured: &Path) -> anyhow::Result<PathBuf> {
    if !configured.as_os_str().is_empty() {
        if configured.exists() {
            if ffmpeg_works(configured) {
                return Ok(configured.to_path_buf());
            }
            bail!("Configured ffmpeg at '{}' did not run correctly", configured.display());
        }
        bail!("Configured ffmpeg path '{}' does not exist", configured.display());
    }

    if let Ok(cands) = probe_ffmpeg_candidates() {
        return Ok(cands);
    }

    bail!("ffmpeg not found on PATH. Install ffmpeg or set `ffmpeg_path` in the config.")
}

fn probe_ffmpeg_candidates() -> anyhow::Result<PathBuf> {
    // 1. PATH lookup
    if let Ok(output) = Command::new("ffmpeg").arg("-version").output() {
        if output.status.success() {
            if let Ok(which) = which_ffmpeg() {
                return Ok(which);
            }
        }
    }
    // 2. Common install locations
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(pf) = std::env::var("ProgramFiles") {
        candidates.push(PathBuf::from(&pf).join("ffmpeg\\bin\\ffmpeg.exe"));
        candidates.push(PathBuf::from(&pf).join("FFmpeg\\bin\\ffmpeg.exe"));
    }
    if let Ok(pfx86) = std::env::var("ProgramFiles(x86)") {
        candidates.push(PathBuf::from(&pfx86).join("ffmpeg\\bin\\ffmpeg.exe"));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(PathBuf::from(&local).join("ffmpeg\\bin\\ffmpeg.exe"));
        // winget-installed packages (Gyan.FFmpeg etc.)
        let pkg_root = PathBuf::from(&local).join("Microsoft\\WinGet\\Packages");
        if let Ok(entries) = std::fs::read_dir(&pkg_root) {
            for e in entries.flatten() {
                let root = e.path();
                if root.is_dir() {
                    let bin = root.join("bin\\ffmpeg.exe");
                    if bin.exists() {
                        candidates.push(bin);
                    }
                    // nested <pkg>\bin variant
                    if let Ok(sub) = std::fs::read_dir(&root) {
                        for se in sub.flatten() {
                            let b2 = se.path().join("bin\\ffmpeg.exe");
                            if b2.exists() {
                                candidates.push(b2);
                            }
                        }
                    }
                }
            }
        }
    }
    if let Ok(usr) = std::env::var("USERPROFILE") {
        candidates.push(PathBuf::from(&usr).join("scoop\\shims\\ffmpeg.exe"));
        candidates.push(PathBuf::from(&usr).join("winget\\links\\ffmpeg.exe"));
    }
    for c in &candidates {
        if c.exists() && ffmpeg_works(c) {
            return Ok(c.clone());
        }
    }
    bail!("no ffmpeg candidate succeeded")
}

fn which_ffmpeg() -> anyhow::Result<PathBuf> {
    #[cfg(windows)]
    {
        // Use the canonical `where` behaviour through `CommandExt`-free approach:
        // spawn `where ffmpeg` and parse first line.
        if let Ok(out) = Command::new("where").arg("ffmpeg").output() {
            if out.status.success() {
                let first = String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .next()
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                if !first.is_empty() {
                    return Ok(PathBuf::from(first));
                }
            }
        }
    }
    bail!("ffmpeg not resolvable via PATH")
}

fn ffmpeg_works(p: &Path) -> bool {
    Command::new(p)
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Resolve an encoder name for a given codec/backend, preferring GPU when supported.
pub fn pick_encoder(codec: crate::config::Codec, backend: crate::config::EncoderBackend, ffmpeg: &Path) -> anyhow::Result<String> {
    let need_gpu = match backend {
        crate::config::EncoderBackend::Auto => codec.is_gpu_encodable(),
        crate::config::EncoderBackend::Cpu => false,
        crate::config::EncoderBackend::Gpu => true,
    };

    if !need_gpu {
        return Ok(codec.ffmpeg_cpu().to_string());
    }

    // GPU requested (or auto): try each GPU encoder in preference order with a
    // real smoke test so a broken/driverless encoder never breaks the daemon.
    for cand in gpu_candidates(codec).iter() {
        if encoder_available(ffmpeg, cand) {
            log::info!("using GPU encoder: {}", cand);
            return Ok(cand.to_string());
        }
    }
    if matches!(backend, crate::config::EncoderBackend::Gpu) {
        anyhow::bail!(
            "requested GPU encoding for {} but no working GPU encoder found ({}); rerun with --backend auto/cpu",
            codec.as_str(),
            gpu_candidates(codec).join(", ")
        );
    }
    log::warn!("no usable GPU encoder for {}; falling back to CPU ({})", codec.as_str(), codec.ffmpeg_cpu());
    Ok(codec.ffmpeg_cpu().to_string())
}

#[allow(clippy::match_like_matches_macro)]
fn gpu_candidates(codec: crate::config::Codec) -> Vec<&'static str> {
    let mut c = Vec::new();
    if let Some(nv) = codec.ffmpeg_gpu() {
        c.push(nv);
    }
    c.push(codec.ffmpeg_gpu_fallback());
    c
}

/// Encode 0.3s of generated video and check exit status — confirms the encoder
/// is actually usable on this machine (drivers, licensing, build features).
fn encoder_available(ffmpeg: &Path, encoder: &str) -> bool {
    let out = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=black:s=16x16:r=10:d=0.3",
            "-c:v",
            encoder,
            "-t",
            "0.2",
            "-f",
            "null",
            "-",
        ])
        .output();
    out.map(|o| o.status.success()).unwrap_or(false)
}

pub fn ensure_dir(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("failed to create directory {}", dir.display()))
}

/// Modal folder picker. `initial` is the directory the dialog starts in.
/// On non-Windows platforms this always returns `None`.
pub fn pick_folder(initial: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use windows::core::{PCWSTR, PWSTR};
        use windows::Win32::Foundation::{LPARAM, MAX_PATH};
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::Win32::UI::Shell::{BROWSEINFOW, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, SHBrowseForFolderW, SHGetPathFromIDListW};

        let title: Vec<u16> = "Select an output folder".encode_utf16().chain(std::iter::once(0)).collect();
        let bi = BROWSEINFOW {
            hwndOwner: Default::default(),
            pidlRoot: std::ptr::null_mut(),
            pszDisplayName: PWSTR::null(),
            lpszTitle: PCWSTR(title.as_ptr()),
            ulFlags: BIF_NEWDIALOGSTYLE | BIF_RETURNONLYFSDIRS,
            lpfn: None,
            lParam: LPARAM::default(),
            iImage: 0,
        };

        let _ = initial;

        let pidl = unsafe { SHBrowseForFolderW(&bi) };
        if pidl.is_null() {
            return None;
        }
        let mut buf = [0u16; MAX_PATH as usize];
        let ok = unsafe { SHGetPathFromIDListW(pidl, &mut buf) };
        unsafe { CoTaskMemFree(Some(pidl as *const _)) };
        if ok.as_bool() {
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            let s = String::from_utf16_lossy(&buf[..end]);
            if !s.is_empty() {
                return Some(PathBuf::from(s));
            }
        }
        None
    }
    #[cfg(not(windows))]
    {
        let _ = initial;
        None
    }
}