//! HTTP-semantics byte-range resolution for direct play (the port of
//! Batocera.Drone's `http_range.py`, adapted to the typed `ByteRange`):
//! 200 for no range, 206 with clamped bounds for a satisfiable one, 416 for
//! an unsatisfiable one. Range end is inclusive.

use swarm_core::peer::{ByteRange, ContentRange};

#[derive(Debug, PartialEq, Eq)]
pub enum ResolvedRange {
    /// No range requested — whole entity, status 200.
    Full { len: u64 },
    /// Satisfiable range — status 206 with this content-range.
    Partial(ContentRange),
    /// Unsatisfiable — status 416.
    Unsatisfiable,
}

pub fn resolve(range: Option<ByteRange>, total: u64) -> ResolvedRange {
    let Some(range) = range else {
        return ResolvedRange::Full { len: total };
    };
    if total == 0 {
        return ResolvedRange::Unsatisfiable;
    }
    match range {
        ByteRange::FromTo { start, end } => {
            if start >= total {
                return ResolvedRange::Unsatisfiable;
            }
            let end = end.map_or(total - 1, |e| e.min(total - 1));
            if end < start {
                return ResolvedRange::Unsatisfiable;
            }
            ResolvedRange::Partial(ContentRange { start, end, total })
        }
        ByteRange::Suffix { last } => {
            if last == 0 {
                return ResolvedRange::Unsatisfiable;
            }
            let start = total.saturating_sub(last);
            ResolvedRange::Partial(ContentRange {
                start,
                end: total - 1,
                total,
            })
        }
    }
}

/// MIME type from the file extension, for player container sniffing.
pub fn content_type(relative_path: &str) -> &'static str {
    match relative_path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mov" => "video/quicktime",
        "mpg" | "mpeg" => "video/mpeg",
        "ts" | "m2ts" => "video/mp2t",
        "wmv" => "video/x-ms-wmv",
        "flv" => "video/x-flv",
        "3gp" => "video/3gpp",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" | "opus" => "audio/ogg",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "wav" => "audio/wav",
        "aiff" => "audio/aiff",
        "wma" => "audio/x-ms-wma",
        "ape" => "audio/x-ape",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_range_is_full() {
        assert_eq!(resolve(None, 100), ResolvedRange::Full { len: 100 });
    }

    #[test]
    fn open_ended_range() {
        assert_eq!(
            resolve(
                Some(ByteRange::FromTo {
                    start: 10,
                    end: None
                }),
                100
            ),
            ResolvedRange::Partial(ContentRange {
                start: 10,
                end: 99,
                total: 100
            })
        );
    }

    #[test]
    fn end_is_clamped() {
        assert_eq!(
            resolve(
                Some(ByteRange::FromTo {
                    start: 0,
                    end: Some(1_000_000)
                }),
                100
            ),
            ResolvedRange::Partial(ContentRange {
                start: 0,
                end: 99,
                total: 100
            })
        );
    }

    #[test]
    fn start_past_eof_is_unsatisfiable() {
        assert_eq!(
            resolve(
                Some(ByteRange::FromTo {
                    start: 100,
                    end: None
                }),
                100
            ),
            ResolvedRange::Unsatisfiable
        );
    }

    #[test]
    fn suffix_range() {
        assert_eq!(
            resolve(Some(ByteRange::Suffix { last: 10 }), 100),
            ResolvedRange::Partial(ContentRange {
                start: 90,
                end: 99,
                total: 100
            })
        );
        // Suffix longer than the file means the whole file.
        assert_eq!(
            resolve(Some(ByteRange::Suffix { last: 500 }), 100),
            ResolvedRange::Partial(ContentRange {
                start: 0,
                end: 99,
                total: 100
            })
        );
        assert_eq!(
            resolve(Some(ByteRange::Suffix { last: 0 }), 100),
            ResolvedRange::Unsatisfiable
        );
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type("movies/a.MKV"), "video/x-matroska");
        assert_eq!(content_type("music/a.flac"), "audio/flac");
        assert_eq!(content_type("weird.bin"), "application/octet-stream");
    }
}
