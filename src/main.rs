use skia_safe::{
    AlphaType, BlurStyle, Color, ColorType, Data, Font, FontMgr, ImageInfo, MaskFilter, Paint,
    Point, Surface, surfaces,
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
    let font = Font::new(typeface, config.font_size);

    // 4. Prepare IO
    let stdin = io::stdin();
    let mut line_iter = stdin.lock().lines();
    let mut stdout = io::stdout();

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
                draw_subtitle(&mut surface, sub, &config, &font);
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
            // Read pixels from surface into our buffer
            let _ = surface.read_pixels(&info, &mut pixel_buffer, row_bytes, (0, 0));
        }

        if stdout.write_all(&pixel_buffer).is_err() {
            break;
        }

        frame_count += 1;
    }

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

fn draw_subtitle(surface: &mut Surface, sub: &Subtitle, config: &Config, font: &Font) {
    let canvas = surface.canvas();
    canvas.clear(Color::TRANSPARENT);

    let line_height = font.spacing() * config.line_height_multiplier;

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
        // Convert radius to sigma
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

    for (i, line) in sub.lines.iter().enumerate() {
        let line_index_from_bottom = (sub.lines.len() - 1 - i) as f32;
        let y = config.baseline as f32 - (line_index_from_bottom * line_height);

        let width = font.measure_text(line, Some(&text_paint)).0;
        let x = (config.width as f32 - width) / 2.0;

        // Draw Shadow
        if config.shadow_opacity > 0.0 {
            canvas.draw_str(line, Point::new(x + off_x, y + off_y), font, &shadow_paint);
        }

        // Draw Text
        canvas.draw_str(line, Point::new(x, y), font, &text_paint);
    }
}
