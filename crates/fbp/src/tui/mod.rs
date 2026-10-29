//! The terminal front-end: terminal setup, the event loop, teardown.
//!
//! The state lives in `app`, the panels in `panels`, the tree in `layout`. The loop is the
//! htrk3 shape: draw, tick, wait for an event, repeat -- one input path, one frame.

pub mod app;
pub mod artcache;
pub mod blocks;
pub mod layout;
pub mod panels;
pub mod playback;
pub mod theme;

#[cfg(feature = "shots")]
pub mod shot;

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::crossterm::{cursor, execute, terminal};
use ratatui::Terminal;

use crate::config::Config;
use app::App;

/// How long the loop waits for input before ticking. The transport and the art fetch need
/// this to advance while nothing is pressed.
const TICK: Duration = Duration::from_millis(60);

pub fn run(
    db: PathBuf,
    config: Config,
    filter: Option<String>,
    autoplay: bool,
) -> Result<(), String> {
    let mut app = App::open(db, config, filter, autoplay)?;
    let mut terminal = setup()?;
    let outcome = event_loop(&mut terminal, &mut app);
    let restored = restore(&mut terminal);
    app.finish();
    outcome.and(restored)
}

fn setup() -> Result<Terminal<CrosstermBackend<io::Stdout>>, String> {
    terminal::enable_raw_mode().map_err(|e| e.to_string())?;
    let mut out = io::stdout();
    execute!(out, terminal::EnterAlternateScreen, cursor::Hide).map_err(|e| e.to_string())?;
    Terminal::new(CrosstermBackend::new(out)).map_err(|e| e.to_string())
}

fn restore(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<(), String> {
    execute!(
        terminal.backend_mut(),
        cursor::Show,
        terminal::LeaveAlternateScreen
    )
    .map_err(|e| e.to_string())?;
    terminal::disable_raw_mode().map_err(|e| e.to_string())
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<(), String> {
    loop {
        terminal
            .draw(|frame| app.draw(frame))
            .map_err(|e| e.to_string())?;
        app.tick();
        if app.quit {
            break;
        }
        if event::poll(TICK).map_err(|e| e.to_string())? {
            match event::read().map_err(|e| e.to_string())? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
                // A resize just changes the next frame's rects; nothing to invalidate.
                _ => {}
            }
        }
        if app.quit {
            break;
        }
    }
    Ok(())
}
