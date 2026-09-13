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
- **Polished egui dashboard** — live FPS stats, one-click actions, settings editor
- **Rebindable hotkeys** — point-and-click in the GUI
- **Monitor selection** — pick which display to capture, with resolutions listed

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

## Config

Config lives at `%APPDATA%\rsclipping\config.json`. The GUI edits it for you.
Check the effective config with `rsclipping show-config`.

## Building

```sh
cargo build --release
```

Requires Rust edition 2021. On Windows the DXGI capture backend is used; on Linux
an X11/Wayland (PipeWire) backend is used where available.

## Releases

Prebuilt installers and portable builds are published on the
[Releases](../../../releases) page:

| Artifact | Platform | Description |
| --- | --- | --- |
| `RSClipping-Setup-<ver>.exe` | Windows | Full installer with dark-themed GUI |
| `RSClipping-<ver>-win-portable.zip` | Windows | Portable, no install |
| `RSClipping-<ver>-x86_64.AppImage` | Linux | Portable AppImage |
| `RSClipping-Online-Setup-<ver>.exe` | Windows | Small bootstrap; downloads the latest release |

## License

MIT