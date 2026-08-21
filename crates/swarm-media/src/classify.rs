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

/// A release-type "category" folder some libraries insert between Artist and
/// the real album folder — `Artist/Album/<Real Album Name>/track.mp3`,
/// `Artist/Compilation/<Real Release Name>/track.mp3`. Ported from
/// batocera.drone's `music/filename_parser.py::_CATEGORY_FOLDER_NAMES`
/// (`drone-music-feature` skill), confirmed there against a real ~2,400-track
/// library where every release of one of these types collapsed into a single
/// fake bucket (e.g. every ATB album under one "ATB / Album" group) — same
/// vocabulary, since it mirrors MusicBrainz's own release-group type list.
const CATEGORY_FOLDER_NAMES: &[&str] = &[
    "album", "albums", "single", "singles", "ep", "eps", "broadcast", "broadcasts", "other", "others",
    "compilation", "compilations", "soundtrack", "soundtracks", "spokenword", "interview", "interviews",
    "audiobook", "audiobooks", "audio drama", "live", "live album", "live albums", "remix", "remixes",
    "dj-mix", "dj mix", "mixtape", "mixtapes", "street", "demo", "demos", "field recording",
    "field recordings", "bootleg", "bootlegs", "bonus", "bonuses",
];

/// Real libraries often number these wrapper folders (`"3. Remixes"`,
/// `"4. Bonus"`) rather than using the bare category name — confirmed
/// against a real library where an artist's remix/bonus folders were laid
/// out exactly this way. Strips a leading `N`/`N.`/`N ` ordinal before the
/// category-name check so those still match.
fn strip_leading_ordinal(name: &str) -> &str {
    let trimmed = name.trim_start();
    let digits_end = trimmed.find(|c: char| !c.is_ascii_digit()).unwrap_or(trimmed.len());
    if digits_end == 0 {
        return name;
    }
    let rest = trimmed[digits_end..].trim_start_matches(['.', ' ']).trim();
    if rest.is_empty() {
        name
    } else {
        rest
    }
}

fn is_category_folder(name: &str) -> bool {
    CATEGORY_FOLDER_NAMES.contains(&strip_leading_ordinal(name).to_lowercase().as_str())
}

/// Strips a trailing `" - Discography"`/`" Discography"` suffix (case-
/// insensitive) from an artist folder name. Real, live example: a folder
/// named `"Kyau & Albert - Discography"` (containing every one of that
/// artist's releases as subfolders) was being treated as if "Discography"
/// were literally part of the artist's name — both the grouping display and
/// every MusicBrainz search built from it (`artist:"Kyau & Albert -
/// Discography"`) were wrong as a result. Confirmed live against a real
/// ~5,300-track library: two artist folders (`Kyau & Albert - Discography`,
/// `Staind - Discography`) used this exact convention, together 666 tracks
/// (~12% of the library).
fn strip_discography_suffix(name: &str) -> &str {
    for suffix in [" - discography", " discography"] {
        if name.len() > suffix.len() && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix) {
            let stripped = name[..name.len() - suffix.len()].trim_end();
            if !stripped.is_empty() {
                return stripped;
            }
        }
    }
    name
}

/// A generic top-level "this is where the music lives" folder name, only
/// ever meaningful as the very first path segment (unlike
/// [`CATEGORY_FOLDER_NAMES`], which applies one level into an artist).
/// Deliberately a short, narrow list — the same accepted trade-off as any
/// name-based folder convention (a real artist literally named "Music"
/// would be misread), kept small so it only catches the genuinely generic,
/// unambiguous wrapper names real libraries actually use.
const MEDIA_TYPE_WRAPPER_NAMES: &[&str] = &["music", "songs", "audio", "tracks"];

fn is_media_type_wrapper(name: &str) -> bool {
    MEDIA_TYPE_WRAPPER_NAMES.contains(&name.to_lowercase().as_str())
}

/// The video equivalent of [`MEDIA_TYPE_WRAPPER_NAMES`] — a top-level "this
/// is where the shows live" folder (Sonarr/Plex/Kodi's own convention: a
/// `TV Shows/<Show Name>/...` root). Confirmed live against a real library
/// (`Batocera-movies-shows/Shows/<Show Name>/...`) where content nested
/// under a show folder with neither a `SxxEyy`/`Ep. NN` marker nor a
/// `Season N` subfolder (deeply nested featurettes/deleted-scenes/bonus
/// content) fell all the way through to the movie fallback below and was
/// searched against the wrong TMDb database entirely. Only meaningful as
/// the very first path segment, same reasoning as the music wrapper.
const VIDEO_TYPE_WRAPPER_NAMES: &[&str] = &["shows", "show", "tv", "tv shows", "tv series", "series", "television"];

fn is_video_type_wrapper(name: &str) -> bool {
    VIDEO_TYPE_WRAPPER_NAMES.contains(&name.to_lowercase().as_str())
}

/// The show folder immediately below a recognized Shows/TV wrapper folder
/// somewhere in `dirs` ([VIDEO_TYPE_WRAPPER_NAMES]), if any. Scans the whole
/// ancestor chain rather than anchoring to index 0, same robustness as
/// [find_ancestor_season], since a real path may carry an extra leading
/// multi-root label segment ahead of the wrapper. Shared by every
/// show_title fallback chain that needs "the real show folder" rather than
/// [show_title_from_ancestors]'s naive nearest-non-season-folder walk,
/// which can land on a generic bonus-content wrapper folder name
/// (`"Featurettes"`, `"Extras"`) instead of the actual show — confirmed
/// live: a file with its own `S00E02`-style marker sitting directly inside
/// a `Featurettes` folder (no season-shaped ancestor, no stem-prefix text)
/// picked up show_title `"Featurettes"` before this existed.
fn wrapper_derived_show_name(dirs: &[&str]) -> Option<String> {
    let wrapper_idx = dirs.iter().position(|d| is_video_type_wrapper(d))?;
    let show_title = clean_title(dirs.get(wrapper_idx + 1)?);
    (!show_title.is_empty()).then_some(show_title)
}

