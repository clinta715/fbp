//! Half blocks: the terminal projection of a bitmap (SCOPE section 2).
//!
//! One cell carries two stacked pixels: the upper one as the glyph's foreground, the lower
//! one as its background, drawn as U+2580. So a 40x20-cell tile is a 40x40-pixel cover, in
//! truecolor, in any terminal -- which is the whole argument for art-as-cells: one art
//! pipeline feeds the terminal, the window and the CI screenshots.

use ratatui::style::Color;

/// The half block glyph: upper half of the cell takes the foreground colour.
pub const UPPER: &str = "\u{2580}";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HalfCell {
    pub top: Color,
    pub bottom: Color,
}

impl HalfCell {
    #[cfg(test)]
    pub fn is_flat(&self) -> bool {
        self.top == self.bottom
    }
}

/// Nearest-neighbour projection of an RGB image into `width * height` cells.
///
/// Nearest is deliberate: the source is already a 96px thumbnail and the destination is at
/// most ~60 cells, so a box filter would cost more than it shows. Deterministic, so a shot
/// is reproducible.
pub fn half_blocks(
    rgb: &[u8],
    src_width: u32,
    src_height: u32,
    width: u16,
    height: u16,
) -> Vec<Vec<HalfCell>> {
    let width = width as usize;
    let height = height as usize;
    if width == 0 || height == 0 || src_width == 0 || src_height == 0 {
        return Vec::new();
    }
    let src_width = src_width as usize;
    let src_height = src_height as usize;
    if rgb.len() < src_width * src_height * 3 {
        return Vec::new();
    }

    let mut out = Vec::with_capacity(height);
    for cell_y in 0..height {
        let mut row = Vec::with_capacity(width);
        for cell_x in 0..width {
            let x = cell_x * src_width / width;
            let top_y = (cell_y * 2) * src_height / (height * 2);
            let bottom_y = (cell_y * 2 + 1) * src_height / (height * 2);
            let top = pixel(rgb, src_width, x, top_y);
            let bottom = pixel(rgb, src_width, x, bottom_y);
            row.push(HalfCell { top, bottom });
        }
        out.push(row);
    }
    out
}

fn pixel(rgb: &[u8], src_width: usize, x: usize, y: usize) -> Color {
    let index = (y * src_width + x) * 3;
    Color::Rgb(rgb[index], rgb[index + 1], rgb[index + 2])
}

/// A flat colour tile, used when an album has no art: never a blank hole.
pub fn placeholder(width: u16, height: u16, background: Color, glyph: Color) -> Vec<Vec<HalfCell>> {
    (0..height)
        .map(|_| {
            (0..width)
                .map(|_| HalfCell {
                    top: background,
                    bottom: glyph,
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_by_two_becomes_one_cell_with_two_stacked_pixels() {
        // red green / blue white
        let rgb = vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255];
        let cells = half_blocks(&rgb, 2, 2, 1, 1);
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0][0].top, Color::Rgb(255, 0, 0));
        assert_eq!(cells[0][0].bottom, Color::Rgb(0, 0, 255));

        let wide = half_blocks(&rgb, 2, 2, 2, 1);
        assert_eq!(wide[0][1].top, Color::Rgb(0, 255, 0));
        assert_eq!(wide[0][1].bottom, Color::Rgb(255, 255, 255));
    }

    #[test]
    fn a_solid_image_projects_to_flat_cells() {
        let rgb = vec![7u8; 4 * 4 * 3];
        let cells = half_blocks(&rgb, 4, 4, 6, 3);
        assert_eq!(cells.len(), 3);
        assert!(cells.iter().flatten().all(|c| c.is_flat()));
    }

    #[test]
    fn a_short_buffer_projects_to_nothing_instead_of_panicking() {
        assert!(half_blocks(&[0, 0, 0], 4, 4, 10, 10).is_empty());
        assert!(half_blocks(&[0; 48], 4, 4, 0, 10).is_empty());
    }
}
