use skia_safe::shaper::RunHandler;
use skia_safe::shaper::run_handler::{Buffer, RunInfo};
use skia_safe::{
    AlphaType, BlurStyle, Color, ColorType, Data, Font, FontMgr, ImageInfo, MaskFilter, Paint,
    Point, Shaper, Surface, TextBlob, TextBlobBuilder, Typeface, surfaces,
};
use std::env;
use std::io::{self, BufRead, Write};
use std::str::FromStr;

fn env_or<T: FromStr>(key: &str, default: T) -> T {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

struct Config {
    fps: u64,
    width: i32,
    height: i32,
    baseline: i32,
    font_path: String,
    font_size: f32,
    line_height_multiplier: f32,
    shadow_angle: f32,
    shadow_distance: f32,
    shadow_blur: f32,
    shadow_opacity: f32,
}

struct Subtitle {
    start: u64,
    end: u64,
    lines: Vec<String>,
}

/// A single styled run of text within a subtitle line.
struct TextRun {
    text: String,
    bold: bool,
    italic: bool,
}

/// Core run-parser. Accepts and returns depth counters so callers can thread
/// state across multiple segments (e.g. display lines separated by `"   "`).
///
/// Each `<b>` / `<i>` increments a depth counter; each `</b>` / `</i>` decrements
/// it (floor 0). Bold/italic is active while the respective depth > 0. This means:
///
/// - Nesting order doesn't matter: `<b><i>` ≡ `<i><b>`
/// - Wrong close order is tolerated: `<b><i>T</b></i>` renders T as bold+italic
/// - Unmatched open tag: style stays active to end of string
/// - Unmatched close tag: depth saturates at 0, no panic
/// - Double open: `<b><b>T</b>more</b>` — T and "more" both bold (unlike a
///   simple bool flip, which would drop bold after the first `</b>`)
/// - Flush happens before decrement: at `</b>`, buffered text is emitted with
///   the style that was active *before* the tag, then the counter drops.
fn parse_runs_stateful(
    input: &str,
    mut bold_depth: u32,
    mut italic_depth: u32,
) -> (Vec<TextRun>, u32, u32) {
    let mut runs: Vec<TextRun> = Vec::new();
    let mut current_text = String::new();
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '<' {
            // Collect everything up to the closing '>'
            let mut tag = String::new();
            for c in chars.by_ref() {
                if c == '>' {
                    break;
                }
                tag.push(c);
            }
            // Flush any accumulated text as a run before changing style
            if !current_text.is_empty() {
                runs.push(TextRun {
                    text: current_text.clone(),
                    bold: bold_depth > 0,
                    italic: italic_depth > 0,
                });
                current_text.clear();
            }
            match tag.as_str() {
                "b" => bold_depth += 1,
                "/b" => bold_depth = bold_depth.saturating_sub(1),
                "i" => italic_depth += 1,
                "/i" => italic_depth = italic_depth.saturating_sub(1),
                _ => {} // unknown tags (e.g. <c.color>, <ruby>) are ignored
            }
        } else {
            current_text.push(ch);
        }
    }

    if !current_text.is_empty() {
        runs.push(TextRun {
            text: current_text,
            bold: bold_depth > 0,
            italic: italic_depth > 0,
        });
    }

    (runs, bold_depth, italic_depth)
}

/// Parse a single string into styled runs, starting from unstyled state.
/// Convenience wrapper around `parse_runs_stateful` for standalone use and tests.
fn parse_runs(input: &str) -> Vec<TextRun> {
    parse_runs_stateful(input, 0, 0).0
}

/// Build a Skia `Font` for the given style using synthetic bold/italic —
/// the same synthesis that SVG/CSS applies when no separate variant file exists:
///   bold   → `set_embolden(true)`          (stroke widening)
///   italic → `set_skew_x(-0.25)`           (horizontal shear ≈ tan 14°, matching CSS oblique)
fn styled_font(typeface: &Typeface, size: f32, bold: bool, italic: bool) -> Font {
    let mut font = Font::new(typeface.clone(), size);
    if bold {
        font.set_embolden(true);
    }
    if italic {
        // -0.25 matches the shear browsers apply for synthetic italic in SVG/CSS
        font.set_skew_x(-0.25);
    }
    font
}

/// Practically-infinite shaping width: each display line is already a single
/// line (line breaks are handled by the caller), so HarfBuzz must never wrap.
const NO_WRAP_WIDTH: f32 = 1_000_000.0;

