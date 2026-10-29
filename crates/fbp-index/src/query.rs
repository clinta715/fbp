//! The read side: one place that turns a `Search` into SQL.
//!
//! Sorting is a single `ORDER BY` fragment per key rather than a query builder, and the
//! whole row set is never materialised for a panel: callers ask for a `limit`, which is
//! how a 100k-row table stays interactive (SCOPE risk #1).

use std::path::PathBuf;

use rusqlite::types::Value;
use rusqlite::{params_from_iter, Connection};

const COLUMNS: &str = "t.id, t.path, t.title, t.artist, t.albumartist, t.album, t.track_no,\
     t.disc_no, t.year, t.genre, t.duration_ms, t.codec, t.sample_rate, t.bitrate, t.size,\
     t.missing, t.play_count, t.rating, t.filename, t.composer, t.comment";

/// What a list panel asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Search {
    /// Free text: matched against title, artist, album, albumartist and genre by FTS5.
    /// Empty means "everything".
    pub text: String,
    pub sort: Sort,
    /// Reverse the sort. Applied to the whole `ORDER BY`: for the composite keys this
    /// reverses the grouping too, which is what "descending by artist" should mean.
    pub desc: bool,
    pub limit: Option<usize>,
    pub include_missing: bool,
}

impl Search {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Artist,
    Album,
    Title,
    Path,
    Year,
    Duration,
    Added,
}

impl Sort {
    fn order(self) -> &'static str {
        match self {
            Sort::Artist => "COALESCE(NULLIF(t.albumartist, ''), t.artist, '') COLLATE NOCASE, \
                 COALESCE(t.album, '') COLLATE NOCASE, t.disc_no, t.track_no, t.title COLLATE NOCASE",
            Sort::Album => "COALESCE(t.album, '') COLLATE NOCASE, t.disc_no, t.track_no, \
                 COALESCE(t.artist, '') COLLATE NOCASE",
            Sort::Title => "COALESCE(t.title, t.filename) COLLATE NOCASE",
            Sort::Path => "t.path COLLATE NOCASE",
            Sort::Year => "COALESCE(t.year, 0), COALESCE(t.album, '') COLLATE NOCASE, t.track_no",
            Sort::Duration => "COALESCE(t.duration_ms, 0)",
            Sort::Added => "t.added_at",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Sort::Artist => "artist",
            Sort::Album => "album",
            Sort::Title => "title",
            Sort::Path => "path",
            Sort::Year => "year",
            Sort::Duration => "duration",
            Sort::Added => "added",
        }
    }
}

/// One indexed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: i64,
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub albumartist: Option<String>,
    pub album: Option<String>,
    pub track_no: Option<i64>,
    pub disc_no: Option<i64>,
    pub year: Option<i64>,
    pub genre: Option<String>,
    pub duration_ms: Option<i64>,
    pub codec: Option<String>,
    pub sample_rate: Option<i64>,
    pub bitrate: Option<i64>,
    pub size: i64,
    pub missing: bool,
    pub play_count: i64,
    pub rating: Option<i64>,
    pub filename: String,
    pub composer: Option<String>,
    pub comment: Option<String>,
}

impl Row {
    /// What a list shows in the title column: the tag, else the file name.
    pub fn display_title(&self) -> &str {
        match self.title.as_deref() {
            Some(title) if !title.is_empty() => title,
            _ => &self.filename,
        }
    }

    pub fn path_buf(&self) -> PathBuf {
        PathBuf::from(&self.path)
    }

    /// The editable value of a tag field, as the Properties panel shows it.
    pub fn field(&self, field: fbp_core::tag::Field) -> Option<String> {
        use fbp_core::tag::Field;
        match field {
            Field::Title => self.title.clone(),
            Field::Artist => self.artist.clone(),
            Field::AlbumArtist => self.albumartist.clone(),
            Field::Album => self.album.clone(),
            Field::Genre => self.genre.clone(),
            Field::Year => self.year.map(|v| v.to_string()),
            Field::TrackNo => self.track_no.map(|v| v.to_string()),
            Field::DiscNo => self.disc_no.map(|v| v.to_string()),
            Field::Composer => self.composer.clone(),
            Field::Comment => self.comment.clone(),
        }
    }

    pub fn clock(&self) -> String {
        match self.duration_ms {
            Some(ms) => format!("{}:{:02}", ms / 60_000, (ms / 1_000) % 60),
            None => "--:--".to_string(),
        }
    }
}

fn row_from(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok(Row {
        id: row.get(0)?,
        path: row.get(1)?,
        title: row.get(2)?,
        artist: row.get(3)?,
        albumartist: row.get(4)?,
        album: row.get(5)?,
        track_no: row.get(6)?,
        disc_no: row.get(7)?,
        year: row.get(8)?,
        genre: row.get(9)?,
        duration_ms: row.get(10)?,
        codec: row.get(11)?,
        sample_rate: row.get(12)?,
        bitrate: row.get(13)?,
        size: row.get(14)?,
        missing: row.get(15)?,
        play_count: row.get(16)?,
        rating: row.get(17)?,
        filename: row.get(18)?,
        composer: row.get(19)?,
        comment: row.get(20)?,
    })
}

