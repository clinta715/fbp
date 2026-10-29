//! Headless frame capture (feature `shots`): render into a `TestBackend`, rasterise the
//! cell grid with an 8x8 bitmap font, write a PNG.
//!
//! Why it exists: a TUI's surfaces are a cell grid, so a screenshot is a deterministic
//! function of the state -- no window, no GPU, no clock. That gives documentation images, a
//! visual review of every panel without a terminal, and an image-level regression check
//! next to the text-level assertions.
//!
//! It is not a terminal emulator: wide glyphs, combining marks and underline are not
//! reproduced; a glyph the font does not have is drawn blank. Half blocks *are* reproduced
//! (`BLOCK_FONTS`), which is why cover art shows up in these images at all.

use std::path::{Path, PathBuf};

use font8x8::{UnicodeFonts, BASIC_FONTS, BLOCK_FONTS, BOX_FONTS, LATIN_FONTS};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;

use super::app::App;
use super::theme::Theme;

const GLYPH: u32 = 8;
/// Two extra pixel rows under each glyph, so text does not touch between lines.
const LINE_SPACING: u32 = 2;

pub struct Shot {
    pub cols: u16,
    pub rows: u16,
    pub scale: u32,
}

impl Shot {
    pub const SIZES: [Shot; 2] = [
        Shot {
            cols: 80,
            rows: 24,
            scale: 2,
        },
        Shot {
            cols: 200,
            rows: 60,
            scale: 2,
        },
    ];

    pub fn file_name(&self) -> String {
        format!("{}x{}.png", self.cols, self.rows)
    }
}

/// Capture every size into `dir`. Art is preloaded first, because the cache is asynchronous
/// and a screenshot taken immediately would show placeholders.
pub fn capture_all(app: &mut App, dir: &Path) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    app.reset_for_capture();
    app.preload_art(16);
    let mut written = Vec::new();

    for shot in Shot::SIZES {
        let path = dir.join(shot.file_name());
        capture(app, &shot, &path)?;
        written.push(path);
    }

    // The albums tab, which is where the half-block cover art lives.
    app.set_tabs(0, 1);
    for shot in Shot::SIZES {
        let path = dir.join(format!("albums-{}", shot.file_name()));
        capture(app, &shot, &path)?;
        written.push(path);
    }
    app.set_tabs(0, 0);

    // The properties tab: the tag editor.
    app.set_tabs(1, 1);
    app.focus_properties();
    let path = dir.join("properties-200x60.png");
    capture(app, &Shot::SIZES[1], &path)?;
    written.push(path);
    app.set_tabs(1, 0);
    Ok(written)
}

pub fn capture(app: &mut App, shot: &Shot, path: &Path) -> Result<(), String> {
    let backend = TestBackend::new(shot.cols, shot.rows);
    let mut terminal = Terminal::new(backend).map_err(|e| e.to_string())?;
    terminal
        .draw(|frame| app.draw(frame))
        .map_err(|e| e.to_string())?;
    let buffer = terminal.backend().buffer();
    let rgb = rasterise(buffer, shot.scale, app.theme());
    write_png(path, &rgb, shot.cols, shot.rows, shot.scale)
}

fn rasterise(buffer: &Buffer, scale: u32, theme: Theme) -> Vec<u8> {
    let cols = buffer.area.width as u32;
    let rows = buffer.area.height as u32;
    let cell_w = GLYPH * scale;
    let cell_h = (GLYPH + LINE_SPACING) * scale;
    let width = cols * cell_w;
    let height = rows * cell_h;
    let mut pixels = vec![0u8; (width * height * 3) as usize];

    for y in 0..rows {
        for x in 0..cols {
            let cell = &buffer[(x as u16, y as u16)];
            let mut fg = rgb_of(cell.fg, theme.text);
            let mut bg = rgb_of(cell.bg, theme.bg);
            if cell.modifier.contains(Modifier::REVERSED) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if cell.modifier.contains(Modifier::DIM) {
                fg = shade(fg, 0.55);
            }
            if cell.modifier.contains(Modifier::BOLD) {
                fg = shade(fg, 1.35);
            }

            let origin_x = x * cell_w;
            let origin_y = y * cell_h;
            fill(&mut pixels, width, origin_x, origin_y, cell_w, cell_h, bg);

            let Some(glyph) = cell.symbol().chars().next().and_then(lookup) else {
                continue;
            };
            for (row, bits) in glyph.iter().enumerate() {
                for column in 0..GLYPH {
                    if bits & (1 << column) == 0 {
                        continue;
                    }
                    fill(
                        &mut pixels,
                        width,
                        origin_x + column * scale,
                        origin_y + row as u32 * scale,
                        scale,
                        scale,
                        fg,
                    );
                }
            }
        }
    }
    pixels
}

