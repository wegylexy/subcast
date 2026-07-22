#!/usr/bin/env bash
# demo.sh — generate a short demo video exercising <b> and <i> tag rendering.
#
# Usage:
#   bash demo.sh [output.mp4]
#   FONT_PATH=/path/to/font.ttf bash demo.sh [output.mp4]
#
# FONT_PATH is auto-detected from system fonts when not set.
# Requirements: ffmpeg on PATH. On Windows, use demo.ps1 instead.

set -euo pipefail

OUTPUT="${1:-demo.mp4}"

# --- Auto-detect a system font if FONT_PATH is not set ---
if [ -z "${FONT_PATH:-}" ]; then
    # Linux: prefer fontconfig, fall back to well-known paths
    if command -v fc-match >/dev/null 2>&1; then
        FONT_PATH="$(fc-match --format='%{file}' sans 2>/dev/null || true)"
    fi
    # macOS / other fallbacks
    if [ -z "${FONT_PATH:-}" ]; then
        for candidate in \
            "/System/Library/Fonts/Supplemental/Arial.ttf" \
            "/Library/Fonts/Arial.ttf" \
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf" \
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf" \
            "/usr/share/fonts/TTF/DejaVuSans.ttf"; do
            if [ -f "$candidate" ]; then
                FONT_PATH="$candidate"
                break
            fi
        done
    fi
    if [ -z "${FONT_PATH:-}" ]; then
        echo "ERROR: Cannot find a system font. Set FONT_PATH=/path/to/font.ttf"
        echo "       On Windows use demo.ps1 instead."
        exit 1
    fi
    echo "Using font: $FONT_PATH"
fi

echo "Building subcast..."
cargo build --release 2>&1

echo "Generating: $OUTPUT"

# Each line: startMS <TAB> endMS <TAB> text
# Three spaces (   ) separate display lines within one subtitle.
printf '%s\n' \
  $'0\t2000\tPlain text subtitle' \
  $'2000\t4000\t<b>Bold</b> text' \
  $'4000\t6000\t<i>Italic</i> text' \
  $'6000\t8000\t<b><i>Bold italic</i></b> combined' \
  $'8000\t10000\tMixed: <b>bold</b> and <i>italic</i>   second line here' \
| FONT_PATH="$FONT_PATH" \
  FPS=25 WIDTH=1920 HEIGHT=1080 BASELINE=1026 FONT_SIZE=60 \
  SHADOW_DISTANCE=3 SHADOW_BLUR=6 SHADOW_OPACITY=0.75 \
  ./target/release/subcast \
| ffmpeg -y \
    -f rawvideo -pixel_format rgba -video_size 1920x1080 -framerate 25 \
    -i pipe:0 \
    -vf "scale=960:540,format=yuv420p" \
    -c:v libx264 -preset fast -crf 23 \
    "$OUTPUT"

echo "Done: $OUTPUT"
