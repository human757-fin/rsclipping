# Builds the Windows portable zip:
#   release/rsclipping-<VER>-windows-x64.zip
#
# Layout inside the zip:
#   rsclipping.exe
#   README.txt
#
# Requires: cargo (Rust toolchain), PowerShell 5.1+.
param(
    [string]$OutDir = (Join-Path (Split-Path -Parent $PSScriptRoot) "release")
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# Rebuild the release binary
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

# Version from Cargo.toml
$verLine = (Get-Content "Cargo.toml" | Where-Object { $_ -match '^version\s*=\s*"([^"]+)"' } | Select-Object -First 1)
if (-not $verLine) { throw "could not parse version from Cargo.toml" }
$ver = [regex]::Match($verLine, '"([^"]+)"').Groups[1].Value

# Stage
$stage = Join-Path $env:TEMP "rsclipping-portable-$ver"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Path $stage | Out-Null
Copy-Item "target\release\rsclipping.exe" $stage

$readme = @"
RSClipping $ver - portable build (Windows x64)
=============================================

Run rsclipping.exe to open the dashboard. It auto-starts the capture
daemon with global hotkeys (F8 clip / F9 record / F10 stop by default).

Requirements
------------
- FFmpeg on PATH, OR set `ffmpeg_path` in the config below.

Configuration
-------------
Settings are stored in %APPDATA%\rsclipping\config.json and are created
with safe defaults on first run. Edit that file to change hotkeys, codec,
output folders, etc.

Outputs
-------
Clips   -> Desktop\RSClipping
Recordings -> Desktop\RSClippingRecordings

Headless
--------
rsclipping.exe daemon                 # run hotkey daemon without the GUI
rsclipping.exe gui                    # open the dashboard
rsclipping.exe clip 15                # one-shot 15s clip
rsclipping.exe record 60              # one-shot 60s recording
"@
Set-Content -LiteralPath (Join-Path $stage "README.txt") -Value $readme -Encoding UTF8

# Zip
$zipName = "rsclipping-$ver-windows-x64.zip"
$zipPath = Join-Path $OutDir $zipName
if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Path $OutDir | Out-Null }
if (Test-Path $zipPath) { Remove-Item -Force $zipPath }

# Rename staging dir to strip the "-portable" tag and match the archive name
$finalStage = Join-Path (Split-Path -Parent $stage) ("rsclipping-$ver-windows-x64")
if (Test-Path $finalStage) { Remove-Item -Recurse -Force $finalStage }
Rename-Item $stage $finalStage
$stage = $finalStage

Compress-Archive -Path $stage -DestinationPath $zipPath -CompressionLevel Optimal

Remove-Item -Recurse -Force $stage
Write-Host "Wrote $zipPath"
Write-Host "Ready: $zipPath  ($([math]::Round((Get-Item $zipPath).Length/1MB,1)) MB)"