#[allow(clippy::type_complexity)]
fn lookup(c: char) -> Option<[u8; 8]> {
    BASIC_FONTS
        .get(c)
        .or_else(|| LATIN_FONTS.get(c))
        .or_else(|| BLOCK_FONTS.get(c))
        .or_else(|| BOX_FONTS.get(c))
}

fn fill(pixels: &mut [u8], width: u32, x: u32, y: u32, w: u32, h: u32, color: (u8, u8, u8)) {
    for py in y..(y + h) {
        for px in x..(x + w) {
            if px >= width {
                break;
            }
            let index = ((py * width + px) * 3) as usize;
            if index + 2 < pixels.len() {
                pixels[index] = color.0;
                pixels[index + 1] = color.1;
                pixels[index + 2] = color.2;
            }
        }
    }
}

fn rgb_of(color: Color, fallback: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Reset => rgb_of(fallback, Color::Rgb(0x14, 0x16, 0x1a)),
        Color::Black => (0, 0, 0),
        Color::White => (255, 255, 255),
        Color::Red => (0xc0, 0x50, 0x50),
        Color::Green => (0x50, 0xc0, 0x50),
        Color::Blue => (0x50, 0x50, 0xc0),
        Color::Yellow => (0xc0, 0xc0, 0x50),
        Color::Magenta => (0xc0, 0x50, 0xc0),
        Color::Cyan => (0x50, 0xc0, 0xc0),
        Color::Gray => (0x90, 0x90, 0x90),
        Color::DarkGray => (0x50, 0x50, 0x50),
        Color::LightRed => (0xf0, 0x70, 0x70),
        Color::LightGreen => (0x70, 0xf0, 0x70),
        Color::LightBlue => (0x70, 0x70, 0xf0),
        Color::LightYellow => (0xf0, 0xf0, 0x70),
        Color::LightMagenta => (0xf0, 0x70, 0xf0),
        Color::LightCyan => (0x70, 0xf0, 0xf0),
        Color::Indexed(_) => rgb_of(fallback, Color::Rgb(0x80, 0x80, 0x80)),
    }
}

fn shade((r, g, b): (u8, u8, u8), factor: f32) -> (u8, u8, u8) {
    let apply = |v: u8| (v as f32 * factor).clamp(0.0, 255.0) as u8;
    (apply(r), apply(g), apply(b))
}

