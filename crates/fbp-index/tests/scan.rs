//! The scanner against a real fixture tree: tags, increments, removal, search.

use std::path::{Path, PathBuf};

use fbp_index::{Library, ScanOptions, ScanProgress, Search, Sort};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// A fresh copy of the fixture tree per test: the scanner mutates nothing, but the
/// *test* deletes and touches files, so sharing would couple the tests.
fn library_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fbp-index-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    for name in ["a1.mp3", "a2.mp3", "b1.flac", "broken.mp3"] {
        std::fs::copy(fixtures().join(name), dir.join(name)).expect("copy fixture");
    }
    dir
}

fn scan(library: &mut Library, root: &Path) -> fbp_index::ScanReport {
    let progress = ScanProgress::new();
    library
        .scan(root, ScanOptions::default(), &progress)
        .expect("scan")
}

#[test]
fn scan_reads_tags_durations_and_codecs() {
    let mut library = Library::in_memory().expect("library");
    let root = library_root("tags");
    let report = scan(&mut library, &root);

    assert_eq!(report.scanned, 4);
    assert_eq!(report.added, 4);
    assert_eq!(report.removed, 0);

    let rows = library.tracks(&Search::new("first")).expect("tracks");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.title.as_deref(), Some("First"));
    assert_eq!(row.artist.as_deref(), Some("Artist One"));
    assert_eq!(row.albumartist.as_deref(), Some("Artist One"));
    assert_eq!(row.album.as_deref(), Some("Album A"));
    assert_eq!(row.track_no, Some(1));
    assert_eq!(row.year, Some(1999));
    assert_eq!(row.genre.as_deref(), Some("Rock"));
    assert_eq!(row.codec.as_deref(), Some("mp3"));
    assert_eq!(row.sample_rate, Some(44_100));
    let duration = row.duration_ms.expect("duration");
    assert!((900..=1_200).contains(&duration), "duration {duration} ms");

    let flac = library.tracks(&Search::new("joga")).expect("tracks");
    assert_eq!(flac.len(), 1);
    assert_eq!(flac[0].album.as_deref(), Some("Album B"));
    assert_eq!(flac[0].codec.as_deref(), Some("flac"));
}

#[test]
fn a_rescan_of_unchanged_files_writes_nothing() {
    let mut library = Library::in_memory().expect("library");
    let root = library_root("rescan");
    scan(&mut library, &root);
    let second = scan(&mut library, &root);

    assert_eq!(second.added, 0, "a warm rescan must not re-add");
    assert_eq!(second.updated, 0, "a warm rescan must not rewrite rows");
    assert_eq!(second.unchanged, 4);
    assert_eq!(library.count().expect("count"), 4);
}

#[test]
fn a_changed_file_is_reread() {
    let mut library = Library::in_memory().expect("library");
    let root = library_root("changed");
    scan(&mut library, &root);

    // Appending a byte changes size and mtime, which is what the scanner compares.
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(root.join("a1.mp3"))
        .expect("open");
    use std::io::Write;
    file.write_all(&[0u8]).expect("append");
    drop(file);

    let report = scan(&mut library, &root);
    assert_eq!(report.updated, 1);
    assert_eq!(report.unchanged, 3);
}

#[test]
fn a_deleted_file_becomes_missing_not_gone() {
    let mut library = Library::in_memory().expect("library");
    let root = library_root("missing");
    scan(&mut library, &root);
    std::fs::remove_file(root.join("a2.mp3")).expect("remove");

    let report = scan(&mut library, &root);
    assert_eq!(report.removed, 1);
    assert_eq!(
        library.count().expect("count"),
        3,
        "missing rows are excluded by default"
    );

    let with_missing = Search {
        include_missing: true,
        ..Default::default()
    };
    let rows = library.tracks(&with_missing).expect("tracks");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows.iter().filter(|r| r.missing).count(), 1);

    // The tags survive, which is what makes a repair possible later.
    let restored = rows.iter().find(|r| r.missing).expect("missing row");
    assert_eq!(restored.title.as_deref(), Some("Second"));
}

#[test]
fn search_folds_diacritics_and_matches_titles_and_artists() {
    let mut library = Library::in_memory().expect("library");
    let root = library_root("search");
    scan(&mut library, &root);

    // "Björk" is tagged with the diacritic; the index folds it.
    assert_eq!(library.tracks(&Search::new("bjork")).expect("q").len(), 1);
    assert_eq!(library.tracks(&Search::new("Joga")).expect("q").len(), 1);
    // Prefix search, and a token that matches nothing.
    assert_eq!(library.tracks(&Search::new("art")).expect("q").len(), 2);
    assert_eq!(library.tracks(&Search::new("zzzz")).expect("q").len(), 0);
    // A quote in user input is data, not syntax.
    assert_eq!(
        library.tracks(&Search::new("\"first\"")).expect("q").len(),
        1
    );
}

