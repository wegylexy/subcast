# CLAUDE.md

Developer guide for AI coding assistants working on `subcast`.

## Project Overview

`subcast` is a single-binary Rust program that renders subtitle text onto transparent RGBA frames using [Skia](https://skia.org) (via [`skia-safe`](https://crates.io/crates/skia-safe)). It reads timed subtitle cues from stdin and streams raw RGBA frames to stdout for piping into FFmpeg.

## Architecture

```
stdin (TSV lines)
  → parse_line()   → Subtitle { start, end, lines: Vec<String> }
  → frame loop     → per-frame subtitle scheduling (no overlap)
  → draw_subtitle()
      → parse_runs()  → Vec<TextRun { text, bold, italic }>
      → styled_font() → Font (synthetic bold/italic)
      → Skia canvas   → shadow pass + text pass per run
  → read_pixels()  → raw RGBA bytes
  → stdout
```

### Key Functions

| Function | Purpose |
|---|---|
| `parse_line` | Splits a TSV line into `Subtitle` fields |
| `parse_runs` | Parses inline `<b>`/`<i>` tags into styled `TextRun`s |
| `styled_font` | Builds a `Font` with synthetic bold/italic matching SVG/CSS |
| `draw_subtitle` | Renders all runs of all lines onto the Skia canvas |
| `env_or` | Reads an env var with type-inferred parsing and a default |

## Inline Tag Handling

`parse_runs` is a lightweight state machine (no regex). It handles:

| Tag | Effect |
|---|---|
| `<b>` / `</b>` | Toggle `bold` |
| `<i>` / `</i>` | Toggle `italic` |
| anything else | Text preserved, tag silently dropped |

This matches YouTube's WebVTT rendering subset.

## Font Rendering — Synthetic Bold/Italic

A single font file is loaded via `FONT_PATH`. Bold and italic are **synthesised** using Skia primitives:

- **Bold** → `font.set_embolden(true)` — same as SVG/CSS stroke-widening synthesis
- **Italic** → `font.set_skew_x(-0.25)` — horizontal shear at tan ≈ 14°, identical to CSS `font-style: oblique`

This is intentional: the same font renders in an SVG-based web preview editor, so the synthesis must match across both targets. Do **not** switch to loading separate bold/italic font files without coordinating the SVG renderer.

## Adding a New Inline Tag

1. Add a field to `TextRun` (e.g. `underline: bool`).
2. Add match arms in `parse_runs` for the open/close tag strings.
3. Apply the style in `draw_subtitle`: either via `Paint` (e.g. underline via `draw_line`) or a new `styled_font` flag.
4. Add unit tests in the `#[cfg(test)]` block.

## Build & Test

```bash
# Debug build
cargo build

# Release build (used in Docker and demo scripts)
cargo build --release

# Unit tests — no font required, pure logic only
cargo test

# Generate a demo video
bash demo.sh              # Linux/macOS/WSL — auto-detects system font
.\demo.ps1               # Windows PowerShell — auto-detects %SystemRoot%\Fonts\*
```

## Design Decisions

| Decision | Rationale |
|---|---|
| Raw RGBA stdout | No container overhead; FFmpeg handles all muxing |
| Single-file binary | Simple to deploy; `src/main.rs` is the entire codebase |
| No temporal overlap | Subtitle scheduling is intentionally sequential; a new cue waits for the active one to finish |
| Single-pass streaming | Frames are emitted in real-time order; input is consumed lazily as frames are needed |
| Skia for rendering | High-quality text shaping, kerning, subpixel antialiasing |
| **FPS=25 default** | A subtitle appears on the first frame where `now_ms ≥ start_ms`, so exact frame alignment requires `start_ms` to be a multiple of `1000/fps` ms. At 25 fps the step is **40 ms** (exact in f64), so `now_ms` is always a clean integer — no float truncation. At 24/30/60 fps the step is a repeating decimal, adding up to ~0.67/0.33/0.17 ms of truncation wobble per frame. In all cases a subtitle at an arbitrary ms timestamp may appear up to one full frame late. |

## Dependencies

| Crate | Purpose |
|---|---|
| `skia-safe` | Skia 2D graphics — font loading, text rendering, raster surfaces |

## File Layout

```
src/main.rs          ← entire codebase (single-file binary)
demo.sh              ← FFmpeg pipeline demo (Linux/macOS/WSL, auto-detects font)
demo.ps1             ← FFmpeg pipeline demo (Windows PowerShell, auto-detects font)
Dockerfile           ← multi-stage build: rust:1.92 → nvidia/cuda runtime
.github/workflows/   ← Docker publish CI
```
