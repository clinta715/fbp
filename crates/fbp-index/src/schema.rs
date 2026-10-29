//! The schema, and the one rule that keeps scans fast.
//!
//! A rescan of 115k unchanged files must not write 115k rows: `seen` lives in a TEMP
//! table per scan, so an unchanged file costs one `SELECT` and no write at all — which
//! also means the FTS triggers below fire only when a tag actually changed.

use rusqlite::Connection;

pub const VERSION: i64 = 2;

const V1: &str = r#"
CREATE TABLE tracks (
    id          INTEGER PRIMARY KEY,
    path        TEXT    NOT NULL UNIQUE,
    root        TEXT    NOT NULL,
    dir         TEXT    NOT NULL,
    filename    TEXT    NOT NULL,
    ext         TEXT    NOT NULL,
    size        INTEGER NOT NULL,
    mtime       INTEGER NOT NULL,
    inode       INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER,
    codec       TEXT,
    sample_rate INTEGER,
    channels    INTEGER,
    bitrate     INTEGER,
    title       TEXT,
    artist      TEXT,
    albumartist TEXT,
    album       TEXT,
    track_no    INTEGER,
    disc_no     INTEGER,
    disc_total  INTEGER,
    year        INTEGER,
    genre       TEXT,
    composer    TEXT,
    comment     TEXT,
    art_id      INTEGER,
    added_at    INTEGER NOT NULL,
    last_played INTEGER,
    play_count  INTEGER NOT NULL DEFAULT 0,
    skip_count  INTEGER NOT NULL DEFAULT 0,
    rating      INTEGER,
    missing     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX tracks_dir ON tracks(root, dir);
CREATE INDEX tracks_album ON tracks(albumartist, album, disc_no, track_no);
CREATE INDEX tracks_artist ON tracks(artist);
CREATE INDEX tracks_missing ON tracks(missing);

-- The search index keeps its own copy of the text: a scan updates five short columns at
-- most, and an external-content table would need a 'delete' command per row per change.
CREATE VIRTUAL TABLE tracks_fts USING fts5(
    title, artist, album, albumartist, genre,
    tokenize = "unicode61 remove_diacritics 2"
);
CREATE TRIGGER tracks_ai AFTER INSERT ON tracks BEGIN
    INSERT INTO tracks_fts(rowid, title, artist, album, albumartist, genre)
    VALUES (new.id, new.title, new.artist, new.album, new.albumartist, new.genre);
END;
CREATE TRIGGER tracks_ad AFTER DELETE ON tracks BEGIN
    DELETE FROM tracks_fts WHERE rowid = old.id;
END;
CREATE TRIGGER tracks_au AFTER UPDATE OF title, artist, album, albumartist, genre ON tracks BEGIN
    DELETE FROM tracks_fts WHERE rowid = old.id;
    INSERT INTO tracks_fts(rowid, title, artist, album, albumartist, genre)
    VALUES (new.id, new.title, new.artist, new.album, new.albumartist, new.genre);
END;

-- Files that could not be read: a row per file, not a log line that scrolls away.
-- Keyed by path, so a warm rescan (which reads no tags) does not forget them, and a file
-- that is fixed clears its own row the next time it is read.
CREATE TABLE errors (
    id      INTEGER PRIMARY KEY,
    path    TEXT NOT NULL UNIQUE,
    message TEXT NOT NULL,
    at      INTEGER NOT NULL
);

CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
"#;

/// Create or upgrade the database, and set the pragmas a scanning writer wants.
pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;\n         PRAGMA synchronous = NORMAL;\n         PRAGMA temp_store = MEMORY;\n         PRAGMA cache_size = -32000;\n         PRAGMA mmap_size = 268435456;",
    )?;

    let version: Option<i64> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|v| v.parse().ok());

    match version {
        None => {
            conn.execute_batch(V1)?;
            conn.execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES ('schema_version', ?1)",
                [VERSION.to_string()],
            )?;
        }
        Some(found) if found > VERSION => {
            return Err(rusqlite::Error::InvalidQuery);
        }
        Some(1) => {
            // v2: errors are keyed by path so they survive an unchanged rescan.
            conn.execute_batch(
                "DELETE FROM errors WHERE id NOT IN (SELECT MIN(id) FROM errors GROUP BY path);
                 CREATE UNIQUE INDEX errors_path ON errors(path);",
            )?;
            conn.execute(
                "UPDATE meta SET value = ?1 WHERE key = 'schema_version'",
                [VERSION.to_string()],
            )?;
        }
        Some(_) => {}
    }
    Ok(())
}
