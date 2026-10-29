//! Tag writing: the only code in the app that mutates a user's files.
//!
//! Three rules, because this is the data-loss surface:
//!
//! 1. **The journal is written before the first file is touched** -- one row per gesture, so a
//!    500-file edit is one undo and a crash mid-batch can still be taken back.
//! 2. **Every file is written atomically**: copy to a temp file in the same directory, edit the
//!    copy, fsync, rename over the original. A kill at any point leaves either the old file or
//!    the new one, never half of either. (The price is a rewrite per file; an in-place writer
//!    with ID3v2 padding would be faster and would lose files on a power cut.)
//! 3. **A patch, not a tag.** A field nobody edited is never written, so a tag editor cannot
//!    silently overwrite values it did not show.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use fbp_core::tag::{Field, Patch};
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::items::Timestamp;
use lofty::tag::{Accessor, ItemKey, Tag};
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{Error, Library, Result};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteReport {
    pub written: usize,
    pub failed: Vec<(PathBuf, String)>,
    pub undo_id: Option<i64>,
}

impl WriteReport {
    pub fn is_ok(&self) -> bool {
        self.failed.is_empty()
    }

    /// A one-line summary for the console panel.
    pub fn summary(&self, verb: &str) -> String {
        if self.failed.is_empty() {
            format!("{verb} {} file(s)", self.written)
        } else {
            format!(
                "{verb} {} file(s), {} failed",
                self.written,
                self.failed.len()
            )
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UndoEntry {
    path: String,
    before: BTreeMap<String, Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UndoPayload {
    entries: Vec<UndoEntry>,
    after: BTreeMap<String, Option<String>>,
}

pub fn write_tags(library: &Library, paths: &[PathBuf], patch: &Patch) -> Result<WriteReport> {
    patch.validate().map_err(Error::Tag)?;
    if patch.is_empty() {
        return Ok(WriteReport::default());
    }
    if paths.is_empty() {
        return Ok(WriteReport::default());
    }

    let mut entries = Vec::with_capacity(paths.len());
    for path in paths {
        entries.push(UndoEntry {
            path: path.display().to_string(),
            before: fields_for(library.connection(), path)?,
        });
    }
    let after: BTreeMap<String, Option<String>> = patch
        .fields
        .iter()
        .map(|(field, value)| (field.key().to_string(), value.clone()))
        .collect();

    // Journal first: if the process dies right after this, `undo_last` still works.
    let label = format!("{} on {} file(s)", patch.describe(), paths.len());
    let undo_id = journal_push(
        library.connection(),
        "tags",
        &label,
        &UndoPayload { entries, after },
    )?;

    let mut report = WriteReport {
        undo_id: Some(undo_id),
        ..Default::default()
    };
    for path in paths {
        match write_file(path, patch) {
            Ok(()) => {
                report.written += 1;
                let path_text = path.display().to_string();
                if let Err(error) = apply_patch(library.connection(), &path_text, patch) {
                    report.failed.push((path.clone(), error.to_string()));
                }
            }
            Err(error) => report.failed.push((path.clone(), error.to_string())),
        }
    }
    Ok(report)
}

pub fn undo_last(library: &Library) -> Result<WriteReport> {
    let row: Option<(i64, String)> = library
        .connection()
        .query_row(
            "SELECT id, payload FROM undo ORDER BY id DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((id, payload)) = row else {
        return Ok(WriteReport::default());
    };
    let payload: UndoPayload = serde_json::from_str(&payload)
        .map_err(|e| Error::Tag(format!("undo journal {id} is unreadable: {e}")))?;

    let mut report = WriteReport {
        undo_id: Some(id),
        ..Default::default()
    };
    for entry in &payload.entries {
        let patch = patch_from_map(&entry.before);
        if patch.is_empty() {
            continue;
        }
        let path = PathBuf::from(&entry.path);
        match write_file(&path, &patch) {
            Ok(()) => {
                report.written += 1;
                if let Err(error) = apply_patch(library.connection(), &entry.path, &patch) {
                    report.failed.push((path, error.to_string()));
                }
            }
            Err(error) => report.failed.push((path, error.to_string())),
        }
    }

    // Only forget the gesture once its files are back: a partial undo must stay retryable.
    if report.failed.is_empty() {
        library
            .connection()
            .execute("DELETE FROM undo WHERE id = ?1", [id])?;
    }
    Ok(report)
}

pub fn undo_depth(conn: &Connection) -> Result<usize> {
    Ok(conn.query_row("SELECT COUNT(*) FROM undo", [], |row| row.get::<_, i64>(0))? as usize)
}

fn patch_from_map(map: &BTreeMap<String, Option<String>>) -> Patch {
    let mut patch = Patch::new();
    for (key, value) in map {
        if let Some(field) = Field::from_key(key) {
            patch = patch.with(field, value.clone());
        }
    }
    patch
}

fn journal_push(conn: &Connection, kind: &str, label: &str, payload: &UndoPayload) -> Result<i64> {
    let json = serde_json::to_string(payload).map_err(|e| Error::Tag(e.to_string()))?;
    conn.execute(
        "INSERT INTO undo(kind, label, payload, at) VALUES (?1, ?2, ?3, strftime('%s','now'))",
        params![kind, label, json],
    )?;
    Ok(conn.last_insert_rowid())
}

/// The field values the index currently holds for a path. The panels show these values, so
/// these are what an undo has to put back.
fn fields_for(conn: &Connection, path: &Path) -> Result<BTreeMap<String, Option<String>>> {
    let path = path.display().to_string();
    // Column types matter here: year, track_no and disc_no are INTEGER, and reading them as
    // text is how a tag editor crashes on its own journal.
    let row: Option<BTreeMap<String, Option<String>>> = conn
        .query_row(
            "SELECT title, artist, albumartist, album, genre, year, track_no, disc_no,
                    composer, comment
             FROM tracks WHERE path = ?1",
            [&path],
            |row| {
                let text = |index: usize| row.get::<_, Option<String>>(index);
                let number = |index: usize| {
                    row.get::<_, Option<i64>>(index)
                        .map(|v| v.map(|n| n.to_string()))
                };
                let mut map = BTreeMap::new();
                map.insert(Field::Title.key().to_string(), text(0)?);
                map.insert(Field::Artist.key().to_string(), text(1)?);
                map.insert(Field::AlbumArtist.key().to_string(), text(2)?);
                map.insert(Field::Album.key().to_string(), text(3)?);
                map.insert(Field::Genre.key().to_string(), text(4)?);
                map.insert(Field::Year.key().to_string(), number(5)?);
                map.insert(Field::TrackNo.key().to_string(), number(6)?);
                map.insert(Field::DiscNo.key().to_string(), number(7)?);
                map.insert(Field::Composer.key().to_string(), text(8)?);
                map.insert(Field::Comment.key().to_string(), text(9)?);
                Ok(map)
            },
        )
        .optional()?;

    Ok(row.unwrap_or_else(|| {
        Field::ALL
            .iter()
            .map(|f| (f.key().to_string(), None))
            .collect()
    }))
}

/// Keep the index in step with the file, so the panels do not need a rescan to show an edit.
fn apply_patch(conn: &Connection, path: &str, patch: &Patch) -> Result<usize> {
    if patch.is_empty() {
        return Ok(0);
    }
    let mut sets = Vec::with_capacity(patch.fields.len());
    let mut values: Vec<Value> = Vec::with_capacity(patch.fields.len() + 1);
    for (field, value) in &patch.fields {
        sets.push(format!("{} = ?", field.column()));
        values.push(match value {
            Some(text) if field.is_numeric() => match text.trim().parse::<i64>() {
                Ok(number) => Value::Integer(number),
                Err(_) => Value::Null,
            },
            Some(text) => Value::Text(text.clone()),
            None => Value::Null,
        });
    }
    values.push(Value::Text(path.to_string()));
    let sql = format!("UPDATE tracks SET {} WHERE path = ?", sets.join(", "));
    Ok(conn.execute(&sql, params_from_iter(values))?)
}

/// Rewrite one file's tags: temp file in the same directory, then rename.
fn write_file(path: &Path, patch: &Patch) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| Error::Tag(format!("{} has no parent directory", path.display())))?;
    // Keep the real extension: lofty (and every other tagger) picks the format from it, and a
    // ".tmp" suffix would make the temp file unparseable.
    let stem = path
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "track".to_string());
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "tmp".to_string());
    let temp = dir.join(format!(".{stem}.fbp{}.{extension}", std::process::id()));