/// Collects HarfBuzz's shaped glyph runs straight into a `TextBlobBuilder`,
/// positioned relative to a y=0 baseline, while tracking the total x advance.
///
/// Skia's own `Shaper::shape_text_blob` convenience helper positions the
/// first line's baseline at `offset.y - ascent` (it assumes `offset` is the
/// top of a text box, like a paragraph layout) and its returned end-point is
/// the *next line's* start position — always `x = 0` for single-line text.
/// Neither behavior is usable here: we need an exact baseline origin and a
/// real total-advance width to lay out multiple styled runs on one line.
struct BlobCollector {
    builder: TextBlobBuilder,
    advance_x: f32,
}

impl BlobCollector {
    fn new() -> Self {
        Self {
            builder: TextBlobBuilder::new(),
            advance_x: 0.0,
        }
    }
}

impl RunHandler for BlobCollector {
    fn begin_line(&mut self) {}
    fn run_info(&mut self, _info: &RunInfo) {}
    fn commit_run_info(&mut self) {}

    fn run_buffer(&mut self, info: &RunInfo) -> Buffer<'_> {
        let (glyphs, positions) = self
            .builder
            .alloc_run_pos(info.font, info.glyph_count, None);
        Buffer::new(glyphs, positions, Point::new(self.advance_x, 0.0))
    }

    fn commit_run_buffer(&mut self, info: &RunInfo) {
        self.advance_x += info.advance.x;
    }

    fn commit_line(&mut self) {}
}

