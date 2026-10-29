//! The UI half of tag editing: keys in, files out, undo back.
//!
//! The writer is tested against bytes in fbp-index; these tests exist to prove the panel is
//! wired to it -- that Enter commits, that Esc cancels, that the multi-file apply asks first,
//! and that undo goes through the same path.

use std::path::Path;

use super::*;
use fbp_index::{Library, ScanOptions, ScanProgress};
use ratatui::crossterm::event::KeyCode;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn fixture_app(tag: &str) -> (PathBuf, App) {
    let dir = std::env::temp_dir().join(format!("fbp-app-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fbp-index/fixtures");
    for name in ["a1.mp3", "a2.mp3"] {
        std::fs::copy(fixtures.join(name), dir.join(name)).expect("copy fixture");
    }

    let db = dir.join("library.db");
    let mut library = Library::open(&db).expect("library");
    let progress = ScanProgress::new();
    library
        .scan(&dir, ScanOptions::default(), &progress)
        .expect("scan");
    drop(library);

    let config = Config {
        art: false,
        ..Config::default()
    };
    let mut app = App::open(db, config, None, false).expect("app");
    // A leftover session (layout, filter, focus) must not decide what this test sees.
    app.layout = Node::default_layout();
    app.focus = 0;
    app.filter.clear();
    app.reload_rows();
    app.row_sel = 0;
    app.album_sel = 0;
    (dir, app)
}

fn file_field(path: &Path, field: Field) -> Option<String> {
    use lofty::file::TaggedFileExt;
    use lofty::tag::Accessor;
    let tagged = lofty::read_from_path(path).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    match field {
        Field::Artist => tag.artist().map(|v| v.to_string()),
        Field::Album => tag.album().map(|v| v.to_string()),
        Field::Genre => tag.genre().map(|v| v.to_string()),
        other => panic!("the test only reads artist/album/genre, asked for {other:?}"),
    }
}

/// Show the properties tab and focus it, through keys (M toggles the second tab stack).
fn focus_properties(app: &mut App) {
    app.handle_key(key(KeyCode::Char('M')));
    let index = app
        .layout
        .leaves()
        .iter()
        .position(|kind| *kind == PanelKind::Properties)
        .expect("properties is visible after M");
    app.focus = index;
}

fn type_text(app: &mut App, current: &str, replacement: &str) {
    for _ in 0..current.chars().count() {
        app.handle_key(key(KeyCode::Backspace));
    }
    for c in replacement.chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
}

#[test]
fn editing_a_tag_in_the_panel_writes_the_file_and_undoes() {
    let (dir, mut app) = fixture_app("edit");
    focus_properties(&mut app);
    app.props_sel = 1; // artist

    app.handle_key(key(KeyCode::Enter)); // start editing (buffer = current value)
    type_text(&mut app, "Artist One", "New Artist");
    app.handle_key(key(KeyCode::Enter)); // commit

    assert_eq!(
        file_field(&dir.join("a1.mp3"), Field::Artist).as_deref(),
        Some("New Artist"),
        "the file on disk carries the edit"
    );
    assert_eq!(app.undo_depth, 1);

    app.handle_key(key(KeyCode::Char('u')));
    assert_eq!(
        file_field(&dir.join("a1.mp3"), Field::Artist).as_deref(),
        Some("Artist One"),
        "undo put the old value back"
    );
    assert_eq!(app.undo_depth, 0);
}

#[test]
fn escape_leaves_the_file_alone() {
    let (dir, mut app) = fixture_app("escape");
    focus_properties(&mut app);
    app.props_sel = 1;

    app.handle_key(key(KeyCode::Enter));
    type_text(&mut app, "Artist One", "Something Else");
    app.handle_key(key(KeyCode::Esc));

    assert_eq!(
        file_field(&dir.join("a1.mp3"), Field::Artist).as_deref(),
        Some("Artist One")
    );
    assert_eq!(app.undo_depth, 0, "a cancelled edit writes nothing");
}

#[test]
fn apply_to_all_asks_before_it_writes_the_filtered_set() {
    let (dir, mut app) = fixture_app("apply");

    // Both fixture tracks are on "Album A", so a filter on the album keeps them in scope
    // while the edited field (genre) does not change what the filter matches.
    app.filter = "album a".to_string();
    app.reload_rows();
    assert_eq!(app.rows.len(), 2, "both tracks are in the filtered set");

    focus_properties(&mut app);
    app.props_sel = 4; // genre
    app.handle_key(key(KeyCode::Enter));
    type_text(&mut app, "Rock", "Dub");
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        file_field(&dir.join("a1.mp3"), Field::Genre).as_deref(),
        Some("Dub")
    );

    // A asks; n does nothing.
    app.handle_key(key(KeyCode::Char('A')));
    assert!(app.pending.is_some(), "the confirmation is pending");
    app.handle_key(key(KeyCode::Char('n')));
    assert_eq!(
        file_field(&dir.join("a2.mp3"), Field::Genre).as_deref(),
        Some("Rock"),
        "a refused batch writes nothing"
    );

    // A then y writes the whole filtered set in one gesture.
    app.handle_key(key(KeyCode::Char('A')));
    app.handle_key(key(KeyCode::Char('y')));
    assert_eq!(
        file_field(&dir.join("a2.mp3"), Field::Genre).as_deref(),
        Some("Dub")
    );
    assert_eq!(
        app.undo_depth, 2,
        "the single edit and the batch: two gestures"
    );

    // One undo per gesture.
    app.handle_key(key(KeyCode::Char('u')));
    assert_eq!(
        file_field(&dir.join("a1.mp3"), Field::Genre).as_deref(),
        Some("Dub"),
        "the single edit survives the batch undo"
    );
    assert_eq!(
        file_field(&dir.join("a2.mp3"), Field::Genre).as_deref(),
        Some("Rock")
    );
}
