# Generates RSClipping branding assets (no external deps, System.Drawing only):
#   rsclipping.ico  - multi-size Windows icon (16..256) with BMP DIB entries
#   rsclipping.png  - 256x256 source image (used for AppImage, egui window icon)
#
# Palette matches the GUI: BG #101218, border #364052, accent #5b8cff.
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$dir = Split-Path -Parent $MyInvocation.MyCommand.Path
$bg    = [System.Drawing.Color]::FromArgb(255, 16, 18, 24)      # #101218
$bdr   = [System.Drawing.Color]::FromArgb(255, 54, 64, 82)      # #364052
$accent= [System.Drawing.Color]::FromArgb(255, 91, 140, 255)    # #5b8cff

function New-RscIconBitmap([int]$size) {
    $ss = 4  # supersampling factor for crisp downscale
    $big = $size * $ss
    $bmp = New-Object System.Drawing.Bitmap($big, $big, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
    $g.Clear([System.Drawing.Color]::Transparent)

    $rad = [int]($big * 0.20)
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $r = New-Object System.Drawing.Rectangle -ArgumentList @(1, 1, ($big - 2), ($big - 2))
    $d = $rad * 2
    $path.AddArc($r.X, $r.Y, $d, $d, 180, 90)
    $path.AddArc($r.Right - $d, $r.Y, $d, $d, 270, 90)
    $path.AddArc($r.Right - $d, $r.Bottom - $d, $d, $d, 0, 90)
    $path.AddArc($r.X, $r.Bottom - $d, $d, $d, 90, 90)
    $path.CloseFigure()

    $fill = New-Object System.Drawing.SolidBrush($bg)
    $g.FillPath($fill, $path)
    $pen = New-Object System.Drawing.Pen($bdr, [Math]::Max(1, $big * 0.012))
    $g.DrawPath($pen, $path)

    # Accent "scissor" chevron above a bold RS lockup
    $font = New-Object System.Drawing.Font("Segoe UI", [single]($big * 0.30), [System.Drawing.FontStyle]::Bold)
    $brushFont = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::White)
    $brushAccent = New-Object System.Drawing.SolidBrush($accent)

    # "RS" centered, white
    $sf = New-Object System.Drawing.StringFormat
    $sf.Alignment = [System.Drawing.StringAlignment]::Center
    $sf.LineAlignment = [System.Drawing.StringAlignment]::Center
    $rectText = New-Object System.Drawing.RectangleF -ArgumentList @(0, 0, $big, $big)
    $g.DrawString("RS", $font, $brushFont, $rectText, $sf)

    # Accent underline bar
    $bar = New-Object System.Drawing.SolidBrush($accent)
    $barH = [Math]::Max(2, $big * 0.035)
    $barW = [Math]::Max(6, $big * 0.28)
    $barRect = New-Object System.Drawing.RectangleF -ArgumentList @((($big - $barW) / 2), ($big * 0.60), $barW, $barH)
    $g.FillRectangle($bar, $barRect)

    $g.Dispose()
    # Downscale to target
    $final = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g2 = [System.Drawing.Graphics]::FromImage($final)
    $g2.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $g2.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g2.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g2.Clear([System.Drawing.Color]::Transparent)
    $g2.DrawImage($bmp, 0, 0, $size, $size)
    $g2.Dispose()
    $bmp.Dispose()
    $path.Dispose(); $fill.Dispose(); $pen.Dispose(); $font.Dispose()
    $brushFont.Dispose(); $brushAccent.Dispose(); $bar.Dispose(); $sf.Dispose()
    return $final
}

