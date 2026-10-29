//! The application: state, keys, and the one frame.
//!
//! One input path (`handle_key`), one per-frame body (`tick`), one frame (`draw`) -- the
//! htrk3 invariant that keeps a second front-end from becoming a second app. A panel never
//! queries the database for something another panel owns; it reads `App` state.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use fbp_core::tag::{Field, Patch};
use fbp_core::Track;
use fbp_index::{AlbumRow, Library, Row, ScanOptions, ScanProgress, ScanReport, Search, Sort};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;
use serde::{Deserialize, Serialize};

use super::artcache::ArtCache;
use super::layout::{Axis, Node, PanelKind};
use super::panels;
use super::playback::Playback;
use super::theme::Theme;
use crate::config::{self, Config};

const LOG_LINES: usize = 200;

/// A destructive edit waiting for a y/N: applying one value to every filtered track is a
/// single keystroke away from being a single keystroke too many.
struct Pending {
    patch: Patch,
    paths: Vec<PathBuf>,
    label: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Session {
    focus: usize,
    filter: String,
    sort: String,
    desc: bool,
    node: Option<Node>,
}

pub struct App {
    pub library: Library,
    pub db_path: PathBuf,
    pub config: Config,
    pub theme: Theme,
    pub layout: Node,
    /// Index into `layout.leaves()`.
    pub focus: usize,
    pub layout_mode: bool,
    pub filter: String,
    pub filter_editing: bool,
    filter_before: String,
    pub sort: Sort,
    pub desc: bool,

    pub rows: Arc<Vec<Row>>,
    pub row_sel: usize,
    pub row_scroll: usize,
    pub artists: Vec<(String, usize)>,
    pub artist_sel: usize,
    pub albums: Arc<Vec<AlbumRow>>,
    pub album_sel: usize,

    /// Which tag field the Properties panel has selected, and the buffer being typed.
    pub props_sel: usize,
    pub props_editing: Option<(Field, String)>,
    last_edit: Option<(Field, String)>,
    pending: Option<Pending>,
    pub undo_depth: usize,

    pub art: ArtCache,
    pub playback: Playback,
    queue_ids: Vec<i64>,
    pub playing_id: Option<i64>,

    pub log: VecDeque<String>,
    pub scan: Option<Arc<ScanProgress>>,
    scan_thread: Option<std::thread::JoinHandle<Result<ScanReport, String>>>,
    pub quit: bool,
    pub redraw: bool,
}

impl App {
    pub fn open(
        db_path: PathBuf,
        config: Config,
        filter: Option<String>,
        autoplay: bool,
    ) -> Result<Self, String> {
        let library = Library::open(&db_path).map_err(|e| e.to_string())?;
        let session = read_session();

        let layout = session
            .node
            .clone()
            .or_else(|| config.load_layout(&config.layout))
            .unwrap_or_else(Node::default_layout);

        let mut app = Self {
            art: ArtCache::spawn(db_path.clone(), config.art),
            db_path,
            config: config.clone(),
            theme: Theme::default(),
            layout,
            focus: session.focus,
            layout_mode: false,
            filter: filter.clone().unwrap_or(session.filter),
            filter_editing: false,
            filter_before: String::new(),
            sort: sort_from_key(&session.sort),
            desc: session.desc,
            rows: Arc::new(Vec::new()),
            row_sel: 0,
            row_scroll: 0,
            artists: Vec::new(),
            artist_sel: 0,
            albums: Arc::new(Vec::new()),
            album_sel: 0,
            props_sel: 0,
            props_editing: None,
            last_edit: None,
            pending: None,
            undo_depth: 0,
            playback: Playback::new(config.volume),
            queue_ids: Vec::new(),
            playing_id: None,
            log: VecDeque::new(),
            scan: None,
            scan_thread: None,
            quit: false,
            redraw: true,
            library,
        };
        app.focus = app.focus.min(app.layout.leaves().len().saturating_sub(1));
        app.reload_all();
        app.log(format!(
            "{} tracks, {} albums -- press ? for keys",
            app.rows.len(),
            app.albums.len()
        ));
        if autoplay && !app.rows.is_empty() {
            app.play_from_selection();
        }
        Ok(app)
    }

    // ---- state ----------------------------------------------------------------

