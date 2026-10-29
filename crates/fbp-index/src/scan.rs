//! The incremental scanner (SCOPE §5).
//!
//! Three costs, and the shape of the code follows their order:
//! 1. **Metadata** for every file (`stat`). Cheap and parallel — a full metadata pass
//!    over 99k files on this machine's spinning RAID1 is ~2 s.
//! 2. **Tags**, only for files whose `(size, mtime, inode)` changed. This is the expensive
//!    one: it opens and reads each file. Cover art is *not* parsed here (`read_cover_art(false)`)
//!    — art is fetched lazily for the albums a panel actually shows — so a cold scan does
//!    not pull 20 GB of JPEGs through the disk to fill a table.
//! 3. **Writes**, batched 512 rows per transaction by a single writer thread.
//!
//! Readers are parallel and the writer is single: SQLite has one writer by design, and
//! the work that benefits from concurrency is the IO, not the inserts. Removals are
//! computed in memory from the paths seen, so a rescan of an unchanged library does no
//! track writes at all.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::Mutex;
use std::time::{Duration, Instant, UNIX_EPOCH};

use lofty::config::ParseOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey};
use rusqlite::{params, params_from_iter, Connection};
use walkdir::WalkDir;

use fbp_core::is_audio;

use crate::{Error, Result};

#[derive(Debug, Clone, Copy)]
pub struct ScanOptions {
    /// Off makes a scan a pure shape pass (no file reads at all) — useful on a slow mount
    /// and for tests that only care about paths.
    pub read_tags: bool,
    /// Reader threads. `0` picks `available_parallelism` clamped to `1..=8`: on a spinning
    /// disk a handful of readers keeps the IO queue full, past that the seeks fight.
    pub threads: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            read_tags: true,
            threads: 0,
        }
    }
}

/// Live scan state, readable from a UI thread while the scan runs.
#[derive(Default)]
pub struct ScanProgress {
    total: AtomicUsize,
    scanned: AtomicUsize,
    changed: AtomicUsize,
    errors: AtomicUsize,
    cancelled: AtomicBool,
    finished: AtomicBool,
    current: Mutex<String>,
}

impl ScanProgress {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    pub fn scanned(&self) -> usize {
        self.scanned.load(Ordering::Relaxed)
    }

    pub fn changed(&self) -> usize {
        self.changed.load(Ordering::Relaxed)
    }

    pub fn errors(&self) -> usize {
        self.errors.load(Ordering::Relaxed)
    }

    pub fn current(&self) -> String {
        self.current.lock().map(|c| c.clone()).unwrap_or_default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// True once the scan returned, so a reporter thread can stop polling without knowing
    /// how the scan ended.
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }

    fn touch(&self, path: &Path) {
        if let Ok(mut current) = self.current.lock() {
            current.clear();
            current.push_str(&path.display().to_string());
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    pub scanned: usize,
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub errors: usize,
    pub cancelled: bool,
    pub elapsed: Duration,
    pub threads: usize,
}

/// What a file's tags say, already flattened into the columns the index stores.
#[derive(Debug, Default, Clone)]
struct Tags {
    duration_ms: Option<i64>,
    codec: Option<String>,
    sample_rate: Option<i64>,
    channels: Option<i64>,
    bitrate: Option<i64>,
    title: Option<String>,
    artist: Option<String>,
    albumartist: Option<String>,
    album: Option<String>,
    track_no: Option<i64>,
    disc_no: Option<i64>,
    disc_total: Option<i64>,
    year: Option<i64>,
    genre: Option<String>,
    composer: Option<String>,
    comment: Option<String>,
}

/// A file the writer has to store.
struct Prepared {
    path: String,
    dir: String,
    filename: String,
    ext: String,
    size: i64,
    mtime: i64,
    inode: i64,
    tags: Tags,
    existed: bool,
    /// Whether this scan actually tried to read the file's tags. An unchanged file does not
    /// (that is the whole point of the fast path), so its error row is left alone.
    read: bool,
    /// Why the tags could not be read, if they could not: the row is still written and the
    /// reason becomes a row in `errors`.
    failure: Option<String>,
}

enum Work {
    Unchanged(String),
    Changed(Box<Prepared>),
    Failed { path: String, message: String },
}

fn text(value: Option<std::borrow::Cow<'_, str>>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn codec_name(file_type: lofty::file::FileType) -> String {
    match file_type {
        lofty::file::FileType::Mpeg => "mp3".to_string(),
        other => format!("{other:?}").to_ascii_lowercase(),
    }
}

/// Read tags without reading cover art: the expensive bytes in an mp3 tag are the JPEGs,
/// and nothing in the index needs them (art is fetched per album, lazily).
fn read_tags(path: &Path) -> std::result::Result<Tags, String> {
    let tagged = Probe::open(path)
        .map_err(|e| e.to_string())?
        .options(ParseOptions::new().read_cover_art(false))
        .read()
        .map_err(|e| e.to_string())?;

    let property = tagged.properties();
    let mut tags = Tags {
        duration_ms: {
            let duration = property.duration();
            (!duration.is_zero()).then_some(duration.as_millis() as i64)
        },
        codec: Some(codec_name(tagged.file_type())),
        sample_rate: property.sample_rate().map(|v| v as i64),
        channels: property.channels().map(|v| v as i64),
        bitrate: property.audio_bitrate().map(|v| v as i64),
        ..Default::default()
    };

    if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
        tags.title = text(tag.title());
        tags.artist = text(tag.artist());
        tags.album = text(tag.album());
        tags.genre = text(tag.genre());
        tags.comment = text(tag.comment());
        tags.albumartist = tag
            .get_string(ItemKey::AlbumArtist)
            .map(|v| v.trim().to_string());
        tags.composer = tag
            .get_string(ItemKey::Composer)
            .map(|v| v.trim().to_string());
        tags.track_no = tag.track().map(|v| v as i64);
        tags.disc_no = tag.disk().map(|v| v as i64);
        tags.disc_total = tag.disk_total().map(|v| v as i64);
        tags.year = tag.date().map(|date| date.year as i64);
    }
    Ok(tags)
}

fn millis(time: std::time::SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(unix)]
fn inode(meta: &std::fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    meta.ino() as i64
}

#[cfg(not(unix))]
fn inode(_meta: &std::fs::Metadata) -> i64 {
    0
}

/// One reader thread's job: stat a file, decide whether it changed, and read tags if it did.
fn prepare(
    path: &Path,
    known: &HashMap<String, (i64, i64, i64, i64)>,
    options: ScanOptions,
) -> Work {
    let path_string = path.display().to_string();
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(error) => {
            return Work::Failed {
                path: path_string,
                message: error.to_string(),
            }
        }
    };
    let size = meta.len() as i64;
    let mtime = meta.modified().map(millis).unwrap_or_default();
    let inode = inode(&meta);