/// Find an `Ep`/`Episode` marker (case-insensitive, optional trailing `.`,
/// optional space, then 1-4 digits) — a common real-world alternate to
/// `SxxEyy` for shows numbered without a season component in the filename
/// (e.g. `"CENTURIONS - Ep. 57 - Hole in the Ocean, Part 2"`). Confirmed
/// live: this exact convention was silently misclassified as a movie
/// (searched against TMDb's movie DB, so "not found" even though the show
/// exists) before this parser existed. Both boundaries must be non-
/// alphanumeric (or string start/end), same bounding discipline as
/// [parse_nxnn_marker], so this never fires inside a longer word like
/// "Deep" or "Prep". Unlike `SxxEyy`, this marker carries no season of its
/// own — the caller resolves season from an ancestor `Season N` folder,
/// defaulting to 1 when none exists. Returns the parsed episode number.
fn parse_ep_marker(stem: &str) -> Option<u32> {
    let bytes = stem.as_bytes();
    let is_boundary_before = |i: usize| i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
    for word in ["episode", "ep"] {
        let wlen = word.len();
        let mut start = 0;
        while start + wlen <= bytes.len() {
            if is_boundary_before(start) && bytes[start..start + wlen].eq_ignore_ascii_case(word.as_bytes()) {
                let mut i = start + wlen;
                if i < bytes.len() && bytes[i] == b'.' {
                    i += 1;
                }
                while i < bytes.len() && bytes[i] == b' ' {
                    i += 1;
                }
                let digit_start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let digit_len = i - digit_start;
                let at_end = i == bytes.len() || !bytes[i].is_ascii_alphanumeric();
                if digit_len > 0 && digit_len <= 4 && at_end {
                    if let Ok(episode) = stem[digit_start..i].parse() {
                        return Some(episode);
                    }
                }
            }
            start += 1;
        }
    }
    None
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
        // Folder convention: .../Artist/Album/track — anchored from the TOP
        // (artist = the first folder under the media root, album = the
        // second), not the bottom. Ported from batocera.drone's
        // `music/filename_parser.py::classify_location` (`drone-music-
        // feature` skill) after confirming live that anchoring from the
        // bottom (previously: album = dirs.last(), artist = the folder
        // above it) silently produced garbage for any real library nested
        // deeper than exactly two levels — DJ-mix/radio-broadcast-style
        // folder structures like `Gabriel & Dresden/Organized Natures/
        // 01-29/29/track.mp3` classified as artist="01-29", album="29"
        // instead of the correct artist="Gabriel & Dresden", album=
        // "Organized Natures". Anchoring from the top and simply ignoring
        // any deeper segments fixes this without needing to special-case
        // "how deep is too deep" — confirmed against a real library where
        // 82% of tracks (4,371/5,340) were nested past two levels.
        //
        // A generic media-type wrapper folder (a single combined root with
        // Movies/Shows/Music-style top-level subfolders, rather than a
        // dedicated per-type root or label) is skipped the same way a
        // multi-root label prefix already is upstream (see scan_roots/
        // reclassify_all) — otherwise it would be misread as the artist.
        let dirs: &[&str] = if dirs.first().is_some_and(|d| is_media_type_wrapper(d)) { &dirs[1..] } else { &dirs };

        // A category/release-type wrapper folder some libraries insert
        // right after Artist (`Artist/Album/<Real Album Name>/track.mp3`,
        // `Artist/Compilation/<Real Release>/track.mp3`) is skipped so the
        // real album name underneath it is used instead of the category
        // label — see CATEGORY_FOLDER_NAMES.
        let artist = dirs.first().map(|s| clean_title(strip_discography_suffix(s)));
        let album = match dirs.get(1) {
            Some(second) if is_category_folder(second) && dirs.len() >= 3 => {
                Some(clean_title(dirs[2]))
            }
            Some(second) => Some(clean_title(second)),
            None => None,
        };
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
        // Show title: prefer an ancestor season folder (either shape — see
        // find_ancestor_season) over the text before the SxxEyy/NxNN
        // marker, else fall back to the marker's own stem-prefix text, else
        // the older plain-directory fallback (Show/Season 1/file).
        //
        // This used to prefer the stem-prefix text first — reversed after a
        // real, live example proved that wrong: a folder ("Law & Order
        // SVU") containing many seasons' worth of episodes sourced from
        // different release groups, where most files agree on one exact
        // filename wording but a handful vary ("Law and Order SVU",
        // "Law And Order SVU", "Law and Order Special Victims Unit" —
        // confirmed live, 7 real files split into 3 splinter groups this
        // way out of 580). A season folder the user actually organized
        // files into is a much more stable, deliberate identity signal
        // than whatever text a random uploader happened to put in a
        // filename — release-group filename wording varies far more than
        // folder structure does in practice.
        let from_stem = clean_title(title_prefix);
        let folder_derived = find_ancestor_season(&dirs).map(|(name, _, _)| name).filter(|name| !name.is_empty());
        let show_title = folder_derived
            .or_else(|| (!from_stem.is_empty()).then_some(from_stem))
            .or_else(|| wrapper_derived_show_name(&dirs))
            .unwrap_or_else(|| show_title_from_ancestors(&dirs));
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

    // `Ep. NN`/`Episode NN` marker with no season encoded in the filename
    // itself (unlike SxxEyy/NxNN) — season comes from an ancestor
    // `Season N` folder when one exists, else defaults to 1 (the common
    // convention for a continuously-numbered single-season show). The show
    // name is deliberately always folder-derived here, never parsed from
    // the text before the marker — real "Ep. NN" filenames are far less
    // consistently formatted than SxxEyy ones (sometimes an abbreviation,
    // sometimes omitted entirely), so the folder ancestor is the more
    // reliable, path-derived signal (see the module doc comment's grouping
    // rule).
    if let Some(episode) = parse_ep_marker(&stem_clean) {
        let (season, folder_year) = find_ancestor_season(&dirs).map(|(_, s, y)| (s, y)).unwrap_or((1, None));
        let show_title = find_ancestor_season(&dirs)
            .map(|(name, _, _)| name)
            .or_else(|| wrapper_derived_show_name(&dirs))
            .unwrap_or_else(|| show_title_from_ancestors(&dirs));
        if !show_title.is_empty() {
            return Some(Classified {
                kind: MediaKind::Episode,
                title: clean_title(&stem_clean),
                artist: None,
                album: None,
                track_number: None,
                show_title: Some(show_title),
                season: Some(season),
                episode: Some(episode),
                year: year.or(folder_year),
            });
        }
    }

    // No SxxEyy anywhere in the filename itself, but the file sits somewhere
    // under a real season folder — bonus/extra content (a featurette,
    // interview, deleted scene, blooper reel, behind-the-scenes clip...).
    // The specific containing subfolder name isn't matched against a list of
    // known synonyms (too fragile — it varies by uploader); the structural
    // signal alone (nested under a season folder, no episode marker of its
    // own) is what matters. `season: Some(0)` is the real-world Plex/Kodi/
    // TheTVDB convention for "Specials" — deliberately a single show-level
    // bucket rather than per-season, since bonus content isn't numbered
    // against any one season the way real episodes are.
    if let Some((show_title, _season, folder_year)) = find_ancestor_season(&dirs) {
        return Some(Classified {
            kind: MediaKind::Episode,
            title: clean_title(&stem_clean),
            artist: None,
            album: None,
            track_number: None,
            show_title: Some(show_title),
            season: Some(0),
            episode: None,
            year: year.or(folder_year),
        });
    }

    // No episode marker anywhere, no ancestor `Season N` folder — but the
    // file is nested under a recognized Shows/TV wrapper folder somewhere
    // in its ancestor chain (see [VIDEO_TYPE_WRAPPER_NAMES]). This is
    // deeply-nested bonus content (featurettes/deleted-scenes/fake-endings
    // — arbitrarily nested, no fixed convention worth enumerating, same
    // reasoning as the season-folder bonus-content case above) sitting
    // directly under a show folder rather than a season folder. Confirmed
    // live: without this, such files fell all the way through to the movie
    // fallback below and were searched against the wrong TMDb database, and
    // never appeared under their show. The show folder is the segment
    // immediately below the wrapper. Scans the whole ancestor chain rather
    // than anchoring to index 0, same robustness as [find_ancestor_season],
    // since a real path may carry an extra leading multi-root label segment
    // ahead of the wrapper.
    if let Some(show_title) = wrapper_derived_show_name(&dirs) {
        return Some(Classified {
            kind: MediaKind::Episode,
            title: clean_title(&stem_clean),
            artist: None,
            album: None,
            track_number: None,
            show_title: Some(show_title),
            season: Some(0),
            episode: None,
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

/// The pre-existing show-name fallback (kept as its own function since it's
/// now used from two places): nearest-to-furthest, the first ancestor
/// directory that isn't itself a season folder.
fn show_title_from_ancestors(dirs: &[&str]) -> String {
    dirs.iter().rev().map(|d| clean_title(d)).find(|name| !name.is_empty() && !is_season_folder(name)).unwrap_or_default()
}

/// A season-indicating folder, any of three shapes: a literal `"Season N"`,
/// `parse_season_suffix_folder`'s `"<name> SNN"`, or a bare `"SNN"`
/// ([parse_bare_season_folder]) — see those functions. `is_season_folder`
/// treats all three as "skip this while hunting for a plain show-name
/// ancestor".
fn is_season_folder(name: &str) -> bool {
    let lower = name.to_lowercase();
    let literal = lower.strip_prefix("season").map(|rest| rest.trim().bytes().all(|b| b.is_ascii_digit())).unwrap_or(false);
    literal || parse_season_suffix_folder(name).is_some() || parse_bare_season_folder(name).is_some()
}

/// A folder name ending in `" SNN"` (a space, a case-insensitive `S`, then
/// 1-3 digits, at the very end) — e.g. `"Dexter (2006) S03"`,
/// `"Lost (2004) S03"`. Returns `(season, text before the suffix)` on
/// match. Deliberately conservative (the space before `S` is required, and
/// the text before it must be non-empty) — a false positive here would
/// misclassify a real movie as show content, a strictly worse failure than
/// the bonus-content-mistaken-for-a-movie bug this exists to fix.
fn parse_season_suffix_folder(name: &str) -> Option<(u32, &str)> {
    let bytes = name.as_bytes();
    let end = bytes.len();
    let mut digit_start = end;
    while digit_start > 0 && bytes[digit_start - 1].is_ascii_digit() {
        digit_start -= 1;
    }
    if digit_start == end || end - digit_start > 3 {
        return None;
    }
    if digit_start == 0 || !bytes[digit_start - 1].eq_ignore_ascii_case(&b's') {
        return None;
    }
    let s_pos = digit_start - 1;
    if s_pos == 0 || bytes[s_pos - 1] != b' ' {
        return None;
    }
    let season: u32 = name[digit_start..end].parse().ok()?;
    let show_part = name[..s_pos].trim_end();
    (!show_part.is_empty()).then_some((season, show_part))
}

/// A folder whose entire name is just a case-insensitive `S` followed by 1-2
/// digits and nothing else (e.g. `"S06"`, `"S6"`) — an abbreviated
/// alternative to the literal `"Season N"` folder, with an identical
/// relationship to its parent: the show name lives in the *next* real
/// ancestor above it, not in this folder itself (unlike
/// [parse_season_suffix_folder], which requires non-empty show-name text
/// *before* the "S" in the same folder — the two never both match the same
/// name). Deliberately whole-string, not a prefix/suffix match, so a folder
/// like `"S06E01"` (a real convention some rips use to flatten one episode
/// directly under a combined season+episode folder name) can never
/// misfire here — the trailing `"E01"` isn't all-digits, so it fails the
/// all-digit check below.
fn parse_bare_season_folder(name: &str) -> Option<u32> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || !bytes[0].eq_ignore_ascii_case(&b's') {
        return None;
    }
    let digits = &bytes[1..];
    if digits.is_empty() || digits.len() > 2 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    name[1..].parse().ok()
}

/// Walk `dirs` nearest-to-furthest looking for the first ancestor that's a
/// season folder, any of three shapes, and derive `(show_title, season, year)`:
/// - `"<Show Name> (<year>) SNN"` is self-contained — the show name and an
///   optional year both live in the same folder (run through the existing
///   [extract_year_and_strip] + [clean_title] pipeline, same as everywhere
///   else a folder/filename gets turned into a display name).
/// - a literal `"Season N"` or a bare `"SNN"` ([parse_bare_season_folder])
///   has no show name of its own — it comes from the next real ancestor
///   above it (skipping further season/disc folders). Prefers
///   [wrapper_derived_show_name] (the segment right below a recognized
///   Shows/TV wrapper) over [show_title_from_ancestors]'s naive nearest-
///   non-season-folder walk, which — confirmed live — can land on a
///   generic bonus-content wrapper folder instead of the real show when
///   one sits directly above the season folder (e.g. a doubly-nested
///   `.../The Office (US) (2005).../Featurettes/Featurettes/Season 1/...`,
///   where the naive walk stops at the inner `"Featurettes"` instead of
///   climbing two more levels to the actual show folder).
fn find_ancestor_season(dirs: &[&str]) -> Option<(String, u32, Option<u32>)> {
    for (idx, dir) in dirs.iter().enumerate().rev() {
        if let Some((season, show_part)) = parse_season_suffix_folder(dir) {
            let (stripped, year) = extract_year_and_strip(show_part);
            let show_title = clean_title(&stripped);
            if !show_title.is_empty() {
                return Some((show_title, season, year));
            }
            continue;
        }
        let lower = dir.to_lowercase();
        let literal_season = lower.strip_prefix("season").and_then(|rest| rest.trim().parse().ok());
        let Some(season) = literal_season.or_else(|| parse_bare_season_folder(dir)) else {
            continue;
        };
        let show_title = wrapper_derived_show_name(&dirs[..idx]).unwrap_or_else(|| show_title_from_ancestors(&dirs[..idx]));
        if !show_title.is_empty() {
            return Some((show_title, season, None));
        }
    }
    None
}

/// Find an episode marker in `stem`, either shape — `SxxEyy` tried first
/// (unchanged priority/behavior for every filename that already worked),
/// `NxNN` (see [parse_nxnn_marker]) as a fallback when that finds nothing.
/// Both return (season, episode, text before the marker).
fn parse_episode_marker(stem: &str) -> Option<(u32, u32, &str)> {
    parse_sxxeyy_marker(stem).or_else(|| parse_nxnn_marker(stem))
}

/// Find an `SxxEyy` marker (case-insensitive); returns (season, episode, text
/// before the marker).
fn parse_sxxeyy_marker(stem: &str) -> Option<(u32, u32, &str)> {
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

/// Find an `NxNN` marker (e.g. `6x09` = season 6, episode 9) — a common
/// real-world alternate to `SxxEyy` in manual/scene rips. Case-insensitive
/// on the `x`. Both digit runs must be genuinely bounded — string start/end,
/// or a non-alphanumeric byte on either side — so this can never match
/// inside a longer word or a real title that happens to contain a lowercase
/// "x" adjacent to digits (same bounding discipline as
/// [extract_bare_year_token]). Season is capped at 2 digits, episode at 3,
/// matching real-world shows' actual numbering ranges and keeping this from
/// false-positiving on an unrelated longer digit run.
fn parse_nxnn_marker(stem: &str) -> Option<(u32, u32, &str)> {
    let bytes = stem.as_bytes();
    let is_boundary_before = |i: usize| i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
    let is_boundary_at = |i: usize| i == bytes.len() || !bytes[i].is_ascii_alphanumeric();
    for x_pos in 0..bytes.len() {
        if !bytes[x_pos].eq_ignore_ascii_case(&b'x') {
            continue;
        }
        let mut season_start = x_pos;
        while season_start > 0 && bytes[season_start - 1].is_ascii_digit() {
            season_start -= 1;
        }
        let season_len = x_pos - season_start;
        if season_len == 0 || season_len > 2 || !is_boundary_before(season_start) {
            continue;
        }
        let episode_start = x_pos + 1;
        let mut episode_end = episode_start;
        while episode_end < bytes.len() && bytes[episode_end].is_ascii_digit() {
            episode_end += 1;
        }
        let episode_len = episode_end - episode_start;
        if episode_len == 0 || episode_len > 3 || !is_boundary_at(episode_end) {
            continue;
        }
        let season = stem[season_start..x_pos].parse().ok()?;
        let episode = stem[episode_start..episode_end].parse().ok()?;
        return Some((season, episode, &stem[..season_start]));
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
/// tokens are separated by `.`/`_`/` ` (the same separators [clean_title]
/// collapses to spaces) or sit at the string's start/end — covers both the
/// dot-separated scene-release convention (`10.Cloverfield.Lane.2016.
/// 1080p...`) and the equally common plain-space convention (`Shaun of the
/// Dead 2004 (1080p...)`). Only a digit run bounded by a separator or the
/// string's start/end counts, so a year embedded in a longer digit run (a
/// resolution/bitrate number) is never mistaken for one.
///
/// Real bug this fixes: space wasn't originally a recognized separator at
/// all, so a plain-space filename's year silently stayed baked into the
/// title text instead of being captured — confirmed live: "Shaun of the
/// Dead 2004 (1080p x265 q22 FS78 Joy).mkv" searched TMDb for the literal
/// title "Shaun of the Dead 2004" (year field left `None`) instead of title
/// "Shaun of the Dead" + year 2004, and came back unmatched.
///
/// Scans the *whole* string, collecting every valid match, and treats the
/// **last** one's value as authoritative rather than the first —
/// deliberately, so a movie whose own title is a bare number that happens
/// to fall in 1900..=2099 (a real, released film literally titled "1917",
/// or a hypothetical "2012 2009 1080p.mkv") has its real trailing year
/// preferred over misreading the title itself as the year; scene-release/
/// plain-filename convention overwhelmingly places the year as the last
/// semantic token before quality/codec noise, never the first word of the
/// title. Same accepted-heuristic trade-off as every other name-based
/// convention in this module (e.g. [MEDIA_TYPE_WRAPPER_NAMES]) — not
/// airtight against every possible title, but strictly better than the
/// alternative of never capturing a bare year at all.
///
/// Every match sharing the authoritative value is stripped, not just the
/// last one — real bug, found live: `"Interstellar.2014.2014.1080p..."`
/// (the year genuinely repeated twice back to back) only had its second
/// occurrence removed, leaving `"Interstellar 2014"` as the actual search
/// title. A repeated *identical* year is always redundant duplication, safe
/// to collapse entirely; a match with a *different* value (the "1917"
/// case above) is left untouched, since that's real, deliberate title text.
fn extract_bare_year_token(text: &str) -> (String, Option<u32>) {
    let bytes = text.as_bytes();
    let is_sep = |b: u8| b == b'.' || b == b'_' || b == b' ';
    let mut matches: Vec<(usize, usize, u32)> = Vec::new();
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
                    matches.push((start, end, year));
                }
            }
            i = end;
        } else {
            i += 1;
        }
    }
    if let Some(&(_, _, authoritative_year)) = matches.last() {
        let mut out = String::with_capacity(text.len());
        let mut last_end = 0;
        for &(start, end, year) in &matches {
            if year == authoritative_year {
                out.push_str(&text[last_end..start]);
                last_end = end;
            }
        }
        out.push_str(&text[last_end..]);
        return (out, Some(authoritative_year));
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
        let entry = classify("Pink Floyd/The Wall/05 - Hey You.flac").unwrap();
        assert_eq!(entry.kind, MediaKind::Track);
        assert_eq!(entry.title, "Hey You");
        assert_eq!(entry.artist.as_deref(), Some("Pink Floyd"));
        assert_eq!(entry.album.as_deref(), Some("The Wall"));
        assert_eq!(entry.track_number, Some(5));
    }

    #[test]
    fn disc_folder_is_absorbed() {
        let entry = classify("Artist/Album/CD2/03. Song.mp3").unwrap();
        assert_eq!(entry.album.as_deref(), Some("Album"));
        assert_eq!(entry.artist.as_deref(), Some("Artist"));
        assert_eq!(entry.track_number, Some(3));
    }

    #[test]
    fn numeric_title_without_separator_is_not_a_track_number() {
        let entry = classify("Artist/Album/1984.flac").unwrap();
        assert_eq!(entry.track_number, None);
        assert_eq!(entry.title, "1984");
    }

    /// Confirmed live against a real library: a DJ-mix/radio-broadcast-style
    /// folder structure nested past the expected two levels used to have its
    /// artist/album read from the *bottom* of the path (whatever was
    /// nearest the file), producing garbage like artist="01-29", album="29".
    /// Anchoring from the top and ignoring anything deeper fixes it.
    #[test]
    fn audio_track_nested_deeper_than_two_levels_still_groups_by_the_top_two_folders() {
        let entry = classify("Gabriel & Dresden/Organized Natures/01-29/29/track.mp3").unwrap();
        assert_eq!(entry.artist.as_deref(), Some("Gabriel & Dresden"));
        assert_eq!(entry.album.as_deref(), Some("Organized Natures"));
    }

    #[test]
    fn artist_discography_wrapper_suffix_is_stripped() {
        // Real bug, found live: "Kyau & Albert - Discography" and "Staind
        // - Discography" (666 real tracks combined) were treated as
        // literal artist names, breaking both display and every
        // MusicBrainz search built from them.
        let entry = classify("Kyau & Albert - Discography/Worldvibe/01 Track.flac").unwrap();
        assert_eq!(entry.artist.as_deref(), Some("Kyau & Albert"));
        let entry2 = classify("Staind - Discography/14 Shades of Grey/01 Track.flac").unwrap();
        assert_eq!(entry2.artist.as_deref(), Some("Staind"));
    }

    #[test]
    fn discography_suffix_stripping_does_not_eat_a_real_artist_literally_named_discography() {
        // An artist folder that's ONLY "Discography" (no real name left
        // after stripping) must keep the literal folder name rather than
        // becoming an empty artist.
        let entry = classify("Discography/Album/01 Track.flac").unwrap();
        assert_eq!(entry.artist.as_deref(), Some("Discography"));
    }

    #[test]
    fn audio_track_category_wrapper_folder_is_skipped_for_the_real_album_name() {
        let entry = classify("ATB/Album/Distant Earth/01 Show Me.flac").unwrap();
        assert_eq!(entry.artist.as_deref(), Some("ATB"));
        assert_eq!(entry.album.as_deref(), Some("Distant Earth"));
    }

    /// A category-named folder is only a wrapper when there's a real album
    /// segment beneath it to skip to — `Artist/Album/track.ext` (nothing
    /// after "Album") keeps "Album" as the literal album name rather than
    /// being swallowed with nothing left to replace it.
    #[test]
    fn audio_track_category_name_with_nothing_beneath_it_is_kept_literally() {
        let entry = classify("Someone/Album/track.mp3").unwrap();
        assert_eq!(entry.artist.as_deref(), Some("Someone"));
        assert_eq!(entry.album.as_deref(), Some("Album"));
    }

    #[test]
    fn audio_track_category_wrapper_folder_with_disc_subfolder_still_resolves_correctly() {
        let entry = classify("ATB/Compilation/Rare & Remixed/CD1/03 Track.flac").unwrap();
        assert_eq!(entry.artist.as_deref(), Some("ATB"));
        assert_eq!(entry.album.as_deref(), Some("Rare & Remixed"));
    }

    #[test]
    fn audio_track_with_no_album_folder_groups_under_artist_only() {
        let entry = classify("Artist/Song.mp3").unwrap();
        assert_eq!(entry.artist.as_deref(), Some("Artist"));
        assert_eq!(entry.album, None);
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

    // --- ancestor-season-folder-aware bonus/extra-content classification ---
    // Real bug: bonus content nested under a show's season folder had no
    // SxxEyy marker of its own, so it fell all the way through to
    // MediaKind::Movie and got scraped against a totally unrelated TMDb
    // movie. Confirmed live in the user's real library: this exact path got
    // matched to "The Interview" (2014), a real but completely wrong film.

    #[test]
    fn bonus_content_under_a_name_year_season_folder_is_attributed_to_the_show() {
        let entry = classify(
            "Batocera-movies-shows/Shows/Lost (2004)/Lost (2004) S03/Featurettes/Access - Granted/11. hostiles-others.mkv",
        )
        .unwrap();
        assert_eq!(entry.kind, MediaKind::Episode);
        assert_eq!(entry.show_title.as_deref(), Some("Lost"));
        assert_eq!(entry.season, Some(0), "bonus content is a single show-level bucket, not per-season");
        assert_eq!(entry.episode, None);
        assert_eq!(entry.year, Some(2004), "year falls back to the season folder's own (year)");
        assert_eq!(entry.title, "11 hostiles-others");
    }

    #[test]
    fn bonus_content_multiple_subfolders_deep_still_finds_the_season_folder() {
        // Same shape, deeper nesting (Featurettes/Interviews/), and the show
        // folder appears twice (plain "Dexter", then "Dexter (2006) S03") —
        // the nearest season-shaped ancestor wins, not the plain one above it.
        let entry =
            classify("Batocera-movies-shows/Shows/Dexter/Dexter (2006) S03/Featurettes/Interviews/Michael C. Hall.mkv")
                .unwrap();
        assert_eq!(entry.kind, MediaKind::Episode);
        assert_eq!(entry.show_title.as_deref(), Some("Dexter"));
        assert_eq!(entry.season, Some(0));
        assert_eq!(entry.episode, None);
        assert_eq!(entry.year, Some(2006));
        assert_eq!(entry.title, "Michael C Hall");
    }

    #[test]
    fn real_numbered_episode_under_the_name_year_season_folder_shape_is_unaffected() {
        // The new folder shape must not steal season/episode numbers away
        // from a real SxxEyy filename marker — that's still the primary,
        // authoritative signal when present.
        let entry = classify("Shows/Dexter/Dexter (2006) S03/Dexter.S03E01.Our Father.mkv").unwrap();
        assert_eq!(entry.kind, MediaKind::Episode);
        assert_eq!(entry.show_title.as_deref(), Some("Dexter"));
        assert_eq!(entry.season, Some(3));
        assert_eq!(entry.episode, Some(1));
    }

    #[test]
    fn real_numbered_episode_with_no_stem_prefix_falls_back_to_the_season_folder_for_show_title() {
        // Filename has no text before the SxxEyy marker at all — show_title
        // must now also try the new folder shape, not just the old
        // plain-directory fallback.
        let entry = classify("Shows/Dexter/Dexter (2006) S03/S03E01.Our Father.mkv").unwrap();
        assert_eq!(entry.show_title.as_deref(), Some("Dexter"));
        assert_eq!(entry.season, Some(3));
        assert_eq!(entry.episode, Some(1));
    }

    // --- "Ep. NN" episode markers and Shows/TV wrapper fallback ---
    // Real bug: a show numbered "Ep. NN" (no SxxEyy, no season folder) fell
    // through to the movie fallback and was searched against the wrong
    // TMDb database, always coming back "not found" even though the show
    // exists — confirmed live against "The CENTURIONS".

    #[test]
    fn ep_dot_marker_with_no_season_folder_defaults_to_season_one() {
        let entry = classify(
            "Batocera-movies-shows/Shows/The CENTURIONS/CENTURIONS - Ep. 57 - Hole in the Ocean, Part 2 (480p - DVDRip).mp4",
        )
        .unwrap();
        assert_eq!(entry.kind, MediaKind::Episode);
        assert_eq!(entry.show_title.as_deref(), Some("The CENTURIONS"));
        assert_eq!(entry.season, Some(1));
        assert_eq!(entry.episode, Some(57));
    }

    #[test]
    fn ep_marker_without_a_trailing_dot_is_also_recognized() {
        // Real example reported live: "CENTURIONS - Ep 20 - Terror on Ice"
        // — no "." after "Ep", unlike the earlier "Ep. 57" example.
        let entry =
            classify("Batocera-movies-shows/Shows/The CENTURIONS/CENTURIONS - Ep 20 - Terror on Ice.mp4").unwrap();
        assert_eq!(entry.kind, MediaKind::Episode);
        assert_eq!(entry.show_title.as_deref(), Some("The CENTURIONS"));
        assert_eq!(entry.season, Some(1));
        assert_eq!(entry.episode, Some(20));
    }

    #[test]
    fn episode_word_marker_is_also_recognized() {
        let entry = classify("Shows/Some Show/Some Show Episode 12.mkv").unwrap();
        assert_eq!(entry.kind, MediaKind::Episode);
        assert_eq!(entry.show_title.as_deref(), Some("Some Show"));
        assert_eq!(entry.episode, Some(12));
    }

    #[test]
    fn ep_marker_does_not_false_positive_inside_a_longer_word() {
        // "Deep"/"Sleep"/"Prep" all contain "ep" as a substring but not at a
        // word boundary — must not be mistaken for an episode marker.
        let entry = classify("movies/Deep Impact (1998)/Deep Impact.1998.1080p.mkv").unwrap();
        assert_eq!(entry.kind, MediaKind::Movie);
        assert_eq!(entry.episode, None);
    }

    #[test]
    fn ep_marker_uses_an_ancestor_season_folder_when_one_exists() {
        let entry = classify("Shows/Dexter/Dexter (2006) S03/Dexter - Ep. 5 - Our Father.mkv").unwrap();
        assert_eq!(entry.show_title.as_deref(), Some("Dexter"));
        assert_eq!(entry.season, Some(3));
        assert_eq!(entry.episode, Some(5));
        assert_eq!(entry.year, Some(2006));
    }

    #[test]
    fn bonus_content_under_a_shows_wrapper_with_no_season_folder_is_attributed_to_the_show() {
        // Real bug: arbitrarily-nested bonus content (no episode marker, no
        // Season N folder anywhere) under a "Shows/<Show>/..." tree fell
        // all the way through to the movie fallback and never appeared
        // under its show. Confirmed live against "Aqua Teen Hunger Force".
        let entry = classify(
            "Batocera-movies-shows/Shows/Aqua Teen Hunger Force/Featurettes/The Movie/Deleted Scenes/Dorm Room Extended.mkv",
        )
        .unwrap();
        assert_eq!(entry.kind, MediaKind::Episode);
        assert_eq!(entry.show_title.as_deref(), Some("Aqua Teen Hunger Force"));
        assert_eq!(entry.season, Some(0));
        assert_eq!(entry.episode, None);
    }

    #[test]
    fn space_separated_bare_year_is_captured_and_stripped() {
        // Real bug: this exact filename searched TMDb for the literal title
        // "Shaun of the Dead 2004" (year left unset) instead of title
        // "Shaun of the Dead" + year 2004, and came back unmatched.
        let entry = classify("Batocera-movies-shows/Shaun of the Dead 2004 (1080p x265 q22 FS78 Joy).mkv").unwrap();
        assert_eq!(entry.kind, MediaKind::Movie);
        assert_eq!(entry.title, "Shaun of the Dead");
        assert_eq!(entry.year, Some(2004));
    }

    #[test]
    fn space_separated_bare_year_prefers_the_last_match_over_a_numeral_title() {
        // A movie whose own title is a bare number that happens to fall in
        // the recognized year range ("1917") must not have that number
        // mistaken for the year when a real trailing year is also present.
        let entry = classify("movies/1917 2019 1080p BluRay x264.mkv").unwrap();
        assert_eq!(entry.year, Some(2019));
        assert_eq!(entry.title, "1917 1080p BluRay x264");
    }

    #[test]
    fn a_year_repeated_back_to_back_is_fully_collapsed_not_just_its_last_occurrence() {
        // Real bug, found live: "Interstellar.2014.2014.1080p..." only had
        // its *second* "2014" removed, leaving "Interstellar 2014" as the
        // literal search title — a redundant repeated year (identical
        // value both times) must be stripped entirely, unlike two
        // *different* year-shaped tokens (see the "1917" test above).
        let entry = classify("movies/Interstellar.2014.2014.1080p.BluRay.x264.YIFY.mp4").unwrap();
        assert_eq!(entry.year, Some(2014));
        assert_eq!(entry.title, "Interstellar 1080p BluRay x264 YIFY");
    }

    #[test]
    fn bonus_content_under_a_shows_wrapper_handles_arbitrary_extra_nesting() {
        // Real bug: any additional nesting depth under a show folder (not
        // just one level, as the earlier featurettes example covered) must
        // still resolve to the show — confirmed live against a
        // "Featurettes/The Movie/Promo Material/..." tree with no episode
        // marker and no Season N folder anywhere.
        let entry = classify(
            "Batocera-movies-shows/Shows/Aqua Teen Hunger Force/Featurettes/The Movie/Promo Material/Teaser.mkv",
        )
        .unwrap();
        assert_eq!(entry.kind, MediaKind::Episode);
        assert_eq!(entry.show_title.as_deref(), Some("Aqua Teen Hunger Force"));
        assert_eq!(entry.season, Some(0));
    }

    #[test]
    fn bonus_content_with_its_own_season_zero_marker_uses_the_wrapper_show_not_the_containing_folder() {
        // Real bug, found live: a file with its own `S00E02`-style marker
        // sitting directly inside a generic bonus-content folder
        // ("Featurettes") — no season-shaped ancestor, no text before the
        // marker — used to fall back to `show_title_from_ancestors`, which
        // naively picked the nearest folder name and got "Featurettes"
        // itself instead of the real show.
        let entry = classify(
            "Batocera-movies-shows/Shows/Aqua Teen Hunger Force/Featurettes/S00E02 Boston [youtube rip].mp4",
        )
        .unwrap();
        assert_eq!(entry.kind, MediaKind::Episode);
        assert_eq!(entry.show_title.as_deref(), Some("Aqua Teen Hunger Force"));
        assert_eq!(entry.season, Some(0));
        assert_eq!(entry.episode, Some(2));
    }

    #[test]
    fn shows_wrapper_fallback_does_not_fire_for_a_flat_movie_library() {
        // Regression guard: a plain movie library with no "Shows"/"TV"
        // segment anywhere must be completely unaffected.
        let entry = classify("Batocera-movies-shows/Blade 2 (1080p).mp4").unwrap();
        assert_eq!(entry.kind, MediaKind::Movie);
        assert_eq!(entry.show_title, None);
    }

    #[test]
    fn deeply_nested_movie_with_no_season_folder_anywhere_is_not_reclassified() {
        // Regression guard: the new ancestor walk must never fire for a
        // plain movie just because it happens to be nested a few folders
        // deep with no season-folder signal anywhere in its ancestry.
        let entry = classify("movies/Action/Best Of/Really Good Movie (2020)/Really Good Movie.2020.1080p.mkv").unwrap();
        assert_eq!(entry.kind, MediaKind::Movie);
        assert_eq!(entry.show_title, None);
    }

    #[test]
    fn season_suffix_folder_parsing_is_conservative_about_false_positives() {
        // No space before the "S" — must not match (a false positive here
        // would misclassify a real movie, worse than the bug being fixed).
        assert_eq!(parse_season_suffix_folder("MarsS03"), None);
        // Nothing before the "S" at all.
        assert_eq!(parse_season_suffix_folder("S03"), None);
        // Trailing digits with no "S" before them.
        assert_eq!(parse_season_suffix_folder("Volume 03"), None);
        // A real, valid match, no parenthetical year needed.
        assert_eq!(parse_season_suffix_folder("Show S03"), Some((3, "Show")));
    }

    // --- NxNN episode markers and bare SNN season folders ---
    // Real bug, reported after the ancestor-season-folder fix above landed:
    // neither an "NxNN" episode marker nor a bare "S06" season folder (just
    // the abbreviation, no show name attached — that lives in the parent
    // folder) was recognized at all, so a real, correctly-numbered episode
    // still fell through to MediaKind::Movie.

    #[test]
    fn nxnn_episode_marker_under_a_bare_season_folder_is_recognized() {
        let entry = classify(
            "Batocera-movies-shows/Shows/Law & Order SVU/S06/Law & Order Special Victims Unit - 6x09 - Weak.mp4",
        )
        .unwrap();
        assert_eq!(entry.kind, MediaKind::Episode);
        assert_eq!(entry.season, Some(6));
        assert_eq!(entry.episode, Some(9));
        // The bare "S06" ancestor folder resolves to a real show name (the
        // parent folder, "Law & Order SVU") — that folder-derived name now
        // wins over the filename's own stem-prefix text ("Law & Order
        // Special Victims Unit"), even though it's non-empty. See the
        // real-world reasoning on the show_title derivation above the
        // marker-branch `return` in `classify()`.
        assert_eq!(entry.show_title.as_deref(), Some("Law & Order SVU"));
    }

    #[test]
    fn folder_derived_show_name_wins_over_inconsistent_filename_wording_across_a_real_season() {
        // Real bug, found live: one folder's worth of episodes sourced from
        // different release groups had 7 filenames (out of 580) that
        // disagreed in wording with the rest ("SVU" vs "Special Victims
        // Unit", "and"/"And"/"&") — each splintering into its own
        // show_title despite being the exact same folder and show.
        for filename in [
            "Law and Order Special Victims Unit S27E02 A Waiver of Consent 1080p WEBRip 10bit DDP5 1 HEVC-d3g.mkv",
            "Law and Order SVU S27E05 1080p AMZN WEB-DL DDP5 1 H 264-FLUX.mkv",
            "Law.And.Order.SVU.S27E11.1080p.WEB.h264-ETHEL[EZTVx.to].mkv",
            "Law & Order Special Victims Unit - S27E20 - Odd Man Out.mkv",
        ] {
            let path = format!("Batocera-movies-shows/Shows/Law & Order SVU/S27/{filename}");
            let entry = classify(&path).unwrap();
            assert_eq!(entry.kind, MediaKind::Episode, "{filename}");
            assert_eq!(entry.show_title.as_deref(), Some("Law & Order SVU"), "{filename}");
            assert_eq!(entry.season, Some(27), "{filename}");
        }
    }

    #[test]
    fn nxnn_marker_show_title_falls_back_to_the_bare_season_folders_parent_when_stem_has_no_prefix() {
        // No text at all before the "6x09" marker — show_title must fall
        // back through find_ancestor_season, which for a bare "S06" folder
        // (no show name of its own) takes the name from the *parent* folder
        // above it, the same relationship a literal "Season N" folder has.
        let entry = classify("Shows/Law & Order SVU/S06/6x09.mp4").unwrap();
        assert_eq!(entry.season, Some(6));
        assert_eq!(entry.episode, Some(9));
        assert_eq!(entry.show_title.as_deref(), Some("Law & Order SVU"));
    }

    #[test]
    fn bonus_content_under_a_literal_season_folder_finds_the_real_show_past_a_wrapper_folder() {
        // Real bug, found live: bonus content organized as its own doubly-
        // nested "Featurettes/Featurettes/Season 1" structure (an unusual
        // but real-world layout — an outer bonus-content category folder,
        // then a per-season breakdown of that bonus content) was scraped
        // as a show called "Featurettes" instead of "The Office (US)
        // (2005)". show_title_from_ancestors's naive nearest-non-season-
        // folder walk stopped at the inner "Featurettes" (itself not a
        // season folder) without ever reaching the real show folder two
        // levels further up. find_ancestor_season must prefer the
        // wrapper-derived show name (the segment right below the "Shows"
        // wrapper) over that naive walk.
        let path = "Batocera-movies-shows/Shows/The Office (US) (2005) Season 1-9 S01-S09 (1080p BluRay x265 HEVC 10bit AAC 5.1 Silence)/Featurettes/Featurettes/Season 1/The Making of the Pilot.mkv";
        let entry = classify(path).unwrap();
        assert_eq!(entry.kind, MediaKind::Episode);
        // wrapper_derived_show_name doesn't run extract_year_and_strip (only
        // find_ancestor_season's other, self-contained "<Show> (<year>) SNN"
        // branch does) — same raw, uncleaned-of-year/quality-tags shape the
        // pre-existing wrapper_derived_show_name(&dirs) fallback a few lines
        // below already produces for the sibling "bonus content directly
        // under the show folder, no season folder at all" case.
        assert_eq!(
            entry.show_title.as_deref(),
            Some("The Office (US) (2005) Season 1-9 S01-S09 (1080p BluRay x265 HEVC 10bit AAC 5 1 Silence)")
        );
        assert_eq!(entry.season, Some(0));
    }

    #[test]
    fn nxnn_marker_is_conservative_about_false_positives() {
        // No digits before the "x" at all.
        assert_eq!(parse_nxnn_marker("Show - x09 - Title"), None);
        // No digits after the "x" at all.
        assert_eq!(parse_nxnn_marker("Show - 6x - Title"), None);
        // Digits before "x" exist and are the right count, but the run
        // isn't left-bounded — "23" is preceded directly by "m" (from
        // "Item"), not a separator, so this is part of a longer
        // alphanumeric run, not a real marker — must not misfire.
        assert_eq!(parse_nxnn_marker("Item23x09"), None);
        // A real, valid match with mixed case "X".
        assert_eq!(parse_nxnn_marker("Show - 6X09 - Title"), Some((6, 9, "Show - ")));
    }

    #[test]
    fn bare_season_folder_parsing_is_conservative_about_false_positives() {
        // A combined season+episode folder name ("S06E01") is a different
        // real convention (one episode flattened directly under a folder
        // naming both numbers) — must not be misread as a bare season
        // folder just because it starts with "S" + digits.
        assert_eq!(parse_bare_season_folder("S06E01"), None);
        // Nothing after the "S" at all.
        assert_eq!(parse_bare_season_folder("S"), None);
        // Doesn't start with "S".
        assert_eq!(parse_bare_season_folder("06"), None);
        // Real, valid matches, both digit widths, case-insensitive.
        assert_eq!(parse_bare_season_folder("S06"), Some(6));
        assert_eq!(parse_bare_season_folder("s6"), Some(6));
    }

    #[test]
    fn nxnn_marker_does_not_steal_priority_from_an_existing_sxxeyy_marker() {
        // A filename with a real SxxEyy marker must keep using it even if
        // an NxNN-shaped substring could also coincidentally be found —
        // SxxEyy is tried first, unconditionally, unchanged from before.
        let entry = classify("Shows/The Expanse/Season 2/The.Expanse.S02E05.Home.mkv").unwrap();
        assert_eq!(entry.season, Some(2));
        assert_eq!(entry.episode, Some(5));
    }
}
