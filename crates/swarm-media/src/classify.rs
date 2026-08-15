//! File classification and path-derived grouping.
//!
//! Rules inherited from Batocera.Drone (documented there as scar tissue from
//! shipped bugs):
//! - **Allowlist, not denylist** — only known media extensions become catalog
//!   entries, so sidecar files (.nfo, posters, subtitles) never leak in.
//! - **Grouping keys are always path/filename-derived** — embedded tags and
//!   scraped titles are display overlay only, so a bad tag or scrape can
//!   never split or corrupt an album/show grouping.

use swarm_core::peer::MediaKind;

pub const AUDIO_EXTS: &[&str] = &["mp3", "flac", "ogg", "opus", "m4a", "wav", "wma", "aac", "aiff", "ape"];
pub const VIDEO_EXTS: &[&str] =
    &["mp4", "mkv", "avi", "mov", "webm", "m4v", "wmv", "flv", "mpg", "mpeg", "m2ts", "ts", "3gp"];

/// Disc-subfolder names absorbed into the parent album (e.g. `CD1`, `Disc 2`).
fn is_disc_folder(name: &str) -> bool {
    let lower = name.to_lowercase();
    for prefix in ["cd", "disc", "disk"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let rest = rest.trim_start_matches([' ', '-', '_']);
            if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
                return true;
            }
        }
    }
    false
}

#[derive(Debug, Clone, PartialEq)]
pub struct Classified {
    pub kind: MediaKind,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub track_number: Option<u32>,
    pub show_title: Option<String>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
}

pub fn media_extension(relative_path: &str) -> Option<(&'static str, bool)> {
    let ext = relative_path.rsplit('.').next()?.to_lowercase();
    if let Some(known) = AUDIO_EXTS.iter().find(|e| **e == ext) {
        return Some((known, true));
    }
    VIDEO_EXTS.iter().find(|e| **e == ext).map(|known| (*known, false))
}

/// Classify a library-relative path (forward slashes) into a catalog entry.
/// Returns None for non-media extensions.
pub fn classify(relative_path: &str) -> Option<Classified> {
    let (_, is_audio) = media_extension(relative_path)?;
    let segments: Vec<&str> = relative_path.split('/').filter(|s| !s.is_empty()).collect();
    let file_name = segments.last()?;
    let stem = file_name.rsplit_once('.').map(|(s, _)| s).unwrap_or(file_name);
    // Directory chain above the file, with disc folders absorbed.
    let mut dirs: Vec<&str> = segments[..segments.len() - 1].to_vec();
    if dirs.last().is_some_and(|d| is_disc_folder(d)) {
        dirs.pop();
    }

    if is_audio {
        let (track_number, title) = split_track_number(stem);
        // Folder convention: .../Artist/Album/track — album is the nearest
        // directory, artist the one above it.
        let album = dirs.last().map(|s| clean_title(s));
        let artist = dirs.len().checked_sub(2).and_then(|i| dirs.get(i)).map(|s| clean_title(s));
        return Some(Classified {
            kind: MediaKind::Track,
            title,
            artist,
            album,
            track_number,
            show_title: None,
            season: None,
            episode: None,
        });
    }

    if let Some((season, episode, title_prefix)) = parse_episode_marker(stem) {
        // Show title: prefer the text before the SxxEyy marker, else the
        // grandparent/parent directory (Show/Season 1/file layout).
        let from_stem = clean_title(title_prefix);
        let show_title = if from_stem.is_empty() {
            dirs.iter()
                .rev()
                .map(|d| clean_title(d))
                .find(|name| !name.is_empty() && !is_season_folder(name))
                .unwrap_or_default()
        } else {
            from_stem
        };
        return Some(Classified {
            kind: MediaKind::Episode,
            title: clean_title(stem),
            artist: None,
            album: None,
            track_number: None,
            show_title: (!show_title.is_empty()).then_some(show_title),
            season: Some(season),
            episode: Some(episode),
        });
    }

    Some(Classified {
        kind: MediaKind::Movie,
        title: clean_title(stem),
        artist: None,
        album: None,
        track_number: None,
        show_title: None,
        season: None,
        episode: None,
    })
}

fn is_season_folder(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.strip_prefix("season").map(|rest| rest.trim().bytes().all(|b| b.is_ascii_digit())).unwrap_or(false)
}

