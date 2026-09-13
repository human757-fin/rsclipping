RSClipping - screen clipper / recorder

RSClipping records a rolling video window into tiny disk segments and turns
them into clips/recordings with an instant copy (never a re-encode). The
desktop dashboard gives live status, actions and settings.

Fast start
----------
  1. Launch RSClipping (desktop shortcut or the rsclipping.exe in this folder).
  2. Keep it running - it captures silently with global hotkeys:
        F8          save the last N seconds as a clip
        F9          start recording
        F10         stop recording / save
  3. Find your files:
        Clips       ->  Desktop\RSClipping
        Recordings  ->  Desktop\RSClippingRecordings

Requirements
------------
- FFmpeg on PATH, or set `ffmpeg_path` under %APPDATA%\rsclipping\config.json.
- A GPU encoder (NVENC/AMF/QSV) is used when available; CPU x264 works always.

Configuration
-------------
Config lives in %APPDATA%\rsclipping\config.json (created on first run).
Useful keys: hotkey_clip, hotkey_record_start, hotkey_record_stop, codec,
fps, clip_dir, record_dir, monitor_index.

Command line
------------
  rsclipping.exe gui        open dashboard
  rsclipping.exe daemon     headless daemon with global hotkeys
  rsclipping.exe clip 15    capture a 15s one-shot clip
  rsclipping.exe record 60  capture a 60s recording

Source: https://github.com/human757-fin/rsclipping