fn write_png(path: &Path, pixels: &[u8], cols: u16, rows: u16, scale: u32) -> Result<(), String> {
    let width = cols as u32 * GLYPH * scale;
    let height = rows as u32 * (GLYPH + LINE_SPACING) * scale;
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(pixels).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    #[test]
    fn the_rasteriser_paints_glyph_pixels_in_the_cell_colours() {
        let mut buffer = Buffer::empty(ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
        });
        buffer[(0, 0)].set_symbol("#");
        buffer[(0, 0)].set_fg(Color::Rgb(255, 0, 0));
        buffer[(0, 0)].set_bg(Color::Rgb(0, 0, 255));
        let theme = Theme::default();
        let rgb = rasterise(&buffer, 1, theme);

        // Cell (0,0) top-left pixel is the glyph background or the glyph's first pixel.
        let width = 2 * GLYPH;
        let at = |x: u32, y: u32| {
            let index = ((y * width + x) * 3) as usize;
            (rgb[index], rgb[index + 1], rgb[index + 2])
        };
        assert_eq!(
            at(7, 0),
            (0, 0, 255),
            "the cell's background fills the corner"
        );
        assert_eq!(at(7, 7), (0, 0, 255), "and the last glyph row");
        // Some pixel inside the glyph box must be the foreground red.
        let painted = (0..GLYPH).any(|y| (0..GLYPH).any(|x| at(x, y) == (255, 0, 0)));
        assert!(painted, "the glyph pixels use the foreground colour");
    }

    /// The acceptance test for the whole projection: a real library, a real cover, and the
    /// albums panel rasterised to a PNG in which the cover's colours actually appear.
    #[test]
    fn the_albums_panel_renders_real_cover_art() {
        use crate::config::Config;

        let root = std::env::temp_dir().join(format!("fbp-shot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp dir");
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fbp-index/fixtures");
        for name in ["a1.mp3", "a2.mp3", "b1.flac", "cover.jpg"] {
            std::fs::copy(fixtures.join(name), root.join(name)).expect("copy fixture");
        }

        let db = root.join("library.db");
        let mut library = fbp_index::Library::open(&db).expect("library");
        let progress = fbp_index::ScanProgress::new();
        library
            .scan(&root, fbp_index::ScanOptions::default(), &progress)
            .expect("scan");
        drop(library);

        let mut app = App::open(
            db,
            Config {
                art: true,
                ..Config::default()
            },
            None,
            false,
        )
        .expect("app");
        assert!(!app.albums.is_empty(), "the fixture library has albums");
        app.set_tabs(0, 1);
        app.preload_art(4);

        let path = root.join("albums.png");
        capture(
            &mut app,
            &Shot {
                cols: 120,
                rows: 40,
                scale: 1,
            },
            &path,
        )
        .expect("capture");

        let (colors, decoded) = read_png(&path);
        assert!(
            decoded.0 >= 120 * 8 && decoded.1 >= 40 * 10,
            "image covers the grid"
        );
        assert!(
            colors > 200,
            "cover art should produce many colours, got {colors}"
        );
    }

    /// Switching the tab stack must change the frame: a capture that ignores the tab would
    /// make the album wall indistinguishable from the table.
    #[test]
    fn the_tab_stack_changes_what_is_captured() {
        use crate::config::Config;

        let root = std::env::temp_dir().join(format!("fbp-shot-tabs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp dir");
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fbp-index/fixtures");
        for name in ["a1.mp3", "a2.mp3", "cover.jpg"] {
            std::fs::copy(fixtures.join(name), root.join(name)).expect("copy fixture");
        }
        let db = root.join("library.db");
        let mut library = fbp_index::Library::open(&db).expect("library");
        library
            .scan(
                &root,
                fbp_index::ScanOptions::default(),
                &fbp_index::ScanProgress::new(),
            )
            .expect("scan");
        drop(library);

        let mut app = App::open(db, Config::default(), None, false).expect("app");
        app.layout = crate::tui::layout::Node::default_layout();
        app.preload_art(4);

        let shot = Shot {
            cols: 100,
            rows: 30,
            scale: 1,
        };
        let table = root.join("table.png");
        capture(&mut app, &shot, &table).expect("table");
        app.set_tabs(0, 1);
        let albums = root.join("albums.png");
        capture(&mut app, &shot, &albums).expect("albums");

        let table_bytes = std::fs::read(&table).expect("read");
        let album_bytes = std::fs::read(&albums).expect("read");
        assert_ne!(
            table_bytes, album_bytes,
            "the albums tab must render differently"
        );
        assert!(
            !app.albums.is_empty(),
            "the fixture library has albums to draw"
        );
    }

    /// Count distinct RGB values in a PNG; also returns the dimensions.
    fn read_png(path: &Path) -> (usize, (u32, u32)) {
        let file = std::fs::File::open(path).expect("open png");
        let decoder = png::Decoder::new(std::io::BufReader::new(file));
        let mut reader = decoder.read_info().expect("png info");
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).expect("png frame");
        let bytes = &buf[..info.buffer_size()];
        let mut seen = std::collections::HashSet::new();
        for index in (0..bytes.len().saturating_sub(2)).step_by(3) {
            seen.insert((bytes[index], bytes[index + 1], bytes[index + 2]));
        }
        (seen.len(), (info.width, info.height))
    }

    #[test]
    fn block_glyphs_are_available_so_art_rasterises() {
        assert!(
            lookup('\u{2580}').is_some(),
            "half blocks must be in the font"
        );
        assert!(lookup('A').is_some());
    }

    #[test]
    fn reversed_swaps_the_colours() {
        let mut buffer = Buffer::empty(ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        });
        buffer[(0, 0)].set_symbol(" ");
        buffer[(0, 0)].set_style(Style::default().add_modifier(Modifier::REVERSED));
        let rgb = rasterise(&buffer, 1, Theme::default());
        let theme = Theme::default();
        let expected = rgb_of(theme.text, theme.text);
        assert_eq!((rgb[0], rgb[1], rgb[2]), expected);
    }
}