/// Find an `SxxEyy` marker (case-insensitive); returns (season, episode, text
/// before the marker).
fn parse_episode_marker(stem: &str) -> Option<(u32, u32, &str)> {
    let bytes = stem.as_bytes();
    for start in 0..bytes.len() {
        if !bytes[start].eq_ignore_ascii_case(&b's') {
            continue;
        }
        let mut i = start + 1;
        let season_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == season_start || i - season_start > 3 || i >= bytes.len() || !bytes[i].eq_ignore_ascii_case(&b'e') {
            continue;
        }
        let episode_start = i + 1;
        let mut j = episode_start;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j == episode_start || j - episode_start > 4 {
            continue;
        }
        let season = stem[season_start..i].parse().ok()?;
        let episode = stem[episode_start..j].parse().ok()?;
        return Some((season, episode, &stem[..start]));
    }
    None
}

/// Leading track number: `01 - Title`, `01. Title`, `01_Title`, `01 Title`.
fn split_track_number(stem: &str) -> (Option<u32>, String) {
    let digits: String = stem.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits.len() > 3 {
        return (None, clean_title(stem));
    }
    let rest = &stem[digits.len()..];
    let trimmed = rest.trim_start_matches([' ', '-', '.', '_']);
    if trimmed.is_empty() || trimmed.len() == rest.len() {
        // No separator after the digits ("1984.flac" stays a title).
        return (None, clean_title(stem));
    }
    (digits.parse().ok(), clean_title(trimmed))
}

/// Normalize separators for display: dots/underscores to spaces, collapse runs.
fn clean_title(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_space = true;
    for ch in raw.chars() {
        let mapped = if ch == '.' || ch == '_' { ' ' } else { ch };
        if mapped == ' ' {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.push(mapped);
            last_space = false;
        }
    }
    out.trim().trim_end_matches(['-', ' ']).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_track_with_artist_album_and_number() {
        let entry = classify("music/Pink Floyd/The Wall/05 - Hey You.flac").unwrap();
        assert_eq!(entry.kind, MediaKind::Track);
        assert_eq!(entry.title, "Hey You");
        assert_eq!(entry.artist.as_deref(), Some("Pink Floyd"));
        assert_eq!(entry.album.as_deref(), Some("The Wall"));
        assert_eq!(entry.track_number, Some(5));
    }

    #[test]
    fn disc_folder_is_absorbed() {
        let entry = classify("music/Artist/Album/CD2/03. Song.mp3").unwrap();
        assert_eq!(entry.album.as_deref(), Some("Album"));
        assert_eq!(entry.artist.as_deref(), Some("Artist"));
        assert_eq!(entry.track_number, Some(3));
    }

    #[test]
    fn numeric_title_without_separator_is_not_a_track_number() {
        let entry = classify("music/Artist/Album/1984.flac").unwrap();
        assert_eq!(entry.track_number, None);
        assert_eq!(entry.title, "1984");
    }

    #[test]
    fn episode_marker_in_filename() {
        let entry = classify("tv/The Expanse/Season 2/The.Expanse.S02E05.Home.mkv").unwrap();
        assert_eq!(entry.kind, MediaKind::Episode);
        assert_eq!(entry.season, Some(2));
        assert_eq!(entry.episode, Some(5));
        assert_eq!(entry.show_title.as_deref(), Some("The Expanse"));
    }

    #[test]
    fn episode_show_title_falls_back_to_directory() {
        let entry = classify("tv/Severance/Season 1/s01e03.mkv").unwrap();
        assert_eq!(entry.show_title.as_deref(), Some("Severance"));
        assert_eq!(entry.season, Some(1));
        assert_eq!(entry.episode, Some(3));
    }

    #[test]
    fn movie_title_cleanup() {
        let entry = classify("movies/Inception (2010)/Inception.2010.1080p.mkv").unwrap();
        assert_eq!(entry.kind, MediaKind::Movie);
        assert_eq!(entry.title, "Inception 2010 1080p");
    }

    #[test]
    fn sidecars_are_rejected() {
        assert!(classify("movies/Inception (2010)/poster.jpg").is_none());
        assert!(classify("movies/Inception (2010)/Inception.nfo").is_none());
        assert!(classify("movies/readme.txt").is_none());
    }
}
