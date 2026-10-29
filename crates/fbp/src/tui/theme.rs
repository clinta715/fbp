//! The palette. Three rules, from the htrk3 UI strategy: every colour is a token (no
//! literals in panels), the terminal's own background is left alone in the cell grid, and
//! the windowed front-end paints its surface with `bg` so the two agree.

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub bg: Color,
    pub panel: Color,
    pub text: Color,
    pub dim: Color,
    pub accent: Color,
    pub selection_bg: Color,
    pub selection_fg: Color,
    pub art_bg: Color,
    pub bar: Color,
    pub warning: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            bg: Color::Rgb(0x14, 0x16, 0x1a),
            panel: Color::Rgb(0x1c, 0x1f, 0x25),
            text: Color::Rgb(0xd8, 0xd8, 0xd0),
            dim: Color::Rgb(0x76, 0x7a, 0x82),
            accent: Color::Rgb(0x8e, 0xc0, 0x7c),
            selection_bg: Color::Rgb(0x33, 0x3a, 0x44),
            selection_fg: Color::Rgb(0xf2, 0xf2, 0xe8),
            art_bg: Color::Rgb(0x24, 0x27, 0x2e),
            bar: Color::Rgb(0x2a, 0x2e, 0x36),
            warning: Color::Rgb(0xd0, 0x8a, 0x6a),
        }
    }
}

impl Theme {
    pub fn text_style(&self) -> Style {
        Style::default().fg(self.text)
    }

    pub fn dim_style(&self) -> Style {
        Style::default().fg(self.dim)
    }

    pub fn selection_style(&self, focused: bool) -> Style {
        if focused {
            Style::default().fg(self.selection_fg).bg(self.selection_bg)
        } else {
            Style::default().fg(self.text).bg(self.panel)
        }
    }

    pub fn title_style(&self, focused: bool) -> Style {
        let mut style = Style::default().fg(if focused { self.accent } else { self.dim });
        if focused {
            style = style.add_modifier(Modifier::BOLD);
        }
        style
    }
}