    pub fn focused(&self) -> PanelKind {
        self.layout
            .leaves()
            .get(self.focus)
            .copied()
            .unwrap_or(PanelKind::Table)
    }

    pub fn log(&mut self, message: impl Into<String>) {
        if self.log.len() >= LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(message.into());
        self.redraw = true;
    }

    fn reload_all(&mut self) {
        self.reload_rows();
        self.albums = Arc::new(self.library.albums().unwrap_or_default());
        self.artists = self.library.artists().unwrap_or_default();
        self.clamp_selection();
    }

    fn reload_rows(&mut self) {
        let search = Search {
            text: self.filter.clone(),
            sort: self.sort,
            desc: self.desc,
            limit: None,
            include_missing: false,
        };
        match self.library.tracks(&search) {
            Ok(rows) => self.rows = Arc::new(rows),
            Err(error) => {
                self.rows = Arc::new(Vec::new());
                let message = format!("query failed: {error}");
                self.log.push_back(message);
            }
        }
        self.clamp_selection();
        self.redraw = true;
    }

    fn clamp_selection(&mut self) {
        self.row_sel = self.row_sel.min(self.rows.len().saturating_sub(1));
        self.album_sel = self.album_sel.min(self.albums.len().saturating_sub(1));
        self.artist_sel = self.artist_sel.min(self.artists.len());
    }

    // ---- transport ------------------------------------------------------------

    fn play_rows(&mut self, rows: &[Row]) {
        if rows.is_empty() {
            return;
        }
        self.queue_ids = rows.iter().map(|r| r.id).collect();
        let tracks: Vec<Track> = rows.iter().map(|r| Track::new(r.path_buf())).collect();
        let first = tracks.first().map(|t| t.file_name()).unwrap_or_default();
        match self.playback.play(tracks) {
            None => self.log(format!("playing {} ({} tracks)", first, rows.len())),
            Some(error) => self.log(format!("cannot play: {error}")),
        }
        self.redraw = true;
    }

    fn play_from_selection(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        // Rotate so the selected row is first: `next` then walks to the end of the list and
        // `prev` reaches the rows above it, which is what a queue means.
        let rows = self.rows.as_ref();
        let start = self.row_sel.min(rows.len() - 1);
        let mut rotated = Vec::with_capacity(rows.len());
        rotated.extend_from_slice(&rows[start..]);
        rotated.extend_from_slice(&rows[..start]);
        self.play_rows(&rotated);
    }

    fn play_album(&mut self) {
        let Some(album) = self.albums.get(self.album_sel) else {
            return;
        };
        let mut search = Search::new(&album.album);
        search.sort = Sort::Album;
        search.limit = None;
        let rows = self.library.tracks(&search).unwrap_or_default();
        let rows: Vec<Row> = rows
            .into_iter()
            .filter(|r| {
                r.album.as_deref() == Some(album.album.as_str())
                    && r.albumartist.as_deref().or(r.artist.as_deref())
                        == Some(album.artist_label())
            })
            .collect();
        self.play_rows(&rows);
    }

    /// Resolve covers synchronously for the first `count` albums. Only the shot path needs
    /// this: a frame cannot wait, so the cache has to be warm before the first draw.
    #[cfg(feature = "shots")]
    pub fn preload_art(&mut self, count: usize) {
        for album in self.albums.iter().take(count) {
            let artist = album.albumartist.as_deref();
            if self.art.contains(artist, &album.album) {
                continue;
            }
            let art = self
                .library
                .fetch_album_art(artist, &album.album)
                .ok()
                .flatten();
            self.art.insert(artist, &album.album, art);
        }
    }

    /// A capture must not depend on the session the last run left behind: it renders the
    /// configured layout, unfiltered, from the top. (Found the hard way: two shots came out
    /// byte-identical because the restored session had one tab stack instead of two.)
    #[cfg(feature = "shots")]
    pub fn reset_for_capture(&mut self) {
        self.layout = self
            .config
            .load_layout(&self.config.layout)
            .unwrap_or_else(Node::default_layout);
        self.focus = 0;
        self.filter.clear();
        self.filter_editing = false;
        self.layout_mode = false;
        self.albums = Arc::new(self.library.albums().unwrap_or_default());
        self.reload_rows();
    }