/// Shape a run of text with HarfBuzz (via Skia's `SkShaper`) against the given
/// font, returning the positioned glyph blob (baseline at y=0) and its total
/// advance width.
///
/// Plain `Font::measure_text` / `Canvas::draw_str` map text to glyphs by a
/// naive one-codepoint-to-one-glyph `cmap` lookup — no GSUB/GPOS is applied.
/// Scripts whose correctness depends on OpenType shaping (Thai stacked
/// combining marks, Arabic/Indic reordering and joining, etc.) render wrong
/// or overlapping without it. HarfBuzz shaping is required for *all*
/// scripts/languages to lay out correctly, not just non-Latin ones.
fn shape_run(shaper: &Shaper, text: &str, font: &Font) -> Option<(TextBlob, f32)> {
    if text.is_empty() {
        return None;
    }
    let mut collector = BlobCollector::new();
    shaper.shape(text, font, true, NO_WRAP_WIDTH, &mut collector);
    let width = collector.advance_x;
    collector.builder.make().map(|blob| (blob, width))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Load Configuration
    let config = Config {
        fps: env_or("FPS", 25),
        width: env_or("WIDTH", 1920),
        height: env_or("HEIGHT", 1080),
        baseline: env_or("BASELINE", 1026),
        font_path: env::var("FONT_PATH").expect("FONT_PATH environment variable must be set"),
        font_size: env_or("FONT_SIZE", 60.0),
        line_height_multiplier: env_or("LINE_HEIGHT", 1.0),
        shadow_angle: env_or("SHADOW_ANGLE", 45.0),
        shadow_distance: env_or("SHADOW_DISTANCE", 0.0),
        shadow_blur: env_or("SHADOW_BLUR", 0.0),
        shadow_opacity: env_or("SHADOW_OPACITY", 1.0),
    };

    // 2. Initialize Skia
    let info = ImageInfo::new(
        (config.width, config.height),
        ColorType::RGBA8888,
        AlphaType::Premul,
        None,
    );

    let mut surface = surfaces::raster(&info, None, None).expect("Failed to create skia surface");

    // 3. Load Font
    let font_data = Data::from_filename(&config.font_path).expect("Failed to read font file");
    let font_mgr = FontMgr::new();
    let typeface = font_mgr
        .new_from_data(&font_data, None)
        .expect("Failed to parse font");

    // Compute line height once from the base (unstyled) font
    let line_height =
        Font::new(typeface.clone(), config.font_size).spacing() * config.line_height_multiplier;

    // HarfBuzz-backed shaper: correct glyph selection/positioning (ligatures,
    // combining-mark stacking, reordering) for any script, not just Latin.
    let shaper = Shaper::new(font_mgr.clone());

    // 4. Prepare IO
    let stdin = io::stdin();
    let mut line_iter = stdin.lock().lines();
    let mut stdout = io::stdout().lock();

    // 5. State Initialization
    let mut frame_count: u64 = 0;
    let frame_dur_ms = 1000.0 / config.fps as f64;

    let mut active_sub: Option<Subtitle> = None;
    let mut queued_sub: Option<Subtitle> = None;

    // Rendering Cache
    let mut last_rendered_key: Option<(u64, u64)> = None;
    let mut is_cleared = false;

    // Buffer for output
    let row_bytes = config.width as usize * 4;
    let mut pixel_buffer = vec![0u8; (config.height as usize) * row_bytes];

    loop {
        let now_ms = (frame_count as f64 * frame_dur_ms) as u64;

        // --- Subtitle Management ---
        if let Some(sub) = &active_sub {
            if now_ms >= sub.end {
                active_sub = None;
            }
        }

        if active_sub.is_none() {
            if let Some(sub) = queued_sub.take() {
                if now_ms < sub.end {
                    if now_ms >= sub.start {
                        active_sub = Some(sub);
                    } else {
                        queued_sub = Some(sub);
                    }
                }
            }
        }

        if active_sub.is_none() && queued_sub.is_none() {
            if let Some(line_res) = line_iter.next() {
                match line_res {
                    Ok(line) => {
                        if let Some(sub) = parse_line(&line) {
                            queued_sub = Some(sub);
                            if let Some(qs) = &queued_sub {
                                if now_ms >= qs.start && now_ms < qs.end {
                                    active_sub = queued_sub.take();
                                }
                            }
                        } else {
                            eprintln!("Skipped: {}", line);
                        }
                    }
                    Err(_) => break,
                }
            } else {
                break;
            }
        } else if let Some(sub) = &queued_sub {
            if active_sub.is_none() && now_ms >= sub.start && now_ms < sub.end {
                active_sub = queued_sub.take();
            }
        }

        // --- Rendering ---
        let mut needs_read = false;

        if let Some(sub) = &active_sub {
            let key = (sub.start, sub.end);
            if last_rendered_key != Some(key) {
                draw_subtitle(&mut surface, sub, &config, &typeface, line_height, &shaper);
                last_rendered_key = Some(key);
                is_cleared = false;
                needs_read = true;
            } else if now_ms < sub.start && !is_cleared {
                // Waiting for start time
                surface.canvas().clear(Color::TRANSPARENT);
                is_cleared = true;
                needs_read = true;
            }
        } else if !is_cleared {
            surface.canvas().clear(Color::TRANSPARENT);
            last_rendered_key = None;
            is_cleared = true;
            needs_read = true;
        }

        // --- Output ---
        if needs_read {
            let _ = surface.read_pixels(&info, &mut pixel_buffer, row_bytes, (0, 0));
        }

        if stdout.write_all(&pixel_buffer).is_err() {
            break;
        }

        frame_count += 1;
    }

    stdout.flush()?;

    Ok(())
}

fn parse_line(line: &str) -> Option<Subtitle> {
    let parts: Vec<&str> = line.split('\t').collect();
    if parts.len() < 3 {
        return None;
    }

    let start = parts[0].parse().ok()?;
    let end = parts[1].parse().ok()?;
    let text = parts[2];

    let lines = text.split("   ").map(|s| s.to_string()).collect();

    Some(Subtitle { start, end, lines })
}

