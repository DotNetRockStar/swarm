//! Embedded tag reading via lofty (ID3v2/Vorbis/MP4 atoms) — the piece
//! Batocera.Drone never had. Tags are a display overlay: they refine titles
//! and artist/album names but never change grouping keys.

use lofty::file::TaggedFileExt;
use lofty::tag::{Accessor, ItemKey};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmbeddedTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub track_number: Option<u32>,
}

/// Best-effort: any parse failure (non-audio bytes, unknown format, corrupt
/// tags) yields None and the caller keeps path-derived values.
pub fn read_tags(path: &Path) -> Option<EmbeddedTags> {
    let tagged = lofty::read_from_path(path).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let non_empty = |value: Option<String>| value.filter(|v| !v.trim().is_empty());
    let tags = EmbeddedTags {
        title: non_empty(tag.title().map(|c| c.into_owned())),
        artist: non_empty(tag.artist().map(|c| c.into_owned())),
        album: non_empty(tag.album().map(|c| c.into_owned())),
        track_number: tag.track().or_else(|| {
            tag.get_string(&ItemKey::TrackNumber).and_then(|raw| raw.split('/').next()?.trim().parse().ok())
        }),
    };
    (tags != EmbeddedTags::default()).then_some(tags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garbage_bytes_are_not_an_error() {
        let dir = std::env::temp_dir().join(format!("swarm-tags-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fake.flac");
        std::fs::write(&path, b"not really a flac file").unwrap();
        assert_eq!(read_tags(&path), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
