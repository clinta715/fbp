//! The library index (SCOPE §5).
//!
//! Owns the SQLite file, the incremental scanner, the tag reader and the queries the
//! panels ask. It knows nothing about the terminal, the audio device or the layout: it
//! takes paths and returns rows, which is what makes it testable against a fixture tree
//! and what keeps every panel's data identical.
//!
//! Two properties are deliberate:
//! - **A rescan is a stat pass.** Unchanged files are `SELECT`ed, never written, so a
//!   warm scan of 115k files is seconds and does not churn the FTS index.
//! - **A broken file is a row.** It lands in `tracks` (with whatever the container would
//!   tell us) *and* in `errors`, so an unplayable file is visible instead of absent.

mod query;
mod scan;
mod schema;

use std::path::{Path, PathBuf};

use rusqlite::Connection;

pub use query::{AlbumRow, Row, Search, Sort, Stats};
pub use scan::{ScanOptions, ScanProgress, ScanReport};

#[derive(Debug)]
pub enum Error {
    Sql(rusqlite::Error),
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Sql(e) => write!(f, "database: {e}"),
            Error::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Sql(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct Library {
    conn: Connection,
    path: PathBuf,
}

impl Library {
    /// Open (creating and migrating as needed) the library at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(&path)?;
        schema::migrate(&conn)?;
        Ok(Self { conn, path })
    }

    /// An in-memory library, for tests and `--dry-run` style work.
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        schema::migrate(&conn)?;
        Ok(Self {
            conn,
            path: PathBuf::from(":memory:"),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The root the last scan walked, so a bare `fbp scan` needs no arguments.
    pub fn root(&self) -> Option<String> {
        self.meta("root")
    }

    pub fn set_root(&self, root: &Path) -> Result<()> {
        self.set_meta("root", &root.display().to_string())
    }

    pub fn meta(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .ok()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)",
            [key, value],
        )?;
        Ok(())
    }

    /// Walk a root and bring the index up to date (see `scan.rs`).
    pub fn scan(
        &mut self,
        root: &Path,
        options: ScanOptions,
        progress: &ScanProgress,
    ) -> Result<ScanReport> {
        self.set_root(root)?;
        scan::scan(&mut self.conn, root, options, progress)
    }

    pub fn tracks(&self, search: &Search) -> Result<Vec<Row>> {
        Ok(query::tracks(&self.conn, search)?)
    }

    /// Files for the player: the same query, path-only.
    pub fn paths(&self, search: &Search) -> Result<Vec<PathBuf>> {
        Ok(query::tracks(&self.conn, search)?
            .into_iter()
            .map(|r| r.path.into())
            .collect())
    }

    pub fn albums(&self) -> Result<Vec<AlbumRow>> {
        Ok(query::albums(&self.conn)?)
    }

    pub fn artists(&self) -> Result<Vec<(String, usize)>> {
        Ok(query::artists(&self.conn)?)
    }

    pub fn stats(&self) -> Result<Stats> {
        Ok(query::stats(&self.conn)?)
    }

    /// The last scan's failures.
    pub fn errors(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, message FROM errors ORDER BY id")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn count(&self) -> Result<usize> {
        let count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM tracks WHERE missing = 0", [], |row| {
                    row.get(0)
                })?;
        Ok(count as usize)
    }
}
