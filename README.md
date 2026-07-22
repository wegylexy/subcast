# SubCast

[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange)](https://www.rust-lang.org)
[![Docker](https://img.shields.io/badge/docker-ghcr.io-blue)](https://github.com/features/packages)

A fast subtitle renderer that reads timed text from stdin and writes raw RGBA frames to stdout, designed to be chained into an FFmpeg pipeline. Rendered with [Skia](https://skia.org) for high-quality antialiased text.

## Quick Start

```bash
cargo build --release

printf '0\t5000\tHello, <b>world</b>!\n' \
| FONT_PATH=/path/to/font.ttf ./target/release/subcast \
| ffmpeg -f rawvideo -pixel_format rgba -video_size 1920x1080 -framerate 25 \
         -i pipe:0 -vf format=yuv420p output.mp4
```

## Demo

Generates a 10-second video exercising all inline styles.
`FONT_PATH` is auto-detected from system fonts when not set.

```bash
# Linux / macOS / WSL
bash demo.sh demo.mp4

# Windows (PowerShell, requires ffmpeg on PATH)
.\demo.ps1
```

## Input Format

Each line on stdin is tab-separated:

```
{startMS}\t{endMS}\t{text}
```

| Field | Description |
|---|---|
| `startMS` | Subtitle start time in milliseconds |
| `endMS` | Subtitle end time in milliseconds |
| `text` | Subtitle text (see below) |

- Separate **display lines** within one cue with 3 consecutive spaces (`   `).
- Malformed lines are skipped with a warning to stderr.
- Input is read lazily; reading stops when stdout is closed.
- Cues are non-overlapping: if a cue starts before the active one ends, it waits.

### Inline Formatting

`<b>` and `<i>` tags are supported, matching YouTube's WebVTT rendering:

| Markup | Effect |
|---|---|
| `<b>text</b>` | Bold (synthetic embolden) |
| `<i>text</i>` | Italic (synthetic oblique, shear −0.25) |
| `<b><i>text</i></b>` | Bold italic |

Styling uses Skia's synthetic bold (`set_embolden`) and oblique (`set_skew_x(-0.25)`), which matches SVG/CSS synthesis — ensuring consistent rendering between this tool and SVG-based web previews using the same font.

Unknown tags (e.g. `<c.color>`) are ignored; their text content is preserved.

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `FONT_PATH` | — | **Required.** Path to a `.ttf` or `.otf` font file |
| `FPS`             | `25`    | Frame rate. At 25 fps the frame step is exactly 40 ms (no float rounding), so `now_ms` is always a clean integer. A subtitle appears on the first frame where `now_ms ≥ start_ms`; for arbitrary ms timestamps it may be up to one frame late. |
| `WIDTH` | `1920` | Output width in pixels |
| `HEIGHT` | `1080` | Output height in pixels |
| `BASELINE` | `1026` | Y-coordinate of the bottom text baseline |
| `FONT_SIZE` | `60` | Font size in points |
| `LINE_HEIGHT` | `1` | Line height multiplier |
| `SHADOW_ANGLE` | `45` | Drop shadow angle in degrees |
| `SHADOW_DISTANCE` | `0` | Drop shadow offset in pixels |
| `SHADOW_BLUR` | `0` | Drop shadow blur radius in pixels |
| `SHADOW_OPACITY` | `1` | Drop shadow opacity (0–1) |

## Output

A stream of raw RGBA frames — each frame is `WIDTH × HEIGHT × 4` bytes in `RGBA8888` format. Transparent frames are emitted for gaps between subtitle cues.

## Docker

```bash
docker build -t subcast .
# or pull from GHCR:
docker pull ghcr.io/<owner>/subcast
```

The published image is based on `nvidia/cuda` and includes FFmpeg, suitable for GPU-accelerated video pipelines.

## Development

```bash
cargo test          # unit tests (no font required)
cargo build         # debug build
cargo build --release  # production binary
```

See [CLAUDE.md](CLAUDE.md) for architecture notes and contributor guidance.