# --- PNG 256 -------------------------------------------------------------
$pngPath = Join-Path $dir "rsclipping.png"
$bmp256 = New-RscIconBitmap 256
$bmp256.Save($pngPath, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp256.Dispose()
Write-Host "wrote $pngPath"

# --- ICO (BMP DIB entries, bottom-up BGRA + empty AND mask) --------------
$sizes = @(16, 24, 32, 48, 64, 128, 256)
$iof = [System.IO.Path]::ChangeExtension($pngPath, ".ico")
$fs = New-Object System.IO.FileStream($iof, [System.IO.FileMode]::Create)
$bw = New-Object System.IO.BinaryWriter($fs)
$bw.Write([uint16]0); $bw.Write([uint16]1); $bw.Write([uint16]$sizes.Count)

$offsets = @()
$bodies = @()
$offset = 6 + 16 * $sizes.Count
foreach ($s in $sizes) {
    $offsets += $offset
    $bmp = New-RscIconBitmap $s
    $bmpBytes = New-Object byte[] ($s * $s * 4)
    $rect = New-Object System.Drawing.Rectangle -ArgumentList @(0, 0, $s, $s)
    $bd = $bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    [System.Runtime.InteropServices.Marshal]::Copy($bd.Scan0, $bmpBytes, 0, $bmpBytes.Length)
    $bmp.UnlockBits($bd)
    $bmp.Dispose()

    $andStride = [int]((($s + 31) / 32) * 4)
    $andSize = $andStride * $s
    $bmSize = $s * $s * 4
    $resSize = 40 + $bmSize + $andSize

    $ms = New-Object System.IO.MemoryStream
    $mw = New-Object System.IO.BinaryWriter($ms)
    $mw.Write([int32]40)              # biSize
    $mw.Write([int32]$s)              # biWidth
    $mw.Write([int32]($s * 2))        # biHeight (XOR + AND)
    $mw.Write([uint16]1)              # biPlanes
    $mw.Write([uint16]32)             # biBitCount
    $mw.Write([int32]0)               # biCompression BI_RGB
    $mw.Write([int32]$bmSize)         # biSizeImage
    $mw.Write([int32]0); $mw.Write([int32]0); $mw.Write([int32]0); $mw.Write([int32]0)
    # BGRA rows, bottom-up
    for ($row = $s - 1; $row -ge 0; $row--) {
        $start = $row * $s * 4
        $mw.Write($bmpBytes, $start, $s * 4)
    }
    # AND mask (all zeros: opaque)
    $mw.Write((New-Object byte[] $andSize), 0, $andSize)
    $mw.Flush()
    $bodies += ,$ms.ToArray()
    $mw.Dispose(); $ms.Dispose()

    $offset += $resSize
}

for ($i = 0; $i -lt $sizes.Count; $i++) {
    $s = $sizes[$i]
    $body = $bodies[$i]
    $dim = if ($s -eq 256) { 0 } else { $s }
    $bw.Write([byte]$dim)
    $bw.Write([byte]$dim)
    $bw.Write([byte]0); $bw.Write([byte]0)
    $bw.Write([uint16]1); $bw.Write([uint16]32)
    $bw.Write([int32]$body.Length)
    $bw.Write([int32]$offsets[$i])
}
foreach ($body in $bodies) { $bw.Write($body) }
$bw.Flush(); $bw.Close()
Write-Host "wrote $iof ($([math]::Round((Get-Item $iof).Length/1kb,1)) KB)"

# --- Inno Setup wizard images (dark) --------------------------------------
function New-Solid([int]$w, [int]$h, [System.Drawing.Color]$c) {
    $b = New-Object System.Drawing.Bitmap($w, $h, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($b)
    $g.Clear($c); $g.Dispose()
    return $b
}

function Draw-WizardLogo([int]$w, [int]$h, [string]$out) {
    $b = New-Solid $w $h $bg
    $g = [System.Drawing.Graphics]::FromImage($b)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    # Top accent line
    $line = New-Object System.Drawing.SolidBrush($accent)
    $g.FillRectangle($line, 0, 0, $w, 6)
    # Centered icon-glow square + "RS"
    $sq = [Math]::Min($w - 40, [int]($h * 0.45))
    $x = ($w - $sq) / 2
    $y = ($h - $sq) / 2
    $fill = New-Object System.Drawing.SolidBrush($bdr)
    $g.FillRectangle($fill, $x, $y, $sq, $sq)
    $fs = [single]($sq * 0.55)
    $font = New-Object System.Drawing.Font("Segoe UI", $fs, [System.Drawing.FontStyle]::Bold, [System.Drawing.GraphicsUnit]::Pixel)
    $white = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::White)
    $sf = New-Object System.Drawing.StringFormat
    $sf.Alignment = [System.Drawing.StringAlignment]::Center
    $sf.LineAlignment = [System.Drawing.StringAlignment]::Center
    $rect = New-Object System.Drawing.RectangleF -ArgumentList @($x, $y, $sq, $sq)
    $g.DrawString("RS", $font, $white, $rect, $sf)
    $g.Dispose()
    $b.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
    $b.Dispose()
    Write-Host "wrote $out"
}

$wizardBig  = Join-Path $dir "wizard-big.png"    # 164x314 (WizardImageFile)
$wizardSmall = Join-Path $dir "wizard-small.png"  # 55x58 (WizardSmallImageFile)
Draw-WizardLogo 164 314 $wizardBig
Draw-WizardLogo 55 58 $wizardSmall