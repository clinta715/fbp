//! The data-loss surface, tested.
//!
//! Every test here runs against a real fixture file in a temp directory, because the thing
//! being tested is what happens to bytes on disk -- a mock would test nothing.

use std::path::{Path, PathBuf};

use fbp_core::tag::{Field, Patch};
use fbp_index::{Library, ScanOptions, ScanProgress, Search, Sort};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// A temp directory holding copies of the named fixtures.
fn workspace(tag: &str, names: &[&str]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fbp-tags-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    for name in names {
        std::fs::copy(fixtures().join(name), dir.join(name)).expect("copy fixture");
    }
    dir
}

fn indexed(dir: &Path) -> Library {
    let mut library = Library::in_memory().expect("library");
    let progress = ScanProgress::new();
    library
        .scan(dir, ScanOptions::default(), &progress)
        .expect("scan");
    library
}

/// Read a field straight from the file, not from the index: the point of the write is the
/// file.
fn file_field(path: &Path, field: Field) -> Option<String> {
    use lofty::file::TaggedFileExt;
    let tagged = lofty::read_from_path(path).expect("read");
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let tag = tag?;
    use lofty::tag::Accessor;
    match field {
        Field::Title => tag.title().map(|v| v.to_string()),
        Field::Artist => tag.artist().map(|v| v.to_string()),
        Field::Album => tag.album().map(|v| v.to_string()),
        Field::Genre => tag.genre().map(|v| v.to_string()),
        Field::Year => tag.date().map(|date| date.year.to_string()),
        Field::TrackNo => tag.track().map(|v| v.to_string()),
        Field::DiscNo => tag.disk().map(|v| v.to_string()),
        Field::AlbumArtist => tag
            .get_string(lofty::tag::ItemKey::AlbumArtist)
            .map(|v| v.to_string()),
        Field::Composer => tag
            .get_string(lofty::tag::ItemKey::Composer)
            .map(|v| v.to_string()),
        Field::Comment => tag.comment().map(|v| v.to_string()),
    }
}

#[test]
fn a_tag_edit_reaches_the_file_and_the_index() {
    let dir = workspace("roundtrip", &["a1.mp3"]);
    let path = dir.join("a1.mp3");
    let library = indexed(&dir);

    let patch = Patch::new()
        .set(Field::Title, "New Title")
        .set(Field::Artist, "New Artist")
        .set(Field::AlbumArtist, "New Album Artist")
        .set(Field::Year, "2020")
        .set(Field::Genre, "Dub");
    let report = library
        .write_tags(std::slice::from_ref(&path), &patch)
        .expect("write");
    assert_eq!(report.written, 1, "{report:?}");
    assert!(report.is_ok(), "{report:?}");

    // The file itself.
    assert_eq!(
        file_field(&path, Field::Title).as_deref(),
        Some("New Title")
    );
    assert_eq!(
        file_field(&path, Field::Artist).as_deref(),
        Some("New Artist")
    );
    assert_eq!(
        file_field(&path, Field::AlbumArtist).as_deref(),
        Some("New Album Artist")
    );
    assert_eq!(file_field(&path, Field::Year).as_deref(), Some("2020"));

    // The index, without a rescan -- and the search index with it.
    let rows = library.tracks(&Search::new("new title")).expect("query");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].artist.as_deref(), Some("New Artist"));
    assert_eq!(rows[0].year, Some(2020));
}

#[test]
fn a_flac_is_written_through_its_own_tag_format() {
    let dir = workspace("flac", &["b1.flac"]);
    let path = dir.join("b1.flac");
    let library = indexed(&dir);

    let patch = Patch::new()
        .set(Field::Album, "Album C")
        .set(Field::AlbumArtist, "Björk Guðmundsdóttir")
        .set(Field::Year, "1997");
    let report = library
        .write_tags(std::slice::from_ref(&path), &patch)
        .expect("write");
    assert!(report.is_ok(), "{report:?}");

    assert_eq!(file_field(&path, Field::Album).as_deref(), Some("Album C"));
    assert_eq!(
        file_field(&path, Field::AlbumArtist).as_deref(),
        Some("Björk Guðmundsdóttir")
    );
    assert_eq!(file_field(&path, Field::Year).as_deref(), Some("1997"));
    // The title was not in the patch, so it is untouched.
    assert_eq!(file_field(&path, Field::Title).as_deref(), Some("Jóga"));
}

#[test]
fn one_gesture_over_many_files_is_one_undo_entry() {
    let dir = workspace("many", &["a1.mp3", "a2.mp3"]);
    let paths = vec![dir.join("a1.mp3"), dir.join("a2.mp3")];
    let library = indexed(&dir);

    let patch = Patch::new().set(Field::Album, "Album Z");
    let report = library.write_tags(&paths, &patch).expect("write");
    assert_eq!(report.written, 2);
    assert_eq!(
        library.undo_depth().expect("depth"),
        1,
        "one gesture, one entry"
    );
    for path in &paths {
        assert_eq!(file_field(path, Field::Album).as_deref(), Some("Album Z"));
    }

    let undo = library.undo_last().expect("undo");
    assert_eq!(undo.written, 2, "{undo:?}");
    assert_eq!(library.undo_depth().expect("depth"), 0);
    for path in &paths {
        assert_eq!(file_field(path, Field::Album).as_deref(), Some("Album A"));
    }
    assert_eq!(
        library
            .tracks(&Search::new("Album A"))
            .expect("query")
            .len(),
        2,
        "the index came back too"
    );
}

