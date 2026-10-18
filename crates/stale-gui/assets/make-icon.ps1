# Generates the Stale Uninstaller icon: crates\stale-gui\stale.ico (16, 32, 48 and 256 px, PNG
# entries), the window icon crates\stale-gui\assets\icon-256.png, and previews
# (preview-16.png, preview-32.png, preview-48.png, preview-256.png) next to this script.
#
# Design: a rounded dark-slate square (#2B3440) with a light trash bin and an amber clock badge
# (#E0A030) in the lower right: "stale" plus "uninstall".
#
# Run from any folder: powershell -ExecutionPolicy Bypass -File crates\stale-gui\assets\make-icon.ps1
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$assets = $PSScriptRoot
$crate = Split-Path -Parent $assets

$slate = [System.Drawing.Color]::FromArgb(255, 0x2B, 0x34, 0x40)
$light = [System.Drawing.Color]::FromArgb(255, 0xF2, 0xF4, 0xF7)
$amber = [System.Drawing.Color]::FromArgb(255, 0xE0, 0xA0, 0x30)

function New-RoundedRect([single]$x, [single]$y, [single]$w, [single]$h, [single]$r) {
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = 2 * $r
    $p.AddArc($x, $y, $d, $d, 180, 90)
    $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    return $p
}

# Draws the mark on a $size x $size bitmap. Everything is laid out on a 256-unit grid; small
# sizes use thicker strokes and fewer details so the mark stays legible at 16 px.
function New-IconBitmap([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)
    $g.ScaleTransform($size / 256.0, $size / 256.0)
    $small = $size -le 32

    $slateBrush = New-Object System.Drawing.SolidBrush($slate)
    $lightBrush = New-Object System.Drawing.SolidBrush($light)
    $amberBrush = New-Object System.Drawing.SolidBrush($amber)

    # Background: rounded dark-slate square.
    $bg = New-RoundedRect 0 0 256 256 48
    $g.FillPath($slateBrush, $bg)

    # Trash bin, shifted left/up to make room for the badge.
    if ($small) {
        # Handle, lid and a plain tapered body: few, thick shapes.
        $g.FillPath($lightBrush, (New-RoundedRect 96 30 64 30 10))
        $g.FillPath($slateBrush, (New-RoundedRect 114 46 28 20 4))
        $g.FillPath($lightBrush, (New-RoundedRect 36 56 184 30 10))
        $body = New-Object System.Drawing.Drawing2D.GraphicsPath
        $body.AddPolygon([System.Drawing.PointF[]]@(
            (New-Object System.Drawing.PointF(52, 98)), (New-Object System.Drawing.PointF(204, 98)),
            (New-Object System.Drawing.PointF(188, 234)), (New-Object System.Drawing.PointF(68, 234))))
        $g.FillPath($lightBrush, $body)
    } else {
        $g.FillPath($lightBrush, (New-RoundedRect 100 36 56 26 9))
        $g.FillPath($slateBrush, (New-RoundedRect 114 48 28 16 3))
        $g.FillPath($lightBrush, (New-RoundedRect 44 60 168 22 8))
        $body = New-Object System.Drawing.Drawing2D.GraphicsPath
        $body.AddPolygon([System.Drawing.PointF[]]@(
            (New-Object System.Drawing.PointF(58, 92)), (New-Object System.Drawing.PointF(198, 92)),
            (New-Object System.Drawing.PointF(184, 222)), (New-Object System.Drawing.PointF(72, 222))))
        $g.FillPath($lightBrush, $body)
        # Slats.
        $slatPen = New-Object System.Drawing.Pen($slate, 12)
        $slatPen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
        $slatPen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
        $g.DrawLine($slatPen, 96, 112, 102, 202)
        $g.DrawLine($slatPen, 128, 112, 128, 202)
        $g.DrawLine($slatPen, 160, 112, 154, 202)
    }

    # Amber clock badge, lower right, with a slate ring separating it from the bin.
    if ($small) { $cx = 192; $cy = 192; $r = 58; $ring = 14 } else { $cx = 190; $cy = 190; $r = 56; $ring = 12 }
    $g.FillEllipse($slateBrush, $cx - $r - $ring, $cy - $r - $ring, 2 * ($r + $ring), 2 * ($r + $ring))
    $g.FillEllipse($amberBrush, $cx - $r, $cy - $r, 2 * $r, 2 * $r)
    $handWidth = if ($small) { 26 } else { 14 }
    $hand = New-Object System.Drawing.Pen($slate, $handWidth)
    $hand.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $hand.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
    $g.DrawLine($hand, $cx, $cy, $cx, $cy - $r * 0.62)
    $g.DrawLine($hand, $cx, $cy, $cx + $r * 0.5, $cy)

    $g.Dispose()
    return $bmp
}

function Get-PngBytes($bmp) {
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    return , $ms.ToArray()
}

$sizes = 16, 32, 48, 256
$images = @()
foreach ($s in $sizes) {
    $bmp = New-IconBitmap $s
    $name = if ($s -eq 256) { 'icon-256.png' } else { "preview-$s.png" }
    $bmp.Save((Join-Path $assets $name), [System.Drawing.Imaging.ImageFormat]::Png)
    $images += , (Get-PngBytes $bmp)
    $bmp.Dispose()
}

# ICO container: ICONDIR, one ICONDIRENTRY per image, then the PNG data.
$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter($out)
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $dim = if ($sizes[$i] -ge 256) { 0 } else { $sizes[$i] }
    $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32)
    $w.Write([uint32]$images[$i].Length); $w.Write([uint32]$offset)
    $offset += $images[$i].Length
}
foreach ($img in $images) { $w.Write($img) }
$w.Flush()
[System.IO.File]::WriteAllBytes((Join-Path $crate 'stale.ico'), $out.ToArray())
Write-Output "wrote $(Join-Path $crate 'stale.ico')"
