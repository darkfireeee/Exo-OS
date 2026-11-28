# tools/bake_ui_assets.ps1
# Bakes crisp 8-bit anti-aliased Inter & UbuntuMono `.exofont` atlases, alpha-matted 32-bit RGBA icons
# (including the user's hexagonal Start emblem), and the Bayer-dithered 960x540 RGB565 Strata Aurora wallpaper
# directly into `assets/ui/` (outside any `src/` tree).

param(
    [string]$RepoRoot = "c:\Users\xavie\Desktop\Exo-OS",
    [string]$BrainDir = "C:\Users\xavie\.gemini\antigravity\brain\3e3de4b9-66a3-47ec-80cb-ff69a35566a3"
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$UiDir = Join-Path $RepoRoot "assets\ui"
$IconsDir = Join-Path $UiDir "icons"
$FontsDir = Join-Path $UiDir "fonts"
New-Item -ItemType Directory -Force -Path $UiDir, $IconsDir, $FontsDir | Out-Null

# ── 1. Helper: Bake `.exofont` (EXOF v1, 8-bit AA alpha, ASCII + Latin-1 French accents) ──
function Bake-ExoFont {
    param(
        [string]$FontPath,
        [float]$PixelSize,
        [bool]$Bold,
        [string]$OutPath
    )
    $pfc = New-Object System.Drawing.Text.PrivateFontCollection
    $pfc.AddFontFile($FontPath)
    $family = $pfc.Families[0]
    $style = if ($Bold) { [System.Drawing.FontStyle]::Bold } else { [System.Drawing.FontStyle]::Regular }
    $font = New-Object System.Drawing.Font($family, $PixelSize, $style, [System.Drawing.GraphicsUnit]::Pixel)

    $codepoints = New-Object System.Collections.Generic.List[int]
    for ($cp = 0x20; $cp -le 0x7E; $cp++) { $codepoints.Add($cp) }
    for ($cp = 0xA0; $cp -le 0xFF; $cp++) { $codepoints.Add($cp) }
    # Add useful typographic glyphs (sorted strictly ascending for binary search!)
    foreach ($extra in @(0x0152, 0x0153, 0x2014, 0x2022, 0x2026, 0x2190, 0x2191, 0x2192, 0x2193, 0x25B8, 0x25C2, 0x2713)) {
        $codepoints.Add($extra)
    }
    $codepoints.Sort()

    $lineH = [int][Math]::Ceiling($font.GetHeight(96.0))
    $ascent = [int][Math]::Round($PixelSize * 0.85)
    $cellW = [int][Math]::Ceiling($PixelSize * 2.2) + 8
    $cellH = $lineH + 8

    $bmp = New-Object System.Drawing.Bitmap($cellW, $cellH, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
    $sf = [System.Drawing.StringFormat]::GenericTypographic
    $sf.FormatFlags = $sf.FormatFlags -bor [System.Drawing.StringFormatFlags]::MeasureTrailingSpaces
    $brush = [System.Drawing.Brushes]::White

    $entries = New-Object System.Collections.Generic.List[object]
    $bitmapBlob = New-Object System.IO.MemoryStream

    foreach ($cp in $codepoints) {
        $str = [char]::ConvertFromUtf32($cp)
        $g.Clear([System.Drawing.Color]::Black)
        $sz = $g.MeasureString($str, $font, 1000, $sf)
        $adv = [int][Math]::Round($sz.Width)
        if ($cp -eq 0x20) {
            $adv = [Math]::Max($adv, [int][Math]::Round($PixelSize * 0.30))
        }
        $adv = [Math]::Clamp($adv, 1, 255)

        $g.DrawString($str, $font, $brush, 1.0, 1.0, $sf)

        # Find tight bounding box of non-zero pixels
        $minX = $cellW; $minY = $cellH; $maxX = -1; $maxY = -1
        for ($y = 0; $y -lt $cellH; $y++) {
            for ($x = 0; $x -lt $cellW; $x++) {
                $px = $bmp.GetPixel($x, $y)
                $v = [Math]::Max($px.R, [Math]::Max($px.G, $px.B))
                if ($v -gt 6) {
                    if ($x -lt $minX) { $minX = $x }
                    if ($y -lt $minY) { $minY = $y }
                    if ($x -gt $maxX) { $maxX = $x }
                    if ($y -gt $maxY) { $maxY = $y }
                }
            }
        }

        $offset = [uint32]$bitmapBlob.Position
        if ($maxX -lt $minX) {
            $gw = 0; $gh = 0; $by = 0
        } else {
            # Keep left bearing by starting at x = 1 if minX >= 1
            $startX = [Math]::Min(1, $minX)
            $gw = [Math]::Min(255, ($maxX - $startX + 1))
            $gh = [Math]::Min(255, ($maxY - $minY + 1))
            $by = [Math]::Clamp(($minY - 1), -127, 127)
            for ($y = 0; $y -lt $gh; $y++) {
                for ($x = 0; $x -lt $gw; $x++) {
                    $px = $bmp.GetPixel($startX + $x, $minY + $y)
                    $v = [byte][Math]::Max($px.R, [Math]::Max($px.G, $px.B))
                    $bitmapBlob.WriteByte($v)
                }
            }
            if ($gw -gt $adv + 1) {
                $adv = [Math]::Min(255, $gw + 1)
            }
        }

        $entries.Add([PSCustomObject]@{
            cp = [uint32]$cp
            adv = [byte]$adv
            w = [byte]$gw
            h = [byte]$gh
            by = [sbyte]$by
            off = $offset
        })
    }

    $g.Dispose(); $bmp.Dispose(); $font.Dispose(); $pfc.Dispose()

    $out = New-Object System.IO.MemoryStream
    $bw = New-Object System.IO.BinaryWriter($out)
    $bw.Write([System.Text.Encoding]::ASCII.GetBytes("EXOF"))
    $bw.Write([uint16]$lineH)
    $bw.Write([uint16]$ascent)
    $bw.Write([uint16]$entries.Count)
    $bw.Write([uint16]0)
    $bw.Write([uint32]$bitmapBlob.Length)
    foreach ($e in $entries) {
        $bw.Write($e.cp)
        $bw.Write($e.adv)
        $bw.Write($e.w)
        $bw.Write($e.h)
        $bw.Write($e.by)
        $bw.Write($e.off)
    }
    $bw.Write($bitmapBlob.ToArray())
    $bw.Flush()
    [System.IO.File]::WriteAllBytes($OutPath, $out.ToArray())
    $bw.Dispose(); $out.Dispose(); $bitmapBlob.Dispose()
    Write-Host "  [FONT] $(Split-Path $OutPath -Leaf) : $($entries.Count) glyphs, lineH=$lineH, $((Get-Item $OutPath).Length) B"
}

Write-Host "=== Baking Inter & UbuntuMono anti-aliased fonts ==="
$interReg  = "C:\Windows\Fonts\Inter-Regular-slnt=0.ttf"
$interBold = "C:\Windows\Fonts\Inter-Bold-slnt=0.ttf"
$monoReg   = "C:\Windows\Fonts\UbuntuMono-Regular.ttf"
if (-not (Test-Path $interReg))  { $interReg  = "C:\Windows\Fonts\segoeui.ttf" }
if (-not (Test-Path $interBold)) { $interBold = "C:\Windows\Fonts\segoeuib.ttf" }
if (-not (Test-Path $monoReg))   { $monoReg   = "C:\Windows\Fonts\consola.ttf" }

Bake-ExoFont -FontPath $interReg  -PixelSize 13 -Bold $false -OutPath (Join-Path $FontsDir "inter_13.exofont")
Bake-ExoFont -FontPath $interReg  -PixelSize 15 -Bold $false -OutPath (Join-Path $FontsDir "inter_15.exofont")
Bake-ExoFont -FontPath $interBold -PixelSize 18 -Bold $true  -OutPath (Join-Path $FontsDir "inter_18_bold.exofont")
Bake-ExoFont -FontPath $interBold -PixelSize 26 -Bold $true  -OutPath (Join-Path $FontsDir "inter_26_bold.exofont")
Bake-ExoFont -FontPath $monoReg   -PixelSize 15 -Bold $false -OutPath (Join-Path $FontsDir "mono_15.exofont")

# ── 2. Helper: Extract & Alpha-Matte Crystal Icon to RGBA raw bytes ──
function Bake-CrystalIcon {
    param(
        [System.Drawing.Bitmap]$SourceBmp,
        [int]$CropX, [int]$CropY, [int]$CropW, [int]$CropH,
        [int]$OutSize,
        [int]$BgThreshold,
        [bool]$UseRadialHexMatte,
        [string]$OutPath
    )
    # Crop -> trim tight non-background bbox -> high-quality bicubic resize to OutSize x OutSize
    $tmp = New-Object System.Drawing.Bitmap($OutSize, $OutSize, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($tmp)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $srcRect = New-Object System.Drawing.Rectangle($CropX, $CropY, $CropW, $CropH)
    $dstRect = New-Object System.Drawing.Rectangle(0, 0, $OutSize, $OutSize)
    $g.DrawImage($SourceBmp, $dstRect, $srcRect, [System.Drawing.GraphicsUnit]::Pixel)
    $g.Dispose()

    # Flood-fill outer background pixels (preserves dark obsidian glass inside icons!)
    $w = $OutSize; $h = $OutSize
    $isBg = New-Object 'bool[,]' $w, $h
    $q = New-Object System.Collections.Generic.Queue[int]

    function Try-Enqueue([int]$x, [int]$y) {
        if ($x -lt 0 -or $y -lt 0 -or $x -ge $w -or $y -ge $h) { return }
        if ($isBg[$x, $y]) { return }
        $c = $tmp.GetPixel($x, $y)
        $maxCh = [Math]::Max($c.R, [Math]::Max($c.G, $c.B))
        # For the user's attached Start logo (which has deep navy blue gradient background), also key low-R/G dark navy on border
        $isDarkBg = if ($UseRadialHexMatte) {
            ($c.R -lt 26 -and $c.G -lt 48 -and $c.B -lt 115)
        } else {
            ($maxCh -le $BgThreshold)
        }
        if ($isDarkBg) {
            $isBg[$x, $y] = $true
            $q.Enqueue(($y * $w) + $x)
        }
    }

    for ($x = 0; $x -lt $w; $x++) { Try-Enqueue $x 0; Try-Enqueue $x ($h - 1) }
    for ($y = 0; $y -lt $h; $y++) { Try-Enqueue 0 $y; Try-Enqueue ($w - 1) $y }

    while ($q.Count -gt 0) {
        $idx = $q.Dequeue()
        $x = $idx % $w
        $y = [int]($idx / $w)
        Try-Enqueue ($x + 1) $y
        Try-Enqueue ($x - 1) $y
        Try-Enqueue $x ($y + 1)
        Try-Enqueue $x ($y - 1)
    }

    # Build straight RGBA with soft anti-aliased 1px edge against bg
    $bytes = New-Object byte[] ($w * $h * 4)
    for ($y = 0; $y -lt $h; $y++) {
        for ($x = 0; $x -lt $w; $x++) {
            $c = $tmp.GetPixel($x, $y)
            $off = ($y * $w + $x) * 4
            if ($isBg[$x, $y]) {
                $bytes[$off] = 0; $bytes[$off+1] = 0; $bytes[$off+2] = 0; $bytes[$off+3] = 0
            } else {
                # Check how many 4-neighbors are bg for silky edge feathering
                $bgN = 0
                if ($x -gt 0 -and $isBg[($x - 1), $y]) { $bgN++ }
                if (($x + 1) -lt $w -and $isBg[($x + 1), $y]) { $bgN++ }
                if ($y -gt 0 -and $isBg[$x, ($y - 1)]) { $bgN++ }
                if (($y + 1) -lt $h -and $isBg[$x, ($y + 1)]) { $bgN++ }
                $a = if ($bgN -eq 0) {
                    255
                } else {
                    $lum = [Math]::Max($c.R, [Math]::Max($c.G, $c.B))
                    [Math]::Clamp([int]($lum * 2.2), 70, 235)
                }
                $bytes[$off]   = $c.R
                $bytes[$off+1] = $c.G
                $bytes[$off+2] = $c.B
                $bytes[$off+3] = [byte]$a
            }
        }
    }
    $tmp.Dispose()
    [System.IO.File]::WriteAllBytes($OutPath, $bytes)
    Write-Host "  [ICON] $(Split-Path $OutPath -Leaf) : ${w}x${h} RGBA ($($bytes.Length) B)"
}

Write-Host "=== Baking User Start Logo & Crystal Icons ==="
$startLogoSrc = [System.Drawing.Bitmap]::FromFile((Join-Path $BrainDir ".user_uploaded\media_1795707428075_45707547.jpg"))
$sw = $startLogoSrc.Width; $sh = $startLogoSrc.Height
$pad = [int]($sw * 0.11)
Bake-CrystalIcon -SourceBmp $startLogoSrc -CropX $pad -CropY $pad -CropW ($sw - 2*$pad) -CropH ($sh - 2*$pad) -OutSize 40 -BgThreshold 22 -UseRadialHexMatte $true -OutPath (Join-Path $IconsDir "start_logo_40.rgba")
Bake-CrystalIcon -SourceBmp $startLogoSrc -CropX $pad -CropY $pad -CropW ($sw - 2*$pad) -CropH ($sh - 2*$pad) -OutSize 64 -BgThreshold 22 -UseRadialHexMatte $true -OutPath (Join-Path $IconsDir "start_logo_64.rgba")
$startLogoSrc.Dispose()

$sheet1 = [System.Drawing.Bitmap]::FromFile((Join-Path $BrainDir "exo_os_icons_2x2_grid_clean_1795707693938.jpg"))
$hw = [int]($sheet1.Width / 2); $hh = [int]($sheet1.Height / 2)
$m1 = [int]($hw * 0.08)
Bake-CrystalIcon -SourceBmp $sheet1 -CropX $m1         -CropY $m1         -CropW ($hw - 2*$m1) -CropH ($hh - 2*$m1) -OutSize 40 -BgThreshold 18 -UseRadialHexMatte $false -OutPath (Join-Path $IconsDir "files_40.rgba")
Bake-CrystalIcon -SourceBmp $sheet1 -CropX ($hw + $m1) -CropY $m1         -CropW ($hw - 2*$m1) -CropH ($hh - 2*$m1) -OutSize 40 -BgThreshold 18 -UseRadialHexMatte $false -OutPath (Join-Path $IconsDir "terminal_40.rgba")
Bake-CrystalIcon -SourceBmp $sheet1 -CropX $m1         -CropY ($hh + $m1) -CropW ($hw - 2*$m1) -CropH ($hh - 2*$m1) -OutSize 40 -BgThreshold 18 -UseRadialHexMatte $false -OutPath (Join-Path $IconsDir "doctor_40.rgba")
Bake-CrystalIcon -SourceBmp $sheet1 -CropX ($hw + $m1) -CropY ($hh + $m1) -CropW ($hw - 2*$m1) -CropH ($hh - 2*$m1) -OutSize 40 -BgThreshold 18 -UseRadialHexMatte $false -OutPath (Join-Path $IconsDir "settings_40.rgba")
$sheet1.Dispose()

$sheet2 = [System.Drawing.Bitmap]::FromFile((Join-Path $BrainDir "exo_os_icons_grid_v3_1795707715315.jpg"))
$hw2 = [int]($sheet2.Width / 2); $hh2 = [int]($sheet2.Height / 2)
$m2 = [int]($hw2 * 0.08)
Bake-CrystalIcon -SourceBmp $sheet2 -CropX $m2          -CropY $m2          -CropW ($hw2 - 2*$m2) -CropH ($hh2 - 2*$m2) -OutSize 40 -BgThreshold 18 -UseRadialHexMatte $false -OutPath (Join-Path $IconsDir "phoenix_40.rgba")
Bake-CrystalIcon -SourceBmp $sheet2 -CropX ($hw2 + $m2) -CropY $m2          -CropW ($hw2 - 2*$m2) -CropH ($hh2 - 2*$m2) -OutSize 40 -BgThreshold 18 -UseRadialHexMatte $false -OutPath (Join-Path $IconsDir "shield_40.rgba")
Bake-CrystalIcon -SourceBmp $sheet2 -CropX $m2          -CropY ($hh2 + $m2) -CropW ($hw2 - 2*$m2) -CropH ($hh2 - 2*$m2) -OutSize 40 -BgThreshold 18 -UseRadialHexMatte $false -OutPath (Join-Path $IconsDir "doc_40.rgba")
Bake-CrystalIcon -SourceBmp $sheet2 -CropX ($hw2 + $m2) -CropY ($hh2 + $m2) -CropW ($hw2 - 2*$m2) -CropH ($hh2 - 2*$m2) -OutSize 40 -BgThreshold 18 -UseRadialHexMatte $false -OutPath (Join-Path $IconsDir "network_40.rgba")
$sheet2.Dispose()

# ── 3. Bake Bayer-Dithered 960x540 RGB565 Strata Aurora Wallpaper ──
Write-Host "=== Baking 960x540 RGB565 Strata Aurora Wallpaper ==="
$wpSrc = [System.Drawing.Bitmap]::FromFile((Join-Path $BrainDir "exo_os_strata_wallpaper_v2_1795707699597.jpg"))
$wpW = 960; $wpH = 540
$wpBmp = New-Object System.Drawing.Bitmap($wpW, $wpH, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
$g = [System.Drawing.Graphics]::FromImage($wpBmp)
$g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
$g.DrawImage($wpSrc, 0, 0, $wpW, $wpH)
$g.Dispose(); $wpSrc.Dispose()

$rect = New-Object System.Drawing.Rectangle(0, 0, $wpW, $wpH)
$bd = $wpBmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
$rawLen = [Math]::Abs($bd.Stride) * $wpH
$raw = New-Object byte[] $rawLen
[System.Runtime.InteropServices.Marshal]::Copy($bd.Scan0, $raw, 0, $rawLen)
$stride = $bd.Stride
$wpBmp.UnlockBits($bd); $wpBmp.Dispose()

$outWp = New-Object byte[] ($wpW * $wpH * 2)
$bayer = @(@(0, 2), @(3, 1))
$oi = 0
for ($y = 0; $y -lt $wpH; $y++) {
    $rowOff = $y * $stride
    $by = $y -band 1
    for ($x = 0; $x -lt $wpW; $x++) {
        $p = $rowOff + $x * 3
        $d = $bayer[$by][$x -band 1]
        $b = [Math]::Min(255, [int]$raw[$p]     + ($d * 2))
        $g8 = [Math]::Min(255, [int]$raw[$p + 1] + $d)
        $r = [Math]::Min(255, [int]$raw[$p + 2] + ($d * 2))
        $v = (($r -shr 3) -shl 11) -bor (($g8 -shr 2) -shl 5) -bor ($b -shr 3)
        $outWp[$oi]     = [byte]($v -band 0xFF)
        $outWp[$oi + 1] = [byte](($v -shr 8) -band 0xFF)
        $oi += 2
    }
}
$wpOutPath = Join-Path $RepoRoot "assets\wallpapers\wallpaper_strata_aurora_960x540.rgb565"
[System.IO.File]::WriteAllBytes($wpOutPath, $outWp)
Write-Host "  [WALLPAPER] $(Split-Path $wpOutPath -Leaf) : ${wpW}x${wpH} RGB565 ($($outWp.Length) B)"
Write-Host "=== ALL UI ASSETS BAKED SUCCESSFULLY ==="
