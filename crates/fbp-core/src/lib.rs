//! Model types for fbp: no I/O, no threads, no dependencies.
//!
//! P0 owns exactly one type (`Track`) and one policy (`is_audio`). The library
//! proper (`Album`, `View`, `Layout`, `Overlay`) arrives in P1/P2; this crate is
//! where they land so the audio and index layers never grow their own notions of
//! what a track is.

use std::path::{Path, PathBuf};

/// A file the player can be pointed at.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Track {
    pub path: PathBuf,
}

impl Track {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The name shown in a playlist: the file name, or the full path if there is none.
    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

/// Extensions the walk hands to the decoder.
///
/// `opus` is here on purpose: P0 indexes and *lists* opus files and reports them as
/// unplayable (SCOPE D3) rather than hiding them, so tuning the walk never silently
/// changes what the user sees.
pub const AUDIO_EXTS: &[&str] = &[
    "mp3", "flac", "wav", "ogg", "oga", "m4a", "mp4", "aac", "alac", "opus",
];

/// Does this path look like audio? Extension-only: P0 has no tag reader yet.
pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            AUDIO_EXTS.contains(&e.as_str())
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_matching_is_case_insensitive() {
        assert!(is_audio(Path::new("/x/a.MP3")));
        assert!(is_audio(Path::new("/x/a.flac")));
        assert!(!is_audio(Path::new("/x/a.txt")));
        assert!(!is_audio(Path::new("/x/a")));
    }

    #[test]
    fn file_name_falls_back_to_the_path() {
        assert_eq!(Track::new("/x/y/z.mp3").file_name(), "z.mp3");
        assert_eq!(Track::new("/").file_name(), "/");
    }
}