    /// Put the focus on the properties panel, whichever tab it is behind.
    #[cfg(feature = "shots")]
    pub fn focus_properties(&mut self) {
        if let Some(index) = self
            .layout
            .leaves()
            .iter()
            .position(|kind| *kind == PanelKind::Properties)
        {
            self.focus = index;
        }
    }

    /// Show a given tab in the nth tab stack (used by the shot path).
    #[cfg(feature = "shots")]
    pub fn set_tabs(&mut self, nth: usize, active: usize) {
        if let Some(slot) = nth_tabs(&mut self.layout, nth) {
            *slot = active;
            self.redraw = true;
        }
    }

    #[cfg(feature = "shots")]
    pub fn theme(&self) -> Theme {
        self.theme
    }

    // ---- scanning -------------------------------------------------------------

    fn start_scan(&mut self) {
        if self.scan.is_some() {
            self.log("a scan is already running");
            return;
        }
        let root = self.config.root_path().to_path_buf();
        let db = self.db_path.clone();
        let progress = Arc::new(ScanProgress::new());
        self.scan = Some(progress.clone());
        self.log(format!("scanning {}", root.display()));

        // Its own connection: the scan owns the write path, the UI keeps reading.
        self.scan_thread = Some(std::thread::spawn(move || {
            let library = Library::open(&db).map_err(|e| e.to_string())?;
            let mut library = library;
            library
                .scan(&root, ScanOptions::default(), &progress)
                .map_err(|e| e.to_string())
        }));
        self.redraw = true;
    }

    fn poll_scan(&mut self) {
        let Some(handle) = &self.scan_thread else {
            return;
        };
        if !handle.is_finished() {
            return;
        }
        let handle = self.scan_thread.take().expect("checked");
        self.scan = None;
        match handle.join() {
            Ok(Ok(report)) => {
                self.log(format!(
                    "scan: {} files, {} changed, {} errors in {:.1}s",
                    report.scanned,
                    report.added + report.updated,
                    report.errors,
                    report.elapsed.as_secs_f64()
                ));
                self.reload_all();
            }
            Ok(Err(error)) => self.log(format!("scan failed: {error}")),
            Err(_) => self.log("scan panicked"),
        }
    }

    pub fn scan_state(&self) -> Option<(usize, usize, usize)> {
        self.scan
            .as_ref()
            .map(|p| (p.scanned(), p.total(), p.errors()))
    }