    let existed = match known.get(&path_string) {
        Some(&(_, known_size, known_mtime, known_inode))
            if known_size == size && known_mtime == mtime && known_inode == inode =>
        {
            return Work::Unchanged(path_string);
        }
        Some(_) => true,
        None => false,
    };

    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    let (tags, failure) = if options.read_tags {
        match read_tags(path) {
            Ok(tags) => (tags, None),
            Err(message) => (Tags::default(), Some(message)),
        }
    } else {
        (Tags::default(), None)
    };

    let mut tags = tags;
    if tags.codec.is_none() {
        // An unreadable file still says what it was meant to be; `errors` carries why it
        // will not play.
        tags.codec = Some(ext.clone());
    }

    Work::Changed(Box::new(Prepared {
        dir: path
            .parent()
            .map(|d| d.display().to_string())
            .unwrap_or_default(),
        filename: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        ext,
        size,
        mtime,
        inode,
        tags,
        existed,
        read: options.read_tags,
        failure,
        path: path_string,
    }))
}

const UPSERT: &str = "\
INSERT INTO tracks (path, root, dir, filename, ext, size, mtime, inode, added_at,
                    duration_ms, codec, sample_rate, channels, bitrate, title, artist,
                    albumartist, album, track_no, disc_no, disc_total, year, genre,
                    composer, comment, missing)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
        ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, 0)
ON CONFLICT(path) DO UPDATE SET
    root = excluded.root, dir = excluded.dir, filename = excluded.filename,
    ext = excluded.ext, size = excluded.size, mtime = excluded.mtime,
    inode = excluded.inode, duration_ms = excluded.duration_ms, codec = excluded.codec,
    sample_rate = excluded.sample_rate, channels = excluded.channels,
    bitrate = excluded.bitrate, title = excluded.title, artist = excluded.artist,
    albumartist = excluded.albumartist, album = excluded.album, track_no = excluded.track_no,
    disc_no = excluded.disc_no, disc_total = excluded.disc_total, year = excluded.year,
    genre = excluded.genre, composer = excluded.composer, comment = excluded.comment,
    missing = 0";

pub fn scan(
    conn: &mut Connection,
    root: &Path,
    options: ScanOptions,
    progress: &ScanProgress,
) -> Result<ScanReport> {
    let result = scan_inner(conn, root, options, progress);
    progress.finished.store(true, Ordering::Relaxed);
    result
}

