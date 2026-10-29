//! Album covers (SCOPE section 5): lazy, one row per album, decoded once.
//!
//! Indexing never reads a JPEG -- that is the whole reason the scanner disables cover-art
//! parsing. This module is the other half of that bargain: when a panel asks for an album's
//! cover, find it (embedded picture first, then a sidecar file), downscale it to a 96px
//! thumbnail, store the JPEG, and hand back raw RGB for the renderer.

use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::TaggedFileExt;
use lofty::probe::Probe;
use rusqlite::{params, Connection, OptionalExtension};

use crate::{Error, Result};

/// Cache key for an album: the grouping the whole app uses (album artist, falling back to
/// the track artist), so two panels asking about the same album agree.
pub fn album_key(albumartist: Option<&str>, album: &str) -> String {
    format!("{}\u{1f}{}", albumartist.unwrap_or("").trim(), album.trim())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Art {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGB, width * height * 3 bytes.
    pub rgb: Vec<u8>,
}

const THUMB: u32 = 96;

pub fn cached(conn: &Connection, albumartist: Option<&str>, album: &str) -> Result<Option<Art>> {
    let key = album_key(albumartist, album);
    let row: Option<(i64, i64, Vec<u8>)> = conn
        .query_row(
            "SELECT width, height, thumb FROM art WHERE album_key = ?1",
            [&key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((width, height, thumb)) = row else {
        return Ok(None);
    };

    let decoded = image::load_from_memory(&thumb).map_err(|e| Error::Art(e.to_string()))?;
    let rgb = decoded.to_rgb8();
    Ok(Some(Art {
        width: width as u32,
        height: height as u32,
        rgb: rgb.into_raw(),
    }))
}

pub fn fetch(conn: &Connection, albumartist: Option<&str>, album: &str) -> Result<Option<Art>> {
    let Some(track) = first_track(conn, albumartist, album)? else {
        return Ok(None);
    };

    let (bytes, source) = match embedded_picture(&track) {
        Some((bytes, mime)) => (bytes, format!("embedded:{}:{mime}", track.display())),
        None => match sidecar(&track) {
            Some((bytes, path)) => (bytes, path.display().to_string()),
            None => return Ok(None),
        },
    };

    let Ok(image) = image::load_from_memory(&bytes) else {
        return Ok(None);
    };
    let thumb = square_thumb(image);
    let mut jpeg = Vec::new();
    {
        let mut cursor = std::io::Cursor::new(&mut jpeg);
        thumb
            .write_to(&mut cursor, image::ImageFormat::Jpeg)
            .map_err(|e| Error::Art(e.to_string()))?;
    }

    let key = album_key(albumartist, album);
    conn.execute(
        "INSERT OR REPLACE INTO art(album_key, source, width, height, thumb, at)
         VALUES (?1, ?2, ?3, ?4, ?5, strftime('%s','now'))",
        params![key, source, THUMB as i64, THUMB as i64, jpeg],
    )?;

    let rgb = thumb.into_raw();
    Ok(Some(Art {
        width: THUMB,
        height: THUMB,
        rgb,
    }))
}

fn first_track(
    conn: &Connection,
    albumartist: Option<&str>,
    album: &str,
) -> Result<Option<std::path::PathBuf>> {
    let path: Option<String> = conn
        .query_row(
            "SELECT path FROM tracks
             WHERE missing = 0 AND album = ?1
               AND COALESCE(NULLIF(albumartist, ''), artist) IS ?2
             ORDER BY COALESCE(disc_no, 0), COALESCE(track_no, 0), filename
             LIMIT 1",
            params![album.trim(), albumartist.map(|v| v.trim())],
            |row| row.get(0),
        )
        .optional()?;
    Ok(path.map(std::path::PathBuf::from))
}

/// Any picture in the file, not only the front cover: most files have exactly one, and a
/// back cover tile is better than a blank one.
fn embedded_picture(track: &Path) -> Option<(Vec<u8>, String)> {
    let tagged = Probe::open(track)
        .ok()?
        .options(ParseOptions::new().read_cover_art(true))
        .read()
        .ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let picture = tag.pictures().first()?;
    let mime = picture
        .mime_type()
        .map(|m| m.to_string())
        .unwrap_or_else(|| "image/jpeg".to_string());
    Some((picture.data().to_vec(), mime))
}

fn sidecar(track: &Path) -> Option<(Vec<u8>, std::path::PathBuf)> {
    let dir = track.parent()?;
    let names = [
        "cover.jpg",
        "Cover.jpg",
        "folder.jpg",
        "Folder.jpg",
        "front.jpg",
        "cover.png",
        "Cover.png",
    ];
    for name in names {
        let path = dir.join(name);
        if let Ok(bytes) = std::fs::read(&path) {
            return Some((bytes, path));
        }
    }
    None
}

/// Fit the picture into a square thumbnail, cropping the long edge: a tile is square, and
/// letterboxing a wide cover would waste half the cells.
fn square_thumb(image: image::DynamicImage) -> image::RgbImage {
    let (width, height) = (image.width(), image.height());
    let side = width.min(height).max(1);
    let x = (width - side) / 2;
    let y = (height - side) / 2;
    let cropped = image.crop_imm(x, y, side, side);
    cropped
        .resize_exact(THUMB, THUMB, image::imageops::FilterType::Triangle)
        .to_rgb8()
}
