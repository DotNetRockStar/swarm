//! Metadata scraping: TMDb for movies/TV (user-supplied free key), keyless
//! MusicBrainz + Cover Art Archive + Wikimedia Commons for music. Same
//! source stack as Batocera.Drone, chosen there (and here) for being
//! license-clean and signup-free beyond the one TMDb key.

pub mod artwork;
pub mod coverart;
pub mod musicbrainz;
pub mod runner;
pub mod tmdb;
pub mod wikimedia;

pub use runner::{run_bulk_scrape, BulkScrapeReport, ScrapeConfig};