#[test]
fn undo_walks_back_one_gesture_at_a_time() {
    let dir = workspace("steps", &["a1.mp3"]);
    let path = dir.join("a1.mp3");
    let library = indexed(&dir);

    library
        .write_tags(
            std::slice::from_ref(&path),
            &Patch::new().set(Field::Artist, "First Edit"),
        )
        .expect("write 1");
    library
        .write_tags(
            std::slice::from_ref(&path),
            &Patch::new().set(Field::Artist, "Second Edit"),
        )
        .expect("write 2");
    assert_eq!(library.undo_depth().expect("depth"), 2);

    library.undo_last().expect("undo 1");
    assert_eq!(
        file_field(&path, Field::Artist).as_deref(),
        Some("First Edit")
    );
    library.undo_last().expect("undo 2");
    assert_eq!(
        file_field(&path, Field::Artist).as_deref(),
        Some("Artist One")
    );
    assert_eq!(library.undo_depth().expect("depth"), 0);

    // Undoing past the start is a no-op, not a panic.
    let nothing = library.undo_last().expect("empty undo");
    assert_eq!(nothing.written, 0);
}

#[test]
fn clearing_a_field_removes_it_from_the_file() {
    let dir = workspace("clear", &["a1.mp3"]);
    let path = dir.join("a1.mp3");
    let library = indexed(&dir);

    library
        .write_tags(
            std::slice::from_ref(&path),
            &Patch::new().clear(Field::Genre),
        )
        .expect("write");
    assert_eq!(file_field(&path, Field::Genre), None);
    let rows = library.tracks(&Search::new("first")).expect("query");
    assert_eq!(rows[0].genre, None);

    // And the undo brings it back.
    library.undo_last().expect("undo");
    assert_eq!(file_field(&path, Field::Genre).as_deref(), Some("Rock"));
}

#[test]
fn a_bad_number_is_refused_before_any_file_is_touched() {
    let dir = workspace("validate", &["a1.mp3"]);
    let path = dir.join("a1.mp3");
    let library = indexed(&dir);
    let before = std::fs::read(&path).expect("read");

    let error = library
        .write_tags(
            std::slice::from_ref(&path),
            &Patch::new().set(Field::TrackNo, "not a number"),
        )
        .expect_err("must refuse");
    assert!(error.to_string().contains("number"), "{error}");
    assert_eq!(std::fs::read(&path).expect("read"), before);
    assert_eq!(library.undo_depth().expect("depth"), 0);
}

#[test]
fn a_write_that_cannot_happen_leaves_the_original_bytes_alone() {
    let dir = workspace("readonly", &["a1.mp3"]);
    let path = dir.join("a1.mp3");
    let library = indexed(&dir);
    let before = std::fs::read(&path).expect("read");

    // A read-only directory stops the temp file from being created, which is the same
    // failure mode as a full disk: nothing may be published under the real name.
    let mut permissions = std::fs::metadata(&dir).expect("metadata").permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o555);
    }
    std::fs::set_permissions(&dir, permissions).expect("chmod");

    let report = library.write_tags(
        std::slice::from_ref(&path),
        &Patch::new().set(Field::Title, "X"),
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut restore = std::fs::metadata(&dir).expect("metadata").permissions();
        restore.set_mode(0o755);
        std::fs::set_permissions(&dir, restore).expect("chmod back");
    }

    let report = report.expect("the call itself succeeds and reports the failure");
    assert_eq!(report.written, 0);
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert_eq!(
        std::fs::read(&path).expect("read"),
        before,
        "bytes unchanged"
    );
    // Whatever temp file name we tried must not be left behind.
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .expect("dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".fbp-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files left behind: {leftovers:?}"
    );
}

#[test]
fn an_album_wide_edit_shows_up_in_album_grouping() {
    let dir = workspace("album", &["a1.mp3", "a2.mp3"]);
    let paths = vec![dir.join("a1.mp3"), dir.join("a2.mp3")];
    let library = indexed(&dir);

    library
        .write_tags(&paths, &Patch::new().set(Field::Album, "Renamed Album"))
        .expect("write");

    let _sorted = Search {
        sort: Sort::Album,
        ..Default::default()
    };
    let albums = library.albums().expect("albums");
    assert_eq!(albums.len(), 1, "both tracks landed in one album");
    assert_eq!(albums[0].album, "Renamed Album");
    assert_eq!(albums[0].tracks, 2);
}