fn scan_inner(
    conn: &mut Connection,
    root: &Path,
    options: ScanOptions,
    progress: &ScanProgress,
) -> Result<ScanReport> {
    let started = Instant::now();
    if !root.is_dir() {
        return Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("not a directory: {}", root.display()),
        )));
    }

    let root_string = root.display().to_string();

    // The walk itself is cheap: it reads directory entries, not files.
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() && is_audio(entry.path()) {
            files.push(entry.path().to_path_buf());
        }
    }
    files.sort();
    progress.total.store(files.len(), Ordering::Relaxed);

    // One query instead of one per file: this is what makes the metadata pass cheap.
    let mut known: HashMap<String, (i64, i64, i64, i64)> = HashMap::with_capacity(files.len());
    {
        let mut stmt =
            conn.prepare("SELECT path, id, size, mtime, inode FROM tracks WHERE root = ?1")?;
        let rows = stmt.query_map([&root_string], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?),
            ))
        })?;
        for row in rows {
            let (path, data) = row?;
            known.insert(path, data);
        }
    }

    let mut report = ScanReport::default();
    let threads = match options.threads {
        0 => std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(1, 8),
        n => n.max(1),
    };
    report.threads = threads;

    let now = millis(std::time::SystemTime::now());
    let (sender, receiver) = sync_channel::<Work>(4096);
    let mut seen: HashSet<String> = HashSet::with_capacity(files.len());

    let outcome = std::thread::scope(|scope| -> Result<()> {
        let chunk = files.len().div_ceil(threads).max(1);
        for slice in files.chunks(chunk) {
            let sender = sender.clone();
            let known = &known;
            scope.spawn(move || {
                for path in slice {
                    if progress.is_cancelled() {
                        break;
                    }
                    progress.touch(path);
                    if sender.send(prepare(path, known, options)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);

        // The single writer.
        let mut pending: Vec<Prepared> = Vec::with_capacity(512);
        while let Ok(work) = receiver.recv() {
            match work {
                Work::Unchanged(path) => {
                    seen.insert(path);
                    report.unchanged += 1;
                }
                Work::Changed(prepared) => {
                    seen.insert(prepared.path.clone());
                    pending.push(*prepared);
                }
                Work::Failed { path, message } => {
                    report.errors += 1;
                    conn.execute(
                        "INSERT OR REPLACE INTO errors(path, message, at) VALUES (?1, ?2, ?3)",
                        params![path, message, now],
                    )?;
                }
            }

            report.scanned += 1;
            progress.scanned.store(report.scanned, Ordering::Relaxed);

            if pending.len() >= 512 {
                flush(conn, &mut pending, &mut report, now, &root_string)?;
            }
        }
        flush(conn, &mut pending, &mut report, now, &root_string)?;
        Ok(())
    });
    outcome?;

    report.cancelled = progress.is_cancelled();

    // Files indexed from this root that the walk did not produce are missing, not deleted:
    // the row keeps the tags so a moved album can be repaired later (P2).
    let stale: Vec<i64> = known
        .iter()
        .filter(|(path, _)| !seen.contains(*path))
        .map(|(_, (id, ..))| *id)
        .collect();
    for batch in stale.chunks(500) {
        let placeholders: Vec<String> = (1..=batch.len()).map(|i| format!("?{i}")).collect();
        conn.execute(
            &format!(
                "UPDATE tracks SET missing = 1 WHERE missing = 0 AND id IN ({})",
                placeholders.join(", ")
            ),
            params_from_iter(batch.iter()),
        )?;
    }
    report.removed = stale.len();

    report.elapsed = started.elapsed();
    Ok(report)
}

fn flush(
    conn: &mut Connection,
    pending: &mut Vec<Prepared>,
    report: &mut ScanReport,
    now: i64,
    root: &str,
) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    let transaction = conn.transaction()?;
    {
        let mut upsert = transaction.prepare(UPSERT)?;
        let mut record_error = transaction
            .prepare("INSERT OR REPLACE INTO errors(path, message, at) VALUES (?1, ?2, ?3)")?;
        let mut clear_error = transaction.prepare("DELETE FROM errors WHERE path = ?1")?;
        for prepared in pending.drain(..) {
            let tags = &prepared.tags;
            upsert.execute(params![
                &prepared.path,
                root,
                &prepared.dir,
                &prepared.filename,
                &prepared.ext,
                prepared.size,
                prepared.mtime,
                prepared.inode,
                now,
                tags.duration_ms,
                tags.codec,
                tags.sample_rate,
                tags.channels,
                tags.bitrate,
                tags.title,
                tags.artist,
                tags.albumartist,
                tags.album,
                tags.track_no,
                tags.disc_no,
                tags.disc_total,
                tags.year,
                tags.genre,
                tags.composer,
                tags.comment,
            ])?;
            if let Some(message) = prepared.failure {
                report.errors += 1;
                record_error.execute(params![prepared.path, message, now])?;
            } else if prepared.read {
                // The file read cleanly this time: whatever was wrong is no longer true.
                clear_error.execute([&prepared.path])?;
            }
            if prepared.existed {
                report.updated += 1;
            } else {
                report.added += 1;
            }
        }
    }
    transaction.commit()?;
    Ok(())
}