    // ---- keys -----------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.pending.is_some() {
            match key.code {
                KeyCode::Char('y') => self.confirm_pending(),
                KeyCode::Char('n') | KeyCode::Esc => {
                    self.pending = None;
                    self.log("cancelled");
                }
                _ => {}
            }
            return;
        }
        if self.props_editing.is_some() {
            self.handle_props_edit_key(key);
            return;
        }
        if self.filter_editing {
            self.handle_filter_key(key);
            return;
        }
        if self.layout_mode {
            self.handle_layout_key(key);
            return;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('l') => {
                    self.layout_mode = true;
                    self.log("layout mode: h/v split, x close, t next panel, arrows resize, Esc");
                    return;
                }
                KeyCode::Char('s') => {
                    self.save_layout();
                    return;
                }
                KeyCode::Char('c') => {
                    self.quit = true;
                    return;
                }
                _ => {}
            }
        }

        if key.modifiers.contains(KeyModifiers::ALT) {
            if let KeyCode::Char(digit @ '1'..='9') = key.code {
                self.load_layout_slot(digit);
                return;
            }
        }

        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Tab => self.focus_next(1),
            KeyCode::BackTab => self.focus_next(-1),
            KeyCode::Char('/') => {
                self.filter_before = self.filter.clone();
                self.filter_editing = true;
            }
            KeyCode::Esc => {
                if !self.filter.is_empty() {
                    self.filter.clear();
                    self.reload_rows();
                }
            }
            KeyCode::Char(' ') => {
                self.playback.toggle();
                self.redraw = true;
            }
            KeyCode::Char('n') => self.playback.next(),
            KeyCode::Char('p') => self.playback.prev(),
            KeyCode::Char('+') | KeyCode::Char('=') => {
                let volume = self.playback.nudge_volume(0.05);
                self.log(format!("volume {:.0}%", volume * 100.0));
            }
            KeyCode::Char('-') => {
                let volume = self.playback.nudge_volume(-0.05);
                self.log(format!("volume {:.0}%", volume * 100.0));
            }
            KeyCode::Char('s') => self.cycle_sort(),
            KeyCode::Char('S') => self.start_scan(),
            KeyCode::Char('m') => self.toggle_tab_nth(0),
            KeyCode::Char('M') => self.toggle_tab_nth(1),
            KeyCode::Char('u') => self.undo(),
            KeyCode::Char('A') => self.apply_to_all(),
            KeyCode::Char('?') => self.log(
                "keys: / filter  Tab focus  jk move  Enter play (or edit a tag field)  space pause  n/p next/prev  +/- vol  s sort  S scan  m/M tab stacks  A apply field to all filtered  u undo  Ctrl+L layout  Ctrl+S save layout  Alt+1..9 slot  q quit",
            ),
            KeyCode::Enter => self.activate(),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::Home | KeyCode::Char('g') => self.move_to(0),
            KeyCode::End | KeyCode::Char('G') => self.move_to(usize::MAX),
            _ => {}
        }
    }

    fn handle_filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                self.filter_editing = false;
                self.row_sel = 0;
                self.reload_rows();
                self.log(format!(
                    "{} tracks match \"{}\"",
                    self.rows.len(),
                    self.filter
                ));
            }
            KeyCode::Esc => {
                self.filter = self.filter_before.clone();
                self.filter_editing = false;
                self.reload_rows();
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.row_sel = 0;
                self.reload_rows();
            }
            KeyCode::Char(c) => {
                self.filter.push(c);
                self.row_sel = 0;
                self.reload_rows();
            }
            _ => {}
        }
    }

    /// Typing in the Properties panel. Enter commits through the journalled writer; Esc
    /// throws the buffer away, because a tag editor that saves on blur is a tag editor that
    /// loses data.
    fn handle_props_edit_key(&mut self, key: KeyEvent) {
        let Some((field, mut buffer)) = self.props_editing.take() else {
            return;
        };
        match key.code {
            KeyCode::Enter => self.commit_field(field, buffer),
            KeyCode::Esc => self.log("tag edit cancelled"),
            KeyCode::Backspace => {
                buffer.pop();
                self.props_editing = Some((field, buffer));
            }
            KeyCode::Char(c) => {
                buffer.push(c);
                self.props_editing = Some((field, buffer));
            }
            _ => self.props_editing = Some((field, buffer)),
        }
    }

    fn commit_field(&mut self, field: Field, value: String) {
        let Some(row) = self.rows.get(self.row_sel).cloned() else {
            return;
        };
        let patch = Patch::new().set(field, value.clone());
        let path = row.path_buf();
        match self.library.write_tags(std::slice::from_ref(&path), &patch) {
            Ok(report) if report.is_ok() => {
                self.last_edit = Some((field, value.clone()));
                self.log(format!(
                    "{} = {:?}: {}",
                    field.label(),
                    value,
                    report.summary("wrote")
                ));
                self.reload_all();
            }
            Ok(report) => {
                let first = report
                    .failed
                    .first()
                    .map(|(_, error)| error.clone())
                    .unwrap_or_default();
                self.log(format!("{}: {first}", report.summary("tag write failed")));
            }
            Err(error) => self.log(format!("tag write failed: {error}")),
        }
        self.refresh_undo();
    }

    /// Apply the last edited value to every track the table is showing -- the multi-file edit,
    /// behind a confirmation.
    fn apply_to_all(&mut self) {
        let Some((field, value)) = self.last_edit.clone() else {
            self.log("edit a field first, then A applies it to the filtered tracks");
            return;
        };
        let paths: Vec<PathBuf> = self.rows.iter().map(|row| row.path_buf()).collect();
        if paths.is_empty() {
            return;
        }
        let patch = Patch::new().set(field, value.clone());
        let label = format!("{}={value:?} on {} tracks", field.key(), paths.len());
        self.log(format!("apply {label}? y/N"));
        self.pending = Some(Pending {
            patch,
            paths,
            label,
        });
    }

    fn confirm_pending(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        match self.library.write_tags(&pending.paths, &pending.patch) {
            Ok(report) if report.is_ok() => {
                self.log(format!("{}: {}", pending.label, report.summary("wrote")));
                self.reload_all();
            }
            Ok(report) => {
                let first = report
                    .failed
                    .first()
                    .map(|(path, error)| format!("{}: {error}", path.display()))
                    .unwrap_or_default();
                self.log(format!(
                    "{}; first failure {first}",
                    report.summary("tag write")
                ));
            }
            Err(error) => self.log(format!("tag write failed: {error}")),
        }
        self.refresh_undo();
    }

    fn undo(&mut self) {
        if self.undo_depth == 0 {
            self.log("nothing to undo");
            return;
        }
        match self.library.undo_last() {
            Ok(report) if report.is_ok() => {
                self.log(report.summary("undid"));
                self.reload_all();
            }
            Ok(report) => self.log(format!(
                "{}; some files could not be restored",
                report.summary("undo")
            )),
            Err(error) => self.log(format!("undo failed: {error}")),
        }
        self.refresh_undo();
    }

    fn refresh_undo(&mut self) {
        self.undo_depth = self.library.undo_depth().unwrap_or(0);
        self.redraw = true;
    }

    fn handle_layout_key(&mut self, key: KeyEvent) {
        let path = self
            .layout
            .leaf_paths()
            .get(self.focus)
            .cloned()
            .unwrap_or_default();
        match key.code {
            KeyCode::Esc | KeyCode::Enter => self.layout_mode = false,
            KeyCode::Char('h') => {
                let next = self.focused().next();
                self.layout.split_at(&path, Axis::Horizontal, next);
            }
            KeyCode::Char('v') => {
                let next = self.focused().next();
                self.layout.split_at(&path, Axis::Vertical, next);
            }
            KeyCode::Char('x') => {
                if !self.layout.close_at(&path) {
                    self.log("cannot close the last panel");
                }
            }
            KeyCode::Char('t') => {
                let next = self.focused().next();
                if let Some(node) = node_at_mut(&mut self.layout, &path) {
                    *node = Node::panel(next);
                }
            }
            KeyCode::Left => {
                self.layout.resize_at(&path, -2);
            }
            KeyCode::Right => {
                self.layout.resize_at(&path, 2);
            }
            KeyCode::Up => {
                self.layout.resize_at(&path, 2);
            }
            KeyCode::Down => {
                self.layout.resize_at(&path, -2);
            }
            _ => {}
        }
        let leaves = self.layout.leaves().len();
        if leaves > 0 {
            self.focus = self.focus.min(leaves - 1);
        }
        self.redraw = true;
    }

    fn focus_next(&mut self, delta: i32) {
        let leaves = self.layout.leaves().len();
        if leaves == 0 {
            return;
        }
        let leaves = leaves as i32;
        self.focus = (((self.focus as i32 + delta) % leaves + leaves) % leaves) as usize;
        self.redraw = true;
    }

    fn move_selection(&mut self, delta: i32) {
        match self.focused() {
            PanelKind::Table => {
                let len = self.rows.len();
                if len > 0 {
                    self.row_sel = (self.row_sel as i32 + delta).clamp(0, len as i32 - 1) as usize;
                }
            }
            PanelKind::Albums => {
                let len = self.albums.len();
                if len > 0 {
                    self.album_sel =
                        (self.album_sel as i32 + delta).clamp(0, len as i32 - 1) as usize;
                }
            }
            PanelKind::Properties => {
                let len = Field::ALL.len();
                self.props_sel = (self.props_sel as i32 + delta).clamp(0, len as i32 - 1) as usize;
            }
            PanelKind::Tree => {
                let len = self.artists.len();
                if len > 0 {
                    self.artist_sel =
                        (self.artist_sel as i32 + delta).clamp(0, len as i32) as usize;
                }
            }
            _ => {}
        }
        self.redraw = true;
    }

    fn move_to(&mut self, target: usize) {
        match self.focused() {
            PanelKind::Table => self.row_sel = target.min(self.rows.len().saturating_sub(1)),
            PanelKind::Albums => self.album_sel = target.min(self.albums.len().saturating_sub(1)),
            PanelKind::Tree => self.artist_sel = target.min(self.artists.len()),
            _ => {}
        }
        self.redraw = true;
    }

    fn activate(&mut self) {
        match self.focused() {
            PanelKind::Table => self.play_from_selection(),
            PanelKind::Albums => self.play_album(),
            PanelKind::Properties => {
                let field = Field::ALL[self.props_sel.min(Field::ALL.len() - 1)];
                let current = self
                    .rows
                    .get(self.row_sel)
                    .and_then(|row| row.field(field))
                    .unwrap_or_default();
                self.log(format!(
                    "editing {} (Enter commits, Esc cancels)",
                    field.label()
                ));
                self.props_editing = Some((field, current));
            }
            PanelKind::Tree => {
                if self.artist_sel == 0 {
                    self.filter.clear();
                } else if let Some((artist, _)) = self.artists.get(self.artist_sel - 1) {
                    self.filter = artist.clone();
                }
                self.reload_rows();
                // Jump to the table, which is where the filtered list lives.
                if let Some(index) = self
                    .layout
                    .leaves()
                    .iter()
                    .position(|k| *k == PanelKind::Table)
                {
                    self.focus = index;
                }
                self.log(format!(
                    "{} tracks for \"{}\"",
                    self.rows.len(),
                    self.filter
                ));
            }
            _ => {}
        }
    }

    fn cycle_sort(&mut self) {
        self.sort = match self.sort {
            Sort::Artist => Sort::Album,
            Sort::Album => Sort::Title,
            Sort::Title => Sort::Year,
            Sort::Year => Sort::Duration,
            Sort::Duration => Sort::Added,
            Sort::Added => Sort::Path,
            Sort::Path => Sort::Artist,
        };
        self.reload_rows();
        self.log(format!("sorted by {}", self.sort.label()));
    }

    /// Switch the nth tab stack (0 = tracks/albums, 1 = now playing/properties).
    fn toggle_tab_nth(&mut self, nth: usize) {
        if let Some(active) = nth_tabs(&mut self.layout, nth) {
            *active = 1 - *active;
            self.redraw = true;
        }
    }

    fn save_layout(&mut self) {
        let name = self.config.layout.clone();
        match self.config.save_layout(&name, &self.layout) {
            Ok(path) => self.log(format!("layout saved to {}", path.display())),
            Err(error) => self.log(format!("could not save layout: {error}")),
        }
    }

    fn load_layout_slot(&mut self, digit: char) {
        let name = format!("slot{digit}");
        match self.config.load_layout(&name) {
            Some(node) => {
                self.layout = node;
                self.focus = 0;
                self.log(format!("layout {name}"));
            }
            None => self.log(format!(
                "no layout {name} (Ctrl+S saves the current one as {})",
                self.config.layout
            )),
        }
    }

    pub fn tick(&mut self) {
        if self.art.poll() {
            self.redraw = true;
        }
        self.poll_scan();
        let playing = self
            .playback
            .index()
            .and_then(|i| self.queue_ids.get(i).copied());
        if playing != self.playing_id {
            self.playing_id = playing;
            self.redraw = true;
        }
    }

    pub fn finish(&mut self) {
        self.playback.stop();
        self.config.save();
        let session = Session {
            focus: self.focus,
            filter: self.filter.clone(),
            sort: self.sort.label().to_string(),
            desc: self.desc,
            node: Some(self.layout.clone()),
        };
        let path = config::config_dir().join("session.toml");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = toml::to_string_pretty(&session) {
            let _ = std::fs::write(path, text);
        }
    }

    // ---- frame ----------------------------------------------------------------

    pub fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let chunks = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).split(area);
        let node = self.layout.clone();
        let mut cursor = 0usize;
        self.draw_node(frame, node, chunks[0], &mut cursor);
        self.draw_status(frame, chunks[1]);
        self.redraw = false;
    }

    fn draw_node(&mut self, frame: &mut Frame, node: Node, area: Rect, cursor: &mut usize) {
        match node {
            Node::Panel { panel } => {
                let focused = *cursor == self.focus;
                *cursor += 1;
                panels::draw(frame, self, panel, area, focused);
            }
            Node::Split {
                axis,
                weights,
                children,
            } => {
                let rects = super::layout::split(area, axis, &weights);
                for (child, rect) in children.into_iter().zip(rects) {
                    self.draw_node(frame, child, rect, cursor);
                }
            }
            Node::Tabs { active, children } => {
                if children.is_empty() {
                    return;
                }
                let strip = Rect { height: 1, ..area };
                let rest = Rect {
                    y: area.y + 1,
                    height: area.height.saturating_sub(1),
                    ..area
                };
                let mut spans = Vec::new();
                for (index, child) in children.iter().enumerate() {
                    let kind = first_panel(child);
                    let style = if index == active {
                        Style::default().fg(self.theme.accent)
                    } else {
                        Style::default().fg(self.theme.dim)
                    };
                    spans.push(Span::styled(
                        format!(" {} ", index + 1),
                        Style::default().fg(self.theme.bar),
                    ));
                    spans.push(Span::styled(format!("{} ", kind.label()), style));
                }
                frame.render_widget(ratatui::widgets::Paragraph::new(Line::from(spans)), strip);
                if let Some(child) = children.get(active).cloned() {
                    self.draw_node(frame, child, rest, cursor);
                }
            }
        }
    }

    fn draw_status(&self, frame: &mut Frame, area: Rect) {
        let playing = if self.playback.is_active() {
            let state = if self.playback.paused() {
                "paused"
            } else {
                "playing"
            };
            format!("{} {}", state, clock(self.playback.position_ms()))
        } else {
            "stopped".to_string()
        };
        let scan = match self.scan_state() {
            Some((scanned, total, errors)) => format!("  scan {scanned}/{total} e{errors}"),
            None => String::new(),
        };
        let filter = if self.filter.is_empty() {
            String::new()
        } else {
            format!("  filter \"{}\"", self.filter)
        };
        let pending = match &self.pending {
            Some(pending) => format!("  APPLY {}? y/N", pending.label),
            None => String::new(),
        };
        let undo = if self.undo_depth > 0 {
            format!("  undo:{}", self.undo_depth)
        } else {
            String::new()
        };
        let mode = if self.layout_mode {
            "  LAYOUT".to_string()
        } else if self.props_editing.is_some() {
            "  EDITING TAG".to_string()
        } else {
            String::new()
        };
        let hints = "  / filter  Tab focus  Enter play  space pause  m/M tab  u undo  Ctrl+L layout  q quit";
        let line = Line::from(vec![
            Span::styled(
                format!(
                    " {}/{} {}{}{}{}{}{} ",
                    self.row_sel + 1,
                    self.rows.len(),
                    playing,
                    filter,
                    scan,
                    undo,
                    mode,
                    pending
                ),
                Style::default().fg(self.theme.bg).bg(self.theme.accent),
            ),
            Span::styled(hints, Style::default().fg(self.theme.dim)),
        ]);
        frame.render_widget(
            ratatui::widgets::Paragraph::new(line).style(Style::default().bg(self.theme.bar)),
            area,
        );
    }
}

