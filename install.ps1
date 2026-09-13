# RSClipping online installer (Windows) - fetches the latest release from GitHub
# and runs the official setup. Usage:
#   powershell -ExecutionPolicy Bypass -File install.ps1
$ErrorActionPreference = "Stop"
$Repo   = "human757-fin/rsclipping"
$Api    = "https://api.github.com/repos/$Repo/releases/latest"
$Asset  = "rsclipping-setup-*.exe"

Write-Host "RSClipping installer - checking for the latest release..." -ForegroundColor Cyan
$headers = @{ "User-Agent" = "rsclipping-online-installer" }
$json = Invoke-RestMethod -Uri $Api -Headers $headers
$version = $json.tag_name.TrimStart('v')
Write-Host "Latest version: v$version" -ForegroundColor Cyan

$file = $json.assets | Where-Object { $_.name -like $Asset } | Select-Object -First 1
if (-not $file) {
    Write-Error "No Windows installer asset found in the latest release."
}

$target = Join-Path $env:TEMP $file.name
Write-Host "Downloading $($file.name) ..." -ForegroundColor Cyan
Invoke-WebRequest -Uri $file.browser_download_url -OutFile $target -Headers $headers
$size = [math]::Round((Get-Item $target).Length / 1MB, 1)
Write-Host "Downloaded ($size MB). Launching installer..." -ForegroundColor Green

Start-Process -FilePath $target -Wait
Remove-Item -Force $target
Write-Host "Done." -ForegroundColor Green