#[test]
fn albums_group_by_album_artist_and_sort_is_explicit() {
    let mut library = Library::in_memory().expect("library");
    let root = library_root("albums");
    scan(&mut library, &root);

    let albums = library.albums().expect("albums");
    assert_eq!(albums.len(), 2);
    let album_a = albums
        .iter()
        .find(|a| a.album == "Album A")
        .expect("Album A");
    assert_eq!(album_a.tracks, 2);
    assert_eq!(album_a.year, Some(1999));
    assert_eq!(album_a.albumartist.as_deref(), Some("Artist One"));

    let by_title = Search {
        sort: Sort::Title,
        ..Default::default()
    };
    let titles: Vec<String> = library
        .tracks(&by_title)
        .expect("tracks")
        .into_iter()
        .map(|r| r.display_title().to_string())
        .collect();
    // NOCASE, title-first with the file name as the fallback for the untagged file.
    assert_eq!(
        titles,
        vec!["broken.mp3", "First", "Jóga", "Second"],
        "sorted by title"
    );

    let page = library
        .tracks(&Search {
            limit: Some(2),
            ..Search::default()
        })
        .expect("page");
    assert_eq!(page.len(), 2, "limit is how a panel pages a 100k library");
}

#[test]
fn a_broken_file_is_a_row_and_an_error() {
    let mut library = Library::in_memory().expect("library");
    let root = library_root("broken");
    let report = scan(&mut library, &root);

    assert_eq!(report.errors, 1);
    let errors = library.errors().expect("errors");
    assert_eq!(errors.len(), 1);
    assert!(errors[0].0.ends_with("broken.mp3"), "{errors:?}");

    let with_missing = Search {
        include_missing: true,
        ..Default::default()
    };
    assert_eq!(library.tracks(&with_missing).expect("tracks").len(), 4);

    let stats = library.stats().expect("stats");
    assert_eq!(stats.tracks, 4, "an unplayable file is still indexed");
    assert_eq!(stats.errors, 1);
}

/// Concurrency must not change the result: the readers are independent, the writer is
/// single, and a scan with one thread and with six has to agree row for row.
/// A warm rescan reads no tags, so it must not forget what the last real read found.
#[test]
fn a_warm_rescan_keeps_the_error_list() {
    let mut library = Library::in_memory().expect("library");
    let root = library_root("errors-kept");
    scan(&mut library, &root);
    assert_eq!(library.errors().expect("errors").len(), 1);

    let report = scan(&mut library, &root);
    assert_eq!(report.unchanged, 4);
    let errors = library.errors().expect("errors");
    assert_eq!(
        errors.len(),
        1,
        "a warm scan must not wipe the error list: {errors:?}"
    );
    assert!(errors[0].0.ends_with("broken.mp3"));
}

/// And a file that stops being broken clears its own row.
#[test]
fn fixing_a_file_clears_its_error() {
    let mut library = Library::in_memory().expect("library");
    let root = library_root("errors-cleared");
    scan(&mut library, &root);
    assert_eq!(library.errors().expect("errors").len(), 1);

    std::fs::copy(fixtures().join("a1.mp3"), root.join("broken.mp3")).expect("replace");
    let report = scan(&mut library, &root);
    assert_eq!(report.updated, 1);
    assert_eq!(library.errors().expect("errors").len(), 0);
    let stats = library.stats().expect("stats");
    assert_eq!(stats.errors, 0);
    assert_eq!(stats.tracks, 4);
}

#[test]
fn parallel_scan_matches_a_serial_scan() {
    let root = library_root("parallel");

    let mut serial_library = Library::in_memory().expect("library");
    let serial_progress = ScanProgress::new();
    serial_library
        .scan(
            &root,
            ScanOptions {
                read_tags: true,
                threads: 1,
            },
            &serial_progress,
        )
        .expect("serial scan");
    let forward = Search {
        sort: Sort::Path,
        ..Default::default()
    };
    let serial: Vec<(String, Option<String>, Option<i64>)> = serial_library
        .tracks(&forward)
        .expect("tracks")
        .into_iter()
        .map(|r| (r.path, r.title, r.duration_ms))
        .collect();

    let mut parallel_library = Library::in_memory().expect("library");
    let parallel_progress = ScanProgress::new();
    let report = parallel_library
        .scan(
            &root,
            ScanOptions {
                read_tags: true,
                threads: 6,
            },
            &parallel_progress,
        )
        .expect("parallel scan");
    assert_eq!(report.threads, 6);
    let parallel: Vec<(String, Option<String>, Option<i64>)> = parallel_library
        .tracks(&forward)
        .expect("tracks")
        .into_iter()
        .map(|r| (r.path, r.title, r.duration_ms))
        .collect();

    assert_eq!(serial, parallel);
    assert_eq!(serial.len(), 4);
}

#[test]
fn stats_roll_up_the_library() {
    let mut library = Library::in_memory().expect("library");
    let root = library_root("stats");
    scan(&mut library, &root);

    let stats = library.stats().expect("stats");
    assert_eq!(stats.tracks, 4);
    assert_eq!(stats.albums, 2);
    assert_eq!(stats.artists, 2);
    assert!(stats.duration_ms >= 3_000, "{}", stats.duration_ms);
    let mp3 = stats
        .by_codec
        .iter()
        .find(|(c, _)| c == "mp3")
        .expect("mp3");
    assert_eq!(mp3.1, 3);
}