/// The active index of the nth tab stack, in draw order.
fn nth_tabs(node: &mut Node, nth: usize) -> Option<&mut usize> {
    fn find<'a>(node: &'a mut Node, nth: usize, seen: &mut usize) -> Option<&'a mut usize> {
        match node {
            Node::Tabs { active, .. } => {
                if *seen == nth {
                    Some(active)
                } else {
                    *seen += 1;
                    None
                }
            }
            Node::Split { children, .. } => {
                children.iter_mut().find_map(|child| find(child, nth, seen))
            }
            Node::Panel { .. } => None,
        }
    }
    let mut seen = 0;
    find(node, nth, &mut seen)
}

fn read_session() -> Session {
    let path = config::config_dir().join("session.toml");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn sort_from_key(key: &str) -> Sort {
    match key {
        "album" => Sort::Album,
        "title" => Sort::Title,
        "path" => Sort::Path,
        "year" => Sort::Year,
        "duration" => Sort::Duration,
        "added" => Sort::Added,
        _ => Sort::Artist,
    }
}

fn node_at_mut<'a>(node: &'a mut Node, path: &[usize]) -> Option<&'a mut Node> {
    let mut node = node;
    for index in path {
        node = match node {
            Node::Split { children, .. } | Node::Tabs { children, .. } => {
                children.get_mut(*index)?
            }
            Node::Panel { .. } => return None,
        };
    }
    Some(node)
}

fn first_panel(node: &Node) -> PanelKind {
    match node {
        Node::Panel { panel } => *panel,
        Node::Tabs { children, active } => children
            .get(*active)
            .map(first_panel)
            .unwrap_or(PanelKind::Table),
        Node::Split { children, .. } => children
            .first()
            .map(first_panel)
            .unwrap_or(PanelKind::Table),
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;

fn clock(ms: u64) -> String {
    format!("{}:{:02}", ms / 60_000, (ms / 1_000) % 60)
}
