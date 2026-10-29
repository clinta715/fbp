//! The panels. Each takes a rect and draws; none of them queries state another panel owns.
//!
//! Every list is virtualised the same way: the visible slice only. That is not an
//! optimisation, it is the rule that keeps a 100k-row table interactive (SCOPE risk 1).

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::app::App;
use super::blocks;
use super::layout::PanelKind;

pub fn draw(frame: &mut Frame, app: &mut App, kind: PanelKind, area: Rect, focused: bool) {
    if area.width < 4 || area.height < 3 {
        return;
    }
    match kind {
        PanelKind::Tree => draw_tree(frame, app, area, focused),
        PanelKind::Table => draw_table(frame, app, area, focused),
        PanelKind::Albums => draw_albums(frame, app, area, focused),
        PanelKind::NowPlaying => draw_now_playing(frame, app, area, focused),
        PanelKind::Properties => draw_properties(frame, app, area, focused),
        PanelKind::Console => draw_console(frame, app, area, focused),
    }
}

fn panel_block(app: &App, kind: PanelKind, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused {
            app.theme.accent
        } else {
            app.theme.bar
        }))
        .title(Span::styled(
            format!(" {} ", kind.label()),
            app.theme.title_style(focused),
        ))
        .style(Style::default().bg(app.theme.panel))
}

fn clip(text: &str, width: usize) -> String {
    let mut out: String = text.chars().take(width).collect();
    if text.chars().count() > width && width > 0 {
        out.pop();
        out.push('\u{2026}');
    }
    out
}

fn pad(text: &str, width: usize) -> String {
    let mut out = clip(text, width);
    let len = out.chars().count();
    if len < width {
        out.push_str(&" ".repeat(width - len));
    }
    out
}

// ---- tree -------------------------------------------------------------------------

