# RSClipping

A screen clipper + rolling recorder written in Rust. Minimal RAM, max throughput.

Records a rolling video window into tiny disk segments and turns them into clips
and recordings with an **instant copy — never a re-encode**.

## Features

- **Background daemon** — runs headless with global hotkeys, works even when the
  window is closed
- **Greenscreen-style past clip** — `clip` hotkey saves the last N seconds instantly
- **On-demand recording** — start/stop hotkeys produce a full-length file
- **Instant remux** — segments are muxed together, no CPU-heavy re-encoding
- **DXGI desktop capture** (Windows) — zero per-frame allocations
- **X11 capture** (Linux / XWayland) — x11rb with full modifier-map hotkey support
- **Polished egui dashboard** — live FPS stats, one-click actions, settings editor
- **Rebindable hotkeys** — point-and-click in the GUI
- **Monitor selection** — pick which display to capture, with resolutions listed
- **Dark-themed Inno Setup installer** (Windows) + portable zip + Linux AppImage

## Quick start

```sh
# Open the dashboard (recommended)
rsclipping gui

# Run headless with global hotkeys
rsclipping daemon

# One-shot clip (grabs the next 10 seconds)
rsclipping clip 10
```

The app needs `ffmpeg` on your `PATH` (it is resolved automatically on first run).

## Releases

Prebuilt installers and portable builds are published on the
[Releases](../../../releases) page:

| Artifact | Platform | Description |
| --- | --- | --- |
| `rsclipping-setup-<ver>.exe` | Windows | Full installer with dark-themed wizard |
| `rsclipping-<ver>-windows-x64.zip` | Windows | Portable, no install needed |
| `rsclipping-<ver>-x86_64.AppImage` | Linux | Portable AppImage (X11/XWayland) |

### Online installer (no download required)

**Windows** — runs PowerShell to fetch and launch the latest setup automatically:

```powershell
powershell -ExecutionPolicy Bypass -File install.ps1
```

**Linux** — downloads the latest AppImage to the current directory:

```bash
bash install.sh
```

## Config

Config lives at `%APPDATA%\rsclipping\config.json` (Windows) or
`~/.config/rsclipping/config.json` (Linux). The GUI edits it for you.

Useful keys: `hotkey_clip`, `hotkey_record_start`, `hotkey_record_stop`,
`codec`, `fps`, `clip_dir`, `record_dir`, `monitor_index`.

## Building

```sh
cargo build --release
```

Requires Rust edition 2021 and FFmpeg on PATH (for testing output).

- **Windows**: DXGI capture via `windows` crate — zero per-frame allocations.
- **Linux**: X11 capture via `x11rb` with MIT-SHM fast path and full modifier-map
  hotkey support (works under XWayland on Wayland composites).

### Platform matrix

| Platform | Capture | Hotkeys | Packaging |
| --- | --- | --- | --- |
| Windows x64 | DXGI Desktop Duplication | `RegisterHotKey` + hidden window | Installer (.iss) + portable zip |
| Linux x86_64 | X11 (XRandR + `XGetImage`) | `XGrabKey` (root window) | AppImage |

## License

MIT