# Generates assets/typors.ico: an original placeholder mark for the typors app.
#
# Design is ours (geometric lower-case "t" on a rounded navy plate); it is not
# derived from any other product's icon, palette or typeface. Replace the file
# and re-run `cargo build` to ship a different mark.
#
#   pwsh -File tools/make-icon.ps1
#   pwsh -File tools/make-icon.ps1 -PreviewPath $env:TEMP\icon.png   # eyeball the mark
#
# Entries: 16/32/48/256 as 32-bit BMP (DIB), which every Windows icon path reads.

param(
    [string]$PreviewPath = ''
)

Add-Type -AssemblyName System.Drawing

$ErrorActionPreference = 'Stop'
$out = Join-Path $PSScriptRoot '..\assets\typors.ico'
$out = [System.IO.Path]::GetFullPath($out)

# --- master artwork, drawn large and downscaled -------------------------------

$S = 1024
$master = New-Object System.Drawing.Bitmap($S, $S, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$g = [System.Drawing.Graphics]::FromImage($master)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
$g.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
$g.Clear([System.Drawing.Color]::Transparent)

function New-RoundedPath([single]$x, [single]$y, [single]$w, [single]$h, [single]$r) {
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = $r * 2
    $path.AddArc($x, $y, $d, $d, 180, 90)
    $path.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $path.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $path.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $path.CloseFigure()
    return $path
}

# Plate: 3% bleed, 21% corner radius, deep navy vertical gradient.
$inset = $S * 0.03
$plateSize = $S - 2 * $inset
$plate = New-RoundedPath $inset $inset $plateSize $plateSize ($plateSize * 0.21)
$plateRect = New-Object System.Drawing.RectangleF($inset, $inset, $plateSize, $plateSize)
$brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
    $plateRect,
    [System.Drawing.Color]::FromArgb(255, 38, 54, 96),   # #263660
    [System.Drawing.Color]::FromArgb(255, 15, 20, 38),   # #0F1426
    90.0)
$g.FillPath($brush, $plate)
$brush.Dispose()

# Glyph: geometric lower-case "t" - stem with an ascender, plus the crossbar.
$glyph = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 233, 241, 255))
$mint = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 69, 209, 178))

$stemW = $S * 0.135
$stemX = ($S - $stemW) / 2
$stemTop = $S * 0.205
$stemBottom = $S * 0.795
$stem = New-RoundedPath $stemX $stemTop $stemW ($stemBottom - $stemTop) ($stemW * 0.34)
$g.FillPath($glyph, $stem)

$barW = $S * 0.52
$barH = $S * 0.125
$bar = New-RoundedPath (($S - $barW) / 2) ($S * 0.292) $barW $barH ($barH * 0.45)
$g.FillPath($glyph, $bar)

# The caret: a mint block sitting under the baseline, the "you are here" mark.
$caretW = $S * 0.30
$caretH = $S * 0.078
$caret = New-RoundedPath (($S - $caretW) / 2) ($S * 0.845) $caretW $caretH ($caretH * 0.45)
$g.FillPath($mint, $caret)

$glyph.Dispose()
$mint.Dispose()
$g.Dispose()

if ($PreviewPath) {
    $master.Save([System.IO.Path]::GetFullPath($PreviewPath), [System.Drawing.Imaging.ImageFormat]::Png)
    "preview: $([System.IO.Path]::GetFullPath($PreviewPath))"
}

# --- per-size rasters --------------------------------------------------------

function Get-Scaled([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $gg = [System.Drawing.Graphics]::FromImage($bmp)
    $gg.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
    $gg.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
    $gg.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $gg.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $gg.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $gg.DrawImage($master, (New-Object System.Drawing.Rectangle(0, 0, $size, $size)))
    $gg.Dispose()
    return $bmp
}

function Get-PngBytes($bmp) {
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    return $ms.ToArray()
}

# 32-bit bottom-up DIB plus the 1-bit AND mask (set where alpha is 0).
function Get-DibBytes($bmp) {
    $w = $bmp.Width
    $h = $bmp.Height
    $ms = New-Object System.IO.MemoryStream
    $bw = New-Object System.IO.BinaryWriter($ms)
    $bw.Write([Int32]40)
    $bw.Write([Int32]$w)
    $bw.Write([Int32]($h * 2))
    $bw.Write([Int16]1)
    $bw.Write([Int16]32)
    $bw.Write([Int32]0)
    $bw.Write([Int32]($w * $h * 4))
    $bw.Write([Int32]0)
    $bw.Write([Int32]0)
    $bw.Write([Int32]0)
    $bw.Write([Int32]0)

    for ($y = $h - 1; $y -ge 0; $y--) {
        for ($x = 0; $x -lt $w; $x++) {
            $c = $bmp.GetPixel($x, $y)
            $bw.Write([byte]$c.B)
            $bw.Write([byte]$c.G)
            $bw.Write([byte]$c.R)
            $bw.Write([byte]$c.A)
        }
    }

    $maskStride = [int]([Math]::Floor(($w + 31) / 32) * 4)
    for ($y = $h - 1; $y -ge 0; $y--) {
        $row = New-Object byte[] $maskStride
        for ($x = 0; $x -lt $w; $x++) {
            if ($bmp.GetPixel($x, $y).A -eq 0) {
                $row[[int][Math]::Floor($x / 8)] = $row[[int][Math]::Floor($x / 8)] -bor (0x80 -shr ($x % 8))
            }
        }
        $bw.Write($row, 0, $maskStride)
    }

    $bw.Flush()
    return $ms.ToArray()
}

$entries = @()
foreach ($size in @(16, 32, 48, 256)) {
    $bmp = Get-Scaled $size
    $entries += [pscustomobject]@{ Size = $size; Bytes = (Get-DibBytes $bmp) }
    $bmp.Dispose()
}
$master.Dispose()

# --- ICO container -----------------------------------------------------------

$ms = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter($ms)
$bw.Write([Int16]0)                # reserved
$bw.Write([Int16]1)                # type: icon
$bw.Write([Int16]$entries.Count)

$offset = 6 + 16 * $entries.Count
foreach ($e in $entries) {
    $dim = if ($e.Size -ge 256) { 0 } else { $e.Size }
    $bw.Write([byte]$dim)          # width (0 means 256)
    $bw.Write([byte]$dim)          # height
    $bw.Write([byte]0)             # palette colours
    $bw.Write([byte]0)             # reserved
    $bw.Write([Int16]1)            # planes
    $bw.Write([Int16]32)           # bits per pixel
    $bw.Write([Int32]$e.Bytes.Length)
    $bw.Write([Int32]$offset)
    $offset += $e.Bytes.Length
}
foreach ($e in $entries) { $bw.Write($e.Bytes, 0, $e.Bytes.Length) }
$bw.Flush()
[System.IO.File]::WriteAllBytes($out, $ms.ToArray())
$bw.Dispose()
$ms.Dispose()

$info = Get-Item $out
"wrote $($info.FullName): $($info.Length) bytes, sizes $(($entries | ForEach-Object { $_.Size }) -join '/')"
