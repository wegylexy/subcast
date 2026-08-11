# demo.ps1 — Generate a short demo video on Windows.
#
# Usage:
#   .\demo.ps1
#   .\demo.ps1 -FontPath C:\Windows\Fonts\arial.ttf -Output demo.mp4
#
# Requirements: ffmpeg on PATH (e.g. via winget install ffmpeg or scoop install ffmpeg).

param(
    [string]$FontPath = "",
    [string]$Output = "demo.mp4"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# --- Auto-detect a system font if not supplied ---
if (-not $FontPath) {
    $candidates = @(
        "$env:SystemRoot\Fonts\segoeui.ttf",
        "$env:SystemRoot\Fonts\arial.ttf",
        "$env:SystemRoot\Fonts\calibri.ttf",
        "$env:SystemRoot\Fonts\tahoma.ttf",
        "$env:SystemRoot\Fonts\verdana.ttf"
    )
    foreach ($f in $candidates) {
        if (Test-Path $f) { $FontPath = $f; break }
    }
    if (-not $FontPath) {
        Write-Error "Cannot find a system font. Pass -FontPath 'C:\path\to\font.ttf'"
        exit 1
    }
    Write-Host "Using font: $FontPath"
}

Write-Host "Building subcast..."
cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Tab-separated subtitle cues
#
# The Thai lines below are a stacked-combining-mark regression check: each
# word carries two marks that must stack vertically (e.g. a tone mark above
# a vowel sign already above the base consonant). Naive glyph placement
# (cmap lookup with no OpenType GPOS) renders the top mark missing or
# overlapping instead of stacked — see CLAUDE.md's shaping note.
$subtitles = @(
    "0`t2000`tPlain text subtitle",
    "2000`t4000`t<b>Bold</b> text",
    "4000`t6000`t<i>Italic</i> text",
    "6000`t8000`t<b><i>Bold italic</i></b> combined",
    "8000`t10000`tMixed: <b>bold</b> and <i>italic</i>   second line here",
    "10000`t12000`tเพื่อน",
    "12000`t14000`tนั้น",
    "14000`t16000`tซึ่ง",
    "16000`t18000`tเนื้อหา"
) -join "`n"

Write-Host "Generating: $Output"

$env:FONT_PATH       = $FontPath
$env:FPS             = "25"
$env:WIDTH           = "1920"
$env:HEIGHT          = "1080"
$env:BASELINE        = "1026"
$env:FONT_SIZE       = "60"
$env:SHADOW_DISTANCE = "3"
$env:SHADOW_BLUR     = "6"
$env:SHADOW_OPACITY  = "0.75"

$subtitles `
| .\target\release\subcast.exe `
| ffmpeg -y `
    -f rawvideo -pixel_format rgba -video_size 1920x1080 -framerate 25 `
    -i pipe:0 `
    -vf "scale=960:540,format=yuv420p" `
    -c:v libx264 -preset fast -crf 23 `
    $Output

Write-Host "Done: $Output"
