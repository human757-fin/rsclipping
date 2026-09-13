//! Clip/recording storage: destination naming and retention enforcement.

use crate::config::Storage;
use crate::utils;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Produce a unique, timestamped destination path in `dir`.
pub fn destination(dir: &Path, kind: &str, extension: &str) -> Result<PathBuf> {
    utils::ensure_dir(dir)?;
    let base = utils::timestamped_base("", kind);
    let mut candidate = dir.join(format!("{}.{}", base, extension));
    let mut i = 1u32;
    while candidate.exists() {
        candidate = dir.join(format!("{}_{}.{}", base, i, extension));
        i += 1;
    }
    Ok(candidate)
}

/// Remove oldest outputs beyond configured clip count / storage cap.
pub fn enforce_retention(cfg: &Storage) -> Result<()> {
    if !cfg.auto_cleanup {
        return Ok(());
    }
    let max_bytes = if cfg.max_storage_mb > 0 {
        Some(cfg.max_storage_mb.saturating_mul(1024 * 1024))
    } else {
        None
    };

    // Collect all outputs (clip + record dirs) ordered oldest→newest.
    let mut files: Vec<(PathBuf, u64, u128)> = Vec::new(); // (path, size, mtime_ms)
    for dir in [&cfg.clip_dir, &cfg.record_dir] {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let path = e.path();
                if !path.is_file() {
                    continue;
                }
                if let Ok(md) = path.metadata() {
                    let size = md.len();
                    let mt = md
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_millis())
                        .unwrap_or(0);
                    files.push((path, size, mt));
                }
            }
        }
    }
    files.sort_by_key(|f| f.2); // oldest first

    let mut total_bytes: u64 = files.iter().map(|f| f.1).sum();

    let mut removed = 0usize;
    let mut i = 0usize;
    if cfg.max_clips > 0 {
        let clip_cap = cfg.max_clips;
        // Count only clip_dir files toward the clip cap.
        let clip_count = files.iter().filter(|f| f.0.starts_with(&cfg.clip_dir)).count();
        if clip_count > clip_cap {
            let must_remove = clip_count - clip_cap;
            let mut done = 0usize;
            while done < must_remove && i < files.len() {
                let (p, size, _) = &files[i];
                if p.starts_with(&cfg.clip_dir) {
                    if std::fs::remove_file(p).is_ok() {
                        total_bytes = total_bytes.saturating_sub(*size);
                        removed += 1;
                        done += 1;
                    }
                }
                i += 1;
            }
        }
    }

    if let Some(max_bytes) = max_bytes {
        while i < files.len() && total_bytes > max_bytes {
            let (p, size, _) = &files[i];
            if std::fs::remove_file(p).is_ok() {
                total_bytes = total_bytes.saturating_sub(*size);
                removed += 1;
            }
            i += 1;
        }
    }

    if removed > 0 {
        log::info!("storage cleanup removed {} file(s)", removed);
    }
    Ok(())
}

/// Wall-clock formatted size string (kept for status output).
#[allow(dead_code)]
pub fn human_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    }
}