fn draw_tree(frame: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let block = panel_block(app, PanelKind::Tree, focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let height = inner.height as usize;
    let total = app.rows.len();
    let mut lines: Vec<Line> = Vec::with_capacity(height);

    // Row 0 is "everything"; the artists follow, so Enter on row 0 clears the filter.
    let label = format!("All tracks ({total})");
    let style = if app.artist_sel == 0 {
        app.theme.selection_style(focused)
    } else {
        app.theme.text_style()
    };
    lines.push(Line::from(Span::styled(
        pad(&label, inner.width as usize),
        style,
    )));

    let scroll = app.artist_sel.saturating_sub(height.saturating_sub(1));
    for (index, (artist, count)) in app
        .artists
        .iter()
        .enumerate()
        .skip(scroll)
        .take(height.saturating_sub(1))
    {
        let selected = index + 1 == app.artist_sel;
        let active = app.filter == *artist;
        let style = if selected {
            app.theme.selection_style(focused)
        } else if active {
            Style::default().fg(app.theme.accent)
        } else {
            app.theme.text_style()
        };
        let text = format!("{} ({count})", artist);
        lines.push(Line::from(Span::styled(
            pad(&text, inner.width as usize),
            style,
        )));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

// ---- table ------------------------------------------------------------------------

fn draw_table(frame: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let block = panel_block(app, PanelKind::Table, focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.rows.is_empty() {
        let hint = if app.filter.is_empty() {
            "nothing indexed -- press S to scan"
        } else {
            "no tracks match the filter"
        };
        frame.render_widget(Paragraph::new(hint).style(app.theme.dim_style()), inner);
        return;
    }

    let height = inner.height as usize;
    let width = inner.width as usize;
    // Keep the selection visible, then render only that window.
    if app.row_sel < app.row_scroll {
        app.row_scroll = app.row_sel;
    }
    if app.row_sel >= app.row_scroll + height {
        app.row_scroll = app.row_sel + 1 - height;
    }

    let time_width = 6;
    let track_width = 3;
    let artist_width = (width * 18 / 100).max(8);
    let album_width = (width * 22 / 100).max(10);
    let title_width = width
        .saturating_sub(time_width + track_width + artist_width + album_width + 4)
        .max(6);

    let mut lines = Vec::with_capacity(height);
    for offset in 0..height {
        let index = app.row_scroll + offset;
        let Some(row) = app.rows.get(index) else {
            break;
        };
        let selected = index == app.row_sel;
        let playing = Some(row.id) == app.playing_id;
        let marker = if playing { "\u{25b6}" } else { " " };
        let text = format!(
            "{marker}{:>track_width$} {} {} {} {:>time_width$}",
            row.track_no
                .map(|n| n.to_string())
                .unwrap_or_else(|| "\u{2014}".to_string()),
            pad(row.display_title(), title_width),
            pad(row.artist.as_deref().unwrap_or("\u{2014}"), artist_width),
            pad(row.album.as_deref().unwrap_or("\u{2014}"), album_width),
            row.clock(),
        );
        let style = if selected {
            app.theme.selection_style(focused)
        } else if playing {
            Style::default().fg(app.theme.accent)
        } else {
            app.theme.text_style()
        };
        lines.push(Line::from(Span::styled(clip(&text, width), style)));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

// ---- albums -----------------------------------------------------------------------

fn draw_albums(frame: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let block = panel_block(app, PanelKind::Albums, focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.albums.is_empty() {
        frame.render_widget(
            Paragraph::new("no albums -- press S to scan").style(app.theme.dim_style()),
            inner,
        );
        return;
    }

    let tile_width: u16 = 22;
    let tile_height: u16 = 13;
    let columns = (inner.width / tile_width).max(1) as usize;
    let rows = (inner.height / tile_height).max(1) as usize;
    let per_page = columns * rows;

    // Scroll by page so the selected album is always on screen.
    let page = app.album_sel / per_page;
    let first_page = page * per_page;
    let start = first_page;

    let mut cursor_y = inner.y;
    for grid_row in 0..rows {
        if cursor_y + tile_height > inner.y + inner.height {
            break;
        }
        let mut cursor_x = inner.x;
        for grid_column in 0..columns {
            let index = start + grid_row * columns + grid_column;
            let Some(album) = app.albums.get(index).cloned() else {
                break;
            };
            let tile = Rect {
                x: cursor_x,
                y: cursor_y,
                width: tile_width.min(inner.x + inner.width - cursor_x),
                height: tile_height,
            };
            draw_album_tile(frame, app, &album, tile, index == app.album_sel, focused);
            cursor_x += tile_width;
            if cursor_x >= inner.x + inner.width {
                break;
            }
        }
        cursor_y += tile_height;
    }
}

fn draw_album_tile(
    frame: &mut Frame,
    app: &mut App,
    album: &fbp_index::AlbumRow,
    tile: Rect,
    selected: bool,
    focused: bool,
) {
    let art_width = tile.width.saturating_sub(1).max(1);
    let art_height = tile.height.saturating_sub(3).max(1);

    let art = app.art.get(album.albumartist.as_deref(), &album.album);
    let cells = match &art {
        Some(art) => blocks::half_blocks(&art.rgb, art.width, art.height, art_width, art_height),
        None => blocks::placeholder(
            art_width,
            art_height,
            app.theme.art_bg,
            if app
                .art
                .is_pending(album.albumartist.as_deref(), &album.album)
            {
                app.theme.panel
            } else {
                app.theme.bar
            },
        ),
    };

    let buffer = frame.buffer_mut();
    for (y, row) in cells.iter().enumerate() {
        if (y as u16) >= art_height {
            break;
        }
        for (x, cell) in row.iter().enumerate() {
            if (x as u16) >= art_width {
                break;
            }
            let position = (tile.x + x as u16, tile.y + y as u16);
            let target = &mut buffer[position];
            target.set_symbol(blocks::UPPER);
            target.set_fg(cell.top);
            target.set_bg(cell.bottom);
        }
    }

    let text_y = tile.y + art_height;
    let title_style = if selected {
        Style::default()
            .fg(app.theme.selection_fg)
            .bg(app.theme.selection_bg)
    } else if focused {
        app.theme.text_style()
    } else {
        Style::default().fg(app.theme.dim)
    };
    let width = tile.width as usize;
    let title = clip(&album.album, width);
    let subtitle = clip(
        &format!(
            "{}  {}",
            album.artist_label(),
            album.year.map(|y| y.to_string()).unwrap_or_default()
        ),
        width,
    );

    let lines = vec![
        Line::from(Span::styled(pad(&title, width), title_style)),
        Line::from(Span::styled(pad(&subtitle, width), app.theme.dim_style())),
        Line::from(Span::styled(
            pad(&format!("{} tracks", album.tracks), width),
            app.theme.dim_style(),
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines),
        Rect {
            x: tile.x,
            y: text_y,
            width: tile.width,
            height: 3,
        },
    );
}

// ---- properties -------------------------------------------------------------------

/// The tag editor: one row per field, the selected row editable in place. It does not write
/// anything itself -- Enter hands the patch to `App::commit_field`, which goes through the
/// journalled writer, so an edit here is one undo away like every other write.
fn draw_properties(frame: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let block = panel_block(app, PanelKind::Properties, focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(row) = app.rows.get(app.row_sel).cloned() else {
        frame.render_widget(
            Paragraph::new("select a track to edit its tags").style(app.theme.dim_style()),
            inner,
        );
        return;
    };

    if inner.width < 12 || inner.height < 3 {
        return;
    }
    let label_width = 13.min(inner.width as usize / 2);
    let value_width = (inner.width as usize).saturating_sub(label_width + 1);
    let mut lines = Vec::with_capacity(fbp_core::tag::Field::ALL.len());

    for (index, field) in fbp_core::tag::Field::ALL.iter().enumerate() {
        let selected = index == app.props_sel;
        let editing = app.props_editing.as_ref().is_some_and(|(f, _)| f == field);
        let value =
            if let Some((_, buffer)) = app.props_editing.as_ref().filter(|(f, _)| f == field) {
                format!("{buffer}\u{2588}")
            } else {
                row.field(*field).unwrap_or_else(|| "\u{2014}".to_string())
            };
        let style = if editing {
            Style::default()
                .fg(app.theme.selection_fg)
                .bg(app.theme.selection_bg)
        } else if selected {
            app.theme.selection_style(focused)
        } else {
            app.theme.text_style()
        };
        let label_style = if selected {
            style
        } else {
            app.theme.dim_style()
        };
        lines.push(Line::from(vec![
            Span::styled(pad(field.label(), label_width), label_style),
            Span::styled(" ", app.theme.dim_style()),
            Span::styled(pad(&value, value_width), style),
        ]));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

// ---- now playing ------------------------------------------------------------------

fn draw_now_playing(frame: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let block = panel_block(app, PanelKind::NowPlaying, focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let current = app
        .playing_id
        .and_then(|id| app.rows.iter().find(|row| row.id == id).cloned())
        .or_else(|| app.rows.get(app.row_sel).cloned());

    let Some(row) = current else {
        frame.render_widget(
            Paragraph::new("nothing selected").style(app.theme.dim_style()),
            inner,
        );
        return;
    };

    let show_art = app.config.art && inner.width >= 30 && inner.height >= 10;
    let (art_area, text_area) = if show_art {
        let side = inner.height.min(inner.width / 2).max(8);
        (
            Some(Rect {
                x: inner.x,
                y: inner.y,
                width: side,
                height: side,
            }),
            Rect {
                x: inner.x + side + 1,
                y: inner.y,
                width: inner.width.saturating_sub(side + 1),
                height: inner.height,
            },
        )
    } else {
        (None, inner)
    };

    if let Some(art_area) = art_area {
        let art = app.art.get(
            row.albumartist.as_deref(),
            row.album.as_deref().unwrap_or(""),
        );
        let cells = match &art {
            Some(art) => blocks::half_blocks(
                &art.rgb,
                art.width,
                art.height,
                art_area.width,
                art_area.height,
            ),
            None => blocks::placeholder(
                art_area.width,
                art_area.height,
                app.theme.art_bg,
                app.theme.bar,
            ),
        };
        let buffer = frame.buffer_mut();
        for (y, line) in cells.iter().enumerate() {
            for (x, cell) in line.iter().enumerate() {
                let position = (art_area.x + x as u16, art_area.y + y as u16);
                if position.0 >= art_area.x + art_area.width || (y as u16) >= art_area.height {
                    break;
                }
                let target = &mut buffer[position];
                target.set_symbol(blocks::UPPER);
                target.set_fg(cell.top);
                target.set_bg(cell.bottom);
            }
        }
    }

    let width = text_area.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        clip(row.display_title(), width),
        Style::default()
            .fg(app.theme.text)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        clip(row.artist.as_deref().unwrap_or("\u{2014}"), width),
        Style::default().fg(app.theme.accent),
    )));
    lines.push(Line::from(Span::styled(
        clip(
            &format!(
                "{}{}",
                row.album.as_deref().unwrap_or("\u{2014}"),
                row.year.map(|y| format!(" ({y})")).unwrap_or_default()
            ),
            width,
        ),
        app.theme.dim_style(),
    )));
    lines.push(Line::from(Span::styled(
        clip(
            &format!(
                "{} \u{b7} {} Hz \u{b7} {} kbps",
                row.codec.as_deref().unwrap_or("?"),
                row.sample_rate.unwrap_or(0),
                row.bitrate.unwrap_or(0)
            ),
            width,
        ),
        app.theme.dim_style(),
    )));

    if text_area.height as usize > lines.len() + 2 {
        lines.push(Line::from(""));
        let position = app.playback.position_ms();
        let duration = row.duration_ms.unwrap_or(0).max(0) as u64;
        let fraction = if duration > 0 {
            (position as f32 / duration as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let bar_width = width.saturating_sub(14).max(4);
        let filled = (fraction * bar_width as f32).round() as usize;
        lines.push(Line::from(vec![
            Span::styled(
                "\u{2501}".repeat(filled),
                Style::default().fg(app.theme.accent),
            ),
            Span::styled(
                "\u{2500}".repeat(bar_width - filled),
                Style::default().fg(app.theme.bar),
            ),
            Span::styled(
                format!(" {} / {}", clock(position), clock(duration)),
                app.theme.dim_style(),
            ),
        ]));
        let state = if !app.playback.is_active() {
            "stopped"
        } else if app.playback.paused() {
            "paused"
        } else {
            "playing"
        };
        lines.push(Line::from(Span::styled(
            clip(
                &format!("{state}  vol {:.0}%", app.playback.volume * 100.0),
                width,
            ),
            app.theme.dim_style(),
        )));
    }

    frame.render_widget(Paragraph::new(lines), text_area);
}

// ---- console ----------------------------------------------------------------------

fn draw_console(frame: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let block = panel_block(app, PanelKind::Console, focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let height = inner.height as usize;
    let width = inner.width as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(height);

    if let Some((scanned, total, errors)) = app.scan_state() {
        let fraction = if total > 0 {
            scanned as f32 / total as f32
        } else {
            0.0
        };
        let bar_width = width.saturating_sub(20).max(4);
        let filled = (fraction * bar_width as f32).round() as usize;
        lines.push(Line::from(vec![
            Span::styled("scan ", Style::default().fg(app.theme.warning)),
            Span::styled(
                "\u{2588}".repeat(filled),
                Style::default().fg(app.theme.accent),
            ),
            Span::styled(
                "\u{2591}".repeat(bar_width - filled),
                Style::default().fg(app.theme.bar),
            ),
            Span::styled(
                format!(" {scanned}/{total} e{errors}"),
                app.theme.dim_style(),
            ),
        ]));
    }

    let room = height.saturating_sub(lines.len());
    let log: Vec<&String> = app
        .log
        .iter()
        .skip(app.log.len().saturating_sub(room))
        .collect();
    for entry in log {
        lines.push(Line::from(Span::styled(
            clip(entry, width),
            app.theme.dim_style(),
        )));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn clock(ms: u64) -> String {
    format!("{}:{:02}", ms / 60_000, (ms / 1_000) % 60)
}