fn draw_subtitle(
    surface: &mut Surface,
    sub: &Subtitle,
    config: &Config,
    typeface: &Typeface,
    line_height: f32,
    shaper: &Shaper,
) {
    let canvas = surface.canvas();
    canvas.clear(Color::TRANSPARENT);

    // Shadow Setup
    let mut shadow_paint = Paint::default();
    shadow_paint.set_color(Color::from_argb(
        (config.shadow_opacity * 255.0) as u8,
        0,
        0,
        0,
    ));
    shadow_paint.set_anti_alias(true);
    if config.shadow_blur > 0.0 {
        let sigma = config.shadow_blur / 2.0;
        shadow_paint.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, sigma, false));
    }

    // Text Setup
    let mut text_paint = Paint::default();
    text_paint.set_color(Color::WHITE);
    text_paint.set_anti_alias(true);

    // Shadow Offset
    let rad = config.shadow_angle.to_radians();
    let off_x = config.shadow_distance * rad.cos();
    let off_y = config.shadow_distance * rad.sin();

    // Thread bold/italic depth across display lines so that a tag opened on
    // line 1 remains active on line 2, matching WebVTT cue-body semantics.
    let mut bold_depth: u32 = 0;
    let mut italic_depth: u32 = 0;

    for (i, line) in sub.lines.iter().enumerate() {
        let line_index_from_bottom = (sub.lines.len() - 1 - i) as f32;
        let y = config.baseline as f32 - (line_index_from_bottom * line_height);

        let runs;
        (runs, bold_depth, italic_depth) = parse_runs_stateful(line, bold_depth, italic_depth);

        // Shape every run up front with HarfBuzz — this both measures the
        // line (for centering) and produces the exact glyph blob to draw,
        // so measurement and rendering can never disagree.
        let shaped: Vec<(TextBlob, f32)> = runs
            .iter()
            .filter_map(|run| {
                let font = styled_font(typeface, config.font_size, run.bold, run.italic);
                shape_run(shaper, &run.text, &font)
            })
            .collect();

        let total_width: f32 = shaped.iter().map(|(_, width)| width).sum();
        let mut x = (config.width as f32 - total_width) / 2.0;

        for (blob, run_width) in &shaped {
            // Draw Shadow
            if config.shadow_opacity > 0.0 {
                canvas.draw_text_blob(blob, Point::new(x + off_x, y + off_y), &shadow_paint);
            }

            // Draw Text
            canvas.draw_text_blob(blob, Point::new(x, y), &text_paint);

            x += run_width;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: assert a run has the expected text and style flags.
    fn check(run: &TextRun, text: &str, bold: bool, italic: bool) {
        assert_eq!(run.text, text, "text mismatch");
        assert_eq!(run.bold, bold, "bold mismatch for {:?}", run.text);
        assert_eq!(run.italic, italic, "italic mismatch for {:?}", run.text);
    }

    // -----------------------------------------------------------------------
    // Basic cases
    // -----------------------------------------------------------------------

    #[test]
    fn plain_text() {
        // No tags → single unstyled run.
        let runs = parse_runs("Hello world");
        assert_eq!(runs.len(), 1);
        check(&runs[0], "Hello world", false, false);
    }

    #[test]
    fn empty_string() {
        // Empty input → no runs.
        let runs = parse_runs("");
        assert_eq!(runs.len(), 0);
    }

    #[test]
    fn only_tags_no_text() {
        // Tags with no text content between them → no runs.
        let runs = parse_runs("<b></b>");
        assert_eq!(runs.len(), 0);
    }

    // -----------------------------------------------------------------------
    // Single-style cases
    // -----------------------------------------------------------------------

    #[test]
    fn bold_only() {
        // <b>text</b> → one bold run.
        let runs = parse_runs("<b>bold</b>");
        assert_eq!(runs.len(), 1);
        check(&runs[0], "bold", true, false);
    }

    #[test]
    fn italic_only() {
        // <i>text</i> → one italic run.
        let runs = parse_runs("<i>italic</i>");
        assert_eq!(runs.len(), 1);
        check(&runs[0], "italic", false, true);
    }

    #[test]
    fn bold_around_plain() {
        // Text before and after bold span → three runs.
        let runs = parse_runs("Hello <b>world</b>!");
        assert_eq!(runs.len(), 3);
        check(&runs[0], "Hello ", false, false);
        check(&runs[1], "world", true, false);
        check(&runs[2], "!", false, false);
    }

    #[test]
    fn italic_around_plain() {
        let runs = parse_runs("see <i>note</i> here");
        assert_eq!(runs.len(), 3);
        check(&runs[0], "see ", false, false);
        check(&runs[1], "note", false, true);
        check(&runs[2], " here", false, false);
    }

    // -----------------------------------------------------------------------
    // Nesting order — must be identical
    // -----------------------------------------------------------------------

    #[test]
    fn bold_then_italic_nested() {
        // <b><i>T</i></b>  →  T is bold+italic.
        let runs = parse_runs("<b><i>T</i></b>");
        assert_eq!(runs.len(), 1);
        check(&runs[0], "T", true, true);
    }

    #[test]
    fn italic_then_bold_nested() {
        // <i><b>T</b></i>  →  same result; nesting order is irrelevant.
        let runs = parse_runs("<i><b>T</b></i>");
        assert_eq!(runs.len(), 1);
        check(&runs[0], "T", true, true);
    }

    // -----------------------------------------------------------------------
    // Wrong close order — must tolerate gracefully
    // -----------------------------------------------------------------------

    #[test]
    fn wrong_close_order_bi() {
        // <b><i>T</b></i>  — </b> arrives while italic is still open.
        // Both were open when T was accumulated, so T is bold+italic.
        // After </b>: bold_depth=0, italic_depth=1 (italic still logically open).
        // After </i>: italic_depth=0.
        let runs = parse_runs("<b><i>T</b></i>");
        assert_eq!(runs.len(), 1);
        check(&runs[0], "T", true, true);
    }

    #[test]
    fn wrong_close_order_ib() {
        // <i><b>T</i></b>  — symmetric to the above.
        let runs = parse_runs("<i><b>T</i></b>");
        assert_eq!(runs.len(), 1);
        check(&runs[0], "T", true, true);
    }

    #[test]
    fn wrong_close_order_with_tail() {
        // <b><i>A</b>B</i>  →  A=bold+italic, B=italic (bold depth dropped to 0).
        let runs = parse_runs("<b><i>A</b>B</i>");
        assert_eq!(runs.len(), 2);
        check(&runs[0], "A", true, true);
        check(&runs[1], "B", false, true);
    }

    // -----------------------------------------------------------------------
    // Unmatched tags
    // -----------------------------------------------------------------------

    #[test]
    fn unmatched_open_bold() {
        // <b>text  — no closing tag; style stays active to end of string.
        let runs = parse_runs("<b>bold forever");
        assert_eq!(runs.len(), 1);
        check(&runs[0], "bold forever", true, false);
    }

    #[test]
    fn unmatched_open_italic() {
        let runs = parse_runs("<i>italic forever");
        assert_eq!(runs.len(), 1);
        check(&runs[0], "italic forever", false, true);
    }

    #[test]
    fn unmatched_close_bold() {
        // </b> with nothing open → depth saturates at 0, no panic, text is plain.
        let runs = parse_runs("text</b>more");
        assert_eq!(runs.len(), 2);
        check(&runs[0], "text", false, false);
        check(&runs[1], "more", false, false);
    }

    #[test]
    fn unmatched_close_italic() {
        let runs = parse_runs("text</i>more");
        assert_eq!(runs.len(), 2);
        check(&runs[0], "text", false, false);
        check(&runs[1], "more", false, false);
    }

    #[test]
    fn unmatched_close_does_not_affect_open() {
        // <b>A</i>B</b>  — the stray </i> decrements italic_depth (already 0,
        // saturates).  Bold is unaffected; A and B are both bold.
        let runs = parse_runs("<b>A</i>B</b>");
        assert_eq!(runs.len(), 2);
        check(&runs[0], "A", true, false);
        check(&runs[1], "B", true, false);
    }

    // -----------------------------------------------------------------------
    // Double-open (key difference from boolean approach)
    // -----------------------------------------------------------------------

    #[test]
    fn double_open_bold() {
        // <b><b>T</b>more</b>  — first </b> drops depth to 1 (still bold),
        // second </b> drops to 0.  "more" must remain bold.
        // A simple bool flip would incorrectly make "more" plain.
        let runs = parse_runs("<b><b>T</b>more</b>");
        assert_eq!(runs.len(), 2);
        check(&runs[0], "T", true, false);
        check(&runs[1], "more", true, false); // depth counter keeps bold active
    }

    #[test]
    fn double_open_italic() {
        let runs = parse_runs("<i><i>T</i>more</i>");
        assert_eq!(runs.len(), 2);
        check(&runs[0], "T", false, true);
        check(&runs[1], "more", false, true);
    }

    #[test]
    fn double_open_bold_italic_mixed() {
        // <b><i><b>T</b>mid</i>tail</b>
        // After opening: bold_depth=2, italic_depth=1.
        // Flush always captures the state BEFORE the closing tag decrements:
        //   </b>  → flush "T"   as B+I (depths 2,1 → both >0), then bold_depth=1
        //   </i>  → flush "mid" as B+I (depths 1,1 → both >0), then italic_depth=0
        //   </b>  → flush "tail" as B   (depths 1,0), then bold_depth=0
        let runs = parse_runs("<b><i><b>T</b>mid</i>tail</b>");
        assert_eq!(runs.len(), 3);
        check(&runs[0], "T", true, true);
        check(&runs[1], "mid", true, true); // italic still open at flush time
        check(&runs[2], "tail", true, false);
    }

    #[test]
    fn state_threads_across_display_lines() {
        // In draw_subtitle, parse_runs_stateful is called for each display line
        // with the depths carried over from the previous line.
        // An unclosed <b> on line 1 MUST remain active on line 2.
        let (line1, b, i) = parse_runs_stateful("<b>bold and unclosed", 0, 0);
        assert_eq!(line1.len(), 1);
        check(&line1[0], "bold and unclosed", true, false);
        // depths thread into the next line
        let (line2, _, _) = parse_runs_stateful("still bold", b, i);
        assert_eq!(line2.len(), 1);
        check(&line2[0], "still bold", true, false); // bold carries over
    }

    #[test]
    fn state_resets_when_closed_before_line_break() {
        // If the tag IS properly closed before the line break, line 2 is plain.
        let (line1, b, i) = parse_runs_stateful("<b>bold</b>", 0, 0);
        assert_eq!(line1.len(), 1);
        check(&line1[0], "bold", true, false);
        assert_eq!(b, 0); // tag was closed
        let (line2, _, _) = parse_runs_stateful("plain", b, i);
        assert_eq!(line2.len(), 1);
        check(&line2[0], "plain", false, false);
    }

    // -----------------------------------------------------------------------
    // Mixed / real-world patterns
    // -----------------------------------------------------------------------

    #[test]
    fn mixed_segments() {
        // Alternating plain, bold, plain, italic, plain.
        let runs = parse_runs("plain <b>bold</b> middle <i>italic</i> end");
        assert_eq!(runs.len(), 5);
        check(&runs[0], "plain ", false, false);
        check(&runs[1], "bold", true, false);
        check(&runs[2], " middle ", false, false);
        check(&runs[3], "italic", false, true);
        check(&runs[4], " end", false, false);
    }

    #[test]
    fn bold_italic_adjacent() {
        // <b>A</b><i>B</i>  — two separate styled spans with no gap text.
        let runs = parse_runs("<b>A</b><i>B</i>");
        assert_eq!(runs.len(), 2);
        check(&runs[0], "A", true, false);
        check(&runs[1], "B", false, true);
    }

    #[test]
    fn unknown_tag_ignored_text_preserved() {
        // <c.color> is a WebVTT colour class tag; text content must be kept.
        let runs = parse_runs("a<c.color>b</c.color>c");
        assert_eq!(runs.len(), 3);
        check(&runs[0], "a", false, false);
        check(&runs[1], "b", false, false);
        check(&runs[2], "c", false, false);
    }

    #[test]
    fn unknown_tag_inside_bold() {
        // Unknown tag between bold tags must not disturb bold state.
        let runs = parse_runs("<b>A<c.x>B</c.x>C</b>");
        assert_eq!(runs.len(), 3);
        check(&runs[0], "A", true, false);
        check(&runs[1], "B", true, false);
        check(&runs[2], "C", true, false);
    }

    // -----------------------------------------------------------------------
    // parse_line
    // -----------------------------------------------------------------------

    #[test]
    fn parse_line_valid() {
        let sub = parse_line("1000\t2000\tHello world").unwrap();
        assert_eq!(sub.start, 1000);
        assert_eq!(sub.end, 2000);
        assert_eq!(sub.lines, vec!["Hello world"]);
    }

    #[test]
    fn parse_line_multiple_lines() {
        let sub = parse_line("0\t5000\tLine one   Line two").unwrap();
        assert_eq!(sub.lines.len(), 2);
        assert_eq!(sub.lines[0], "Line one");
        assert_eq!(sub.lines[1], "Line two");
    }

    #[test]
    fn parse_line_with_inline_tags() {
        // parse_line preserves tags verbatim; parse_runs handles them at render time.
        let sub = parse_line("0\t3000\t<b>Bold</b> text").unwrap();
        assert_eq!(sub.lines[0], "<b>Bold</b> text");
    }

    #[test]
    fn parse_line_too_few_fields() {
        assert!(parse_line("bad input").is_none());
        assert!(parse_line("1000\t2000").is_none());
    }

    #[test]
    fn parse_line_non_numeric_timestamps() {
        assert!(parse_line("abc\t2000\ttext").is_none());
        assert!(parse_line("1000\txyz\ttext").is_none());
    }
}
