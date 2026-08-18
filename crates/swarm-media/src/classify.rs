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
    pub year: Option<u32>,
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
            year: None,
        });
    }

    // Bracketed release-group/resolution/codec tags (`[1080p]`, `(x264)`,
    // `{YIFY}`) are decorative and stripped from the title outright; a bare
    // 4-digit year inside one is the sole exception — meaningful signal for
    // TMDb search, kept even though its brackets are still removed. The
    // dominant real-world scene-release convention has no brackets at all
    // though (`10.Cloverfield.Lane.2016.1080p.BluRay.x264-GROUP.mkv`) — a
    // standalone dot/underscore-delimited year token is just as meaningful
    // a signal and just as wrong left sitting in the middle of a title, so
    // it's captured and stripped the same way once no bracket year was
    // found (bracket wins on the rare filename that somehow has both — a
    // deliberately bracketed year is a more deliberate signal). Movies
    // often only carry the year on the enclosing folder, not the filename
    // (`Inception (2010)/Inception.1080p.mkv`), so fall back there too.
    let (stem_clean, mut year) = extract_year_and_strip(stem);
    if year.is_none() {
        year = dirs.last().and_then(|dir| extract_year_and_strip(dir).1);
    }

    if let Some((season, episode, title_prefix)) = parse_episode_marker(&stem_clean) {
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
            title: clean_title(&stem_clean),
            artist: None,
            album: None,
            track_number: None,
            show_title: (!show_title.is_empty()).then_some(show_title),
            season: Some(season),
            episode: Some(episode),
            year,
        });
    }

    Some(Classified {
        kind: MediaKind::Movie,
        title: clean_title(&stem_clean),
        artist: None,
        album: None,
        track_number: None,
        show_title: None,
        season: None,
        episode: None,
        year,
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

/// Remove every top-level `[...]`, `(...)`, `{...}` span from `text`
/// (mismatched/unterminated brackets are left as literal text, and a nested
/// span is swallowed whole by its enclosing one — non-nested filenames are
/// the only case that matters in practice), returning the stripped text and
/// the first bare 4-digit `1900..=2099` year found inside any span.
fn extract_bracket_tags(text: &str) -> (String, Option<u32>) {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut year = None;
    let mut i = 0;
    while i < chars.len() {
        let opener = chars[i];
        let closer = match opener {
            '[' => Some(']'),
            '(' => Some(')'),
            '{' => Some('}'),
            _ => None,
        };
        if let Some(closer) = closer {
            if let Some(offset) = chars[i + 1..].iter().position(|&c| c == closer) {
                let end = i + 1 + offset;
                if year.is_none() {
                    let inner: String = chars[i + 1..end].iter().collect();
                    year = parse_bare_year(&inner);
                }
                i = end + 1;
                continue;
            }
        }
        out.push(opener);
        i += 1;
    }
    (out, year)
}

fn parse_bare_year(inner: &str) -> Option<u32> {
    let trimmed = inner.trim();
    if trimmed.len() == 4 && trimmed.bytes().all(|b| b.is_ascii_digit()) {
        let year: u32 = trimmed.parse().ok()?;
        (1900..=2099).contains(&year).then_some(year)
    } else {
        None
    }
}

/// Try a bracketed year first (the more deliberate signal), then a
/// standalone unbracketed year token — see [extract_bracket_tags] and
/// [extract_bare_year_token] respectively.
fn extract_year_and_strip(text: &str) -> (String, Option<u32>) {
    let (stripped, year) = extract_bracket_tags(text);
    if year.is_some() {
        return (stripped, year);
    }
    extract_bare_year_token(&stripped)
}

/// Find and remove a standalone `1900..=2099` year token from `text`, where
/// tokens are separated by `.`/`_` (the same separators [clean_title]
/// collapses to spaces) or sit at the string's start/end — the dominant
/// real-world scene-release convention for an unbracketed year
/// (`10.Cloverfield.Lane.2016.1080p...`). Only a digit run bounded by a
/// separator or the string's start/end counts, so a year embedded in a
/// longer digit run (a resolution/bitrate number) is never mistaken for
/// one. The matched token is removed but surrounding separators are left in
/// place; [clean_title]'s run-collapsing already turns the resulting
/// doubled separator into a single space.
fn extract_bare_year_token(text: &str) -> (String, Option<u32>) {
    let bytes = text.as_bytes();
    let is_sep = |b: u8| b == b'.' || b == b'_';
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            let mut end = i;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            let at_start = start == 0 || is_sep(bytes[start - 1]);
            let at_end = end == bytes.len() || is_sep(bytes[end]);
            if end - start == 4 && at_start && at_end {
                if let Some(year) = parse_bare_year(&text[start..end]) {
                    let mut out = String::with_capacity(text.len() - 4);
                    out.push_str(&text[..start]);
                    out.push_str(&text[end..]);
                    return (out, Some(year));
                }
            }
            i = end;
        } else {
            i += 1;
        }
    }
    (text.to_string(), None)
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
        // "2010" is a standalone dot-delimited token in the filename, so
        // extract_bare_year_token now captures and strips it — same
        // treatment a bracketed year already got. See year_captured_and_
        // stripped_from_bare_unbracketed_filename_token below for the
        // dedicated year-focused assertions on this exact pattern.
        let entry = classify("movies/Inception (2010)/Inception.2010.1080p.mkv").unwrap();
        assert_eq!(entry.kind, MediaKind::Movie);
        assert_eq!(entry.year, Some(2010));
        assert_eq!(entry.title, "Inception 1080p");
    }

    #[test]
    fn bracket_year_extracted_and_stripped_from_filename() {
        let entry = classify("movies/Interstellar (2014) [1080p].mkv").unwrap();
        assert_eq!(entry.kind, MediaKind::Movie);
        assert_eq!(entry.year, Some(2014));
        assert!(!entry.title.contains('('), "brackets must not survive into the title: {}", entry.title);
        assert!(!entry.title.contains('['), "brackets must not survive into the title: {}", entry.title);
    }

    #[test]
    fn bracket_year_falls_back_to_the_enclosing_folder() {
        // No year anywhere in the filename (bracketed or bare) — this is
        // the case that genuinely needs the folder fallback, a common
        // real-world layout.
        let entry = classify("movies/Inception (2010)/Inception.1080p.mkv").unwrap();
        assert_eq!(entry.year, Some(2010));
        assert_eq!(entry.title, "Inception 1080p", "folder-derived year must not change the filename-derived title");
    }

    #[test]
    fn year_captured_and_stripped_from_bare_unbracketed_filename_token() {
        // The dominant real-world scene-release convention has no brackets
        // at all — found live on real hardware: year stayed NULL for
        // exactly this pattern before this fix.
        let cloverfield = classify("movies/10.Cloverfield.Lane.2016.1080p.BluRay.x264-GROUP.mkv").unwrap();
        assert_eq!(cloverfield.year, Some(2016));
        assert_eq!(cloverfield.title, "10 Cloverfield Lane 1080p BluRay x264-GROUP");

        let days_later = classify("movies/28.Days.Later.2002.1080p.BluRay.x264-GROUP.mkv").unwrap();
        assert_eq!(days_later.year, Some(2002));
        assert_eq!(days_later.title, "28 Days Later 1080p BluRay x264-GROUP");
    }

    #[test]
    fn bare_year_token_must_be_separator_bounded_not_embedded_in_a_longer_run() {
        // A 4-digit run that's part of a longer digit sequence (a bitrate/
        // resolution-adjacent number) must never be mistaken for a year.
        let entry = classify("movies/Movie.19004.mkv").unwrap();
        assert_eq!(entry.year, None);
        assert_eq!(entry.title, "Movie 19004");
    }

    #[test]
    fn bracket_year_takes_precedence_over_a_bare_token_year() {
        let entry = classify("movies/Movie.2016.[2010].mkv").unwrap();
        assert_eq!(entry.year, Some(2010), "a deliberately bracketed year is the more deliberate signal");
    }

    #[test]
    fn decorative_bracket_tags_are_discarded_without_setting_a_year() {
        let entry = classify("movies/Heat [x264] (YIFY) {web-dl}.mkv").unwrap();
        assert_eq!(entry.year, None);
        assert_eq!(entry.title, "Heat");
    }

    #[test]
    fn bracket_content_without_a_year_is_still_stripped() {
        let entry = classify("tv/Severance/Season 1/Severance.S01E01 [Good News About Hell].mkv").unwrap();
        assert_eq!(entry.kind, MediaKind::Episode);
        assert_eq!(entry.year, None);
        assert!(!entry.title.contains('['));
    }

    #[test]
    fn sidecars_are_rejected() {
        assert!(classify("movies/Inception (2010)/poster.jpg").is_none());
        assert!(classify("movies/Inception (2010)/Inception.nfo").is_none());
        assert!(classify("movies/readme.txt").is_none());
    }
}
