//! The editable tag fields, as a model type.
//!
//! One vocabulary shared by the tag writer (which writes files), the index (which keeps the
//! row in step) and the Properties panel (which shows and edits it). If a field is not in
//! `Field`, it cannot be edited -- that is the point of the enum.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Field {
    Title,
    Artist,
    AlbumArtist,
    Album,
    Genre,
    Year,
    TrackNo,
    DiscNo,
    Composer,
    Comment,
}

impl Field {
    pub const ALL: [Field; 10] = [
        Field::Title,
        Field::Artist,
        Field::AlbumArtist,
        Field::Album,
        Field::Genre,
        Field::Year,
        Field::TrackNo,
        Field::DiscNo,
        Field::Composer,
        Field::Comment,
    ];

    /// The stable key: used in the undo journal, in the CLI and in ":tag" style input.
    pub fn key(self) -> &'static str {
        match self {
            Field::Title => "title",
            Field::Artist => "artist",
            Field::AlbumArtist => "albumartist",
            Field::Album => "album",
            Field::Genre => "genre",
            Field::Year => "year",
            Field::TrackNo => "track",
            Field::DiscNo => "disc",
            Field::Composer => "composer",
            Field::Comment => "comment",
        }
    }

    pub fn from_key(key: &str) -> Option<Field> {
        let key = key.trim().to_ascii_lowercase();
        Field::ALL
            .into_iter()
            .find(|field| field.key() == key || field.label() == key || field.column() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Field::Title => "title",
            Field::Artist => "artist",
            Field::AlbumArtist => "album artist",
            Field::Album => "album",
            Field::Genre => "genre",
            Field::Year => "year",
            Field::TrackNo => "track no",
            Field::DiscNo => "disc no",
            Field::Composer => "composer",
            Field::Comment => "comment",
        }
    }

    /// The column this field lives in, in `tracks`.
    pub fn column(self) -> &'static str {
        match self {
            Field::Title => "title",
            Field::Artist => "artist",
            Field::AlbumArtist => "albumartist",
            Field::Album => "album",
            Field::Genre => "genre",
            Field::Year => "year",
            Field::TrackNo => "track_no",
            Field::DiscNo => "disc_no",
            Field::Composer => "composer",
            Field::Comment => "comment",
        }
    }

    pub fn is_numeric(self) -> bool {
        matches!(self, Field::Year | Field::TrackNo | Field::DiscNo)
    }
}

/// A set of tag changes: a field absent from the list is *untouched*, `None` clears it, and
/// `Some(text)` sets it. "Untouched" is the reason patches rather than whole tags: writing a
/// field nobody edited is how tag editors lose data.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Patch {
    pub fields: Vec<(Field, Option<String>)>,
}

impl Patch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set one field to a value (an empty string clears it).
    pub fn set(mut self, field: Field, value: impl Into<String>) -> Self {
        let value = value.into();
        let keep = !value.trim().is_empty();
        self.fields.push((field, keep.then_some(value)));
        self
    }

    pub fn clear(mut self, field: Field) -> Self {
        self.fields.push((field, None));
        self
    }

    pub fn with(mut self, field: Field, value: Option<String>) -> Self {
        self.fields.push((field, value));
        self
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    pub fn get(&self, field: Field) -> Option<Option<&str>> {
        self.fields
            .iter()
            .find(|(f, _)| *f == field)
            .map(|(_, v)| v.as_deref())
    }

    /// Validate the numeric fields before anything touches a file.
    pub fn validate(&self) -> Result<(), String> {
        for (field, value) in &self.fields {
            if field.is_numeric() {
                if let Some(text) = value {
                    text.trim()
                        .parse::<u32>()
                        .map_err(|_| format!("{} must be a number, got {text:?}", field.label()))?;
                }
            }
        }
        Ok(())
    }

    /// A short description for a log line: "artist=Blur, year=1994".
    pub fn describe(&self) -> String {
        self.fields
            .iter()
            .map(|(field, value)| match value {
                Some(text) => format!("{}={text}", field.key()),
                None => format!("{}=\u{2205}", field.key()),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_round_trip_through_their_keys() {
        for field in Field::ALL {
            assert_eq!(Field::from_key(field.key()), Some(field));
            assert_eq!(Field::from_key(field.column()), Some(field));
        }
        assert_eq!(Field::from_key("album artist"), Some(Field::AlbumArtist));
        assert_eq!(Field::from_key("nope"), None);
    }

    #[test]
    fn an_empty_string_clears_and_an_absent_field_is_untouched() {
        let patch = Patch::new().set(Field::Artist, "  ").set(Field::Title, "T");
        assert_eq!(patch.get(Field::Artist), Some(None));
        assert_eq!(patch.get(Field::Title), Some(Some("T")));
        assert_eq!(patch.get(Field::Album), None);
    }

    #[test]
    fn numeric_fields_are_validated_before_a_file_is_touched() {
        assert!(Patch::new().set(Field::TrackNo, "12").validate().is_ok());
        assert!(Patch::new().set(Field::TrackNo, "abc").validate().is_err());
        assert!(Patch::new().clear(Field::Year).validate().is_ok());
        assert!(Patch::new().set(Field::Title, "text").validate().is_ok());
    }

    #[test]
    fn a_patch_describes_itself_for_the_log() {
        let patch = Patch::new()
            .set(Field::Artist, "Blur")
            .clear(Field::Comment);
        assert_eq!(patch.describe(), "artist=Blur, comment=\u{2205}");
    }
}