    fs::copy(path, &temp)?;

    let edited = (|| -> Result<()> {
        let mut tagged = Probe::open(&temp)
            .map_err(|e| Error::Tag(e.to_string()))?
            .options(ParseOptions::new().read_cover_art(true))
            .read()
            .map_err(|e| Error::Tag(e.to_string()))?;

        if tagged.primary_tag_mut().is_none() {
            let tag_type = tagged.primary_tag_type();
            tagged.insert_tag(Tag::new(tag_type));
        }
        let tag = tagged
            .primary_tag_mut()
            .ok_or_else(|| Error::Tag("file has no writable tag".to_string()))?;
        apply_to_tag(tag, patch)?;
        tagged
            .save_to_path(&temp, WriteOptions::default())
            .map_err(|e| Error::Tag(format!("{}: {e}", path.display())))?;
        Ok(())
    })();

    if let Err(error) = edited {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }

    // Flush before the rename: a crash must not be able to publish a short file.
    if let Ok(file) = fs::File::open(&temp) {
        let _ = file.sync_all();
    }
    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(Error::Io(error));
    }
    if let Ok(handle) = fs::File::open(dir) {
        let _ = handle.sync_all();
    }
    Ok(())
}

fn apply_to_tag(tag: &mut Tag, patch: &Patch) -> Result<()> {
    for (field, value) in &patch.fields {
        let text = value
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty());
        let number = || -> Result<u32> {
            text.unwrap_or("0")
                .parse::<u32>()
                .map_err(|_| Error::Tag(format!("{} must be a number", field.label())))
        };
        match (field, text) {
            (Field::Title, Some(value)) => tag.set_title(value.to_string()),
            (Field::Title, None) => tag.remove_title(),
            (Field::Artist, Some(value)) => tag.set_artist(value.to_string()),
            (Field::Artist, None) => tag.remove_artist(),
            (Field::Album, Some(value)) => tag.set_album(value.to_string()),
            (Field::Album, None) => tag.remove_album(),
            (Field::Genre, Some(value)) => tag.set_genre(value.to_string()),
            (Field::Genre, None) => tag.remove_genre(),
            (Field::Comment, Some(value)) => tag.set_comment(value.to_string()),
            (Field::Comment, None) => tag.remove_comment(),
            (Field::AlbumArtist, Some(value)) => {
                tag.insert_text(ItemKey::AlbumArtist, value.to_string());
            }
            (Field::AlbumArtist, None) => tag.remove_key(ItemKey::AlbumArtist),
            (Field::Composer, Some(value)) => {
                tag.insert_text(ItemKey::Composer, value.to_string());
            }
            (Field::Composer, None) => tag.remove_key(ItemKey::Composer),
            (Field::TrackNo, Some(_)) => tag.set_track(number()?),
            (Field::TrackNo, None) => tag.remove_track(),
            (Field::DiscNo, Some(_)) => tag.set_disk(number()?),
            (Field::DiscNo, None) => tag.remove_disk(),
            (Field::Year, Some(_)) => tag.set_date(Timestamp {
                year: number()?.min(u16::MAX as u32) as u16,
                month: None,
                day: None,
                hour: None,
                minute: None,
                second: None,
            }),
            (Field::Year, None) => tag.remove_date(),
        }
    }
    Ok(())
}