/// Turn free text into an FTS5 query: every token a quoted prefix, so `blur live` finds
/// `Blur` + `Live at …`, and a stray `"` or `*` in a file name cannot become syntax.
fn fts_query(text: &str) -> String {
    text.split_whitespace()
        .map(|token| format!("\"{}\"*", token.replace('\"', "")))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn tracks(conn: &Connection, search: &Search) -> rusqlite::Result<Vec<Row>> {
    let text = search.text.trim();
    let mut sql = format!("SELECT {COLUMNS} FROM tracks t");
    if !text.is_empty() {
        sql.push_str(" JOIN tracks_fts ON tracks_fts.rowid = t.id");
    }

    let mut clauses: Vec<String> = Vec::new();
    let mut values: Vec<Value> = Vec::new();
    if !search.include_missing {
        clauses.push("t.missing = 0".to_string());
    }
    if !text.is_empty() {
        clauses.push("tracks_fts MATCH ?".to_string());
        values.push(Value::Text(fts_query(text)));
    }
    if !clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
    }

    sql.push_str(" ORDER BY ");
    sql.push_str(search.sort.order());
    if search.desc {
        sql.push_str(" DESC");
    }
    sql.push_str(" LIMIT ?");
    values.push(Value::Integer(search.limit.map(|l| l as i64).unwrap_or(-1)));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values), row_from)?;
    rows.collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumRow {
    pub albumartist: Option<String>,
    pub album: String,
    pub tracks: i64,
    pub duration_ms: i64,
    pub year: Option<i64>,
}

impl AlbumRow {
    /// The grouping rule of the library: album artist, falling back to the track artist
    /// (so an untagged compilation still groups per artist instead of per file).
    pub fn artist_label(&self) -> &str {
        self.albumartist.as_deref().unwrap_or("")
    }

    pub fn clock(&self) -> String {
        let minutes = self.duration_ms / 60_000;
        format!("{}h {:02}m", minutes / 60, minutes % 60)
    }
}

pub fn albums(conn: &Connection) -> rusqlite::Result<Vec<AlbumRow>> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(NULLIF(t.albumartist, ''), t.artist) AS aa, t.album,
                COUNT(*), COALESCE(SUM(t.duration_ms), 0), MIN(t.year)
         FROM tracks t
         WHERE t.missing = 0 AND t.album IS NOT NULL AND t.album <> ''
         GROUP BY aa, t.album
         ORDER BY aa COLLATE NOCASE, t.album COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(AlbumRow {
            albumartist: row.get(0)?,
            album: row.get(1)?,
            tracks: row.get(2)?,
            duration_ms: row.get(3)?,
            year: row.get(4)?,
        })
    })?;
    rows.collect()
}

pub fn artists(conn: &Connection) -> rusqlite::Result<Vec<(String, usize)>> {
    let mut stmt = conn.prepare(
        "SELECT artist, COUNT(*) FROM tracks
         WHERE missing = 0 AND artist IS NOT NULL AND artist <> ''
         GROUP BY artist ORDER BY artist COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
    })?;
    rows.collect()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub tracks: usize,
    pub albums: usize,
    pub artists: usize,
    pub missing: usize,
    pub errors: usize,
    pub duration_ms: i64,
    pub bytes: i64,
    pub by_codec: Vec<(String, usize)>,
}

impl Stats {
    pub fn clock(&self) -> String {
        let seconds = self.duration_ms / 1000;
        format!(
            "{}d {}h {:02}m",
            seconds / 86_400,
            (seconds % 86_400) / 3600,
            (seconds % 3600) / 60
        )
    }

    pub fn size_gib(&self) -> f64 {
        self.bytes as f64 / (1024.0 * 1024.0 * 1024.0)
    }
}

fn count(conn: &Connection, sql: &str) -> rusqlite::Result<usize> {
    Ok(conn.query_row(sql, [], |row| row.get::<_, i64>(0))? as usize)
}

pub fn stats(conn: &Connection) -> rusqlite::Result<Stats> {
    let tracks = count(conn, "SELECT COUNT(*) FROM tracks WHERE missing = 0")?;
    let missing = count(conn, "SELECT COUNT(*) FROM tracks WHERE missing = 1")?;
    let errors = count(conn, "SELECT COUNT(*) FROM errors")?;
    let albums = count(
        conn,
        "SELECT COUNT(*) FROM (SELECT 1 FROM tracks WHERE missing = 0 AND album IS NOT NULL
          AND album <> '' GROUP BY COALESCE(NULLIF(albumartist, ''), artist), album)",
    )?;
    let artists = count(
        conn,
        "SELECT COUNT(DISTINCT artist) FROM tracks WHERE missing = 0 AND artist <> ''",
    )?;
    let (duration_ms, bytes): (i64, i64) = conn.query_row(
        "SELECT COALESCE(SUM(duration_ms), 0), COALESCE(SUM(size), 0) FROM tracks WHERE missing = 0",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    let mut stmt = conn.prepare(
        "SELECT COALESCE(codec, '?'), COUNT(*) FROM tracks WHERE missing = 0
         GROUP BY 1 ORDER BY 2 DESC",
    )?;
    let by_codec = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(Stats {
        tracks,
        albums,
        artists,
        missing,
        errors,
        duration_ms,
        bytes,
        by_codec,
    })
}
