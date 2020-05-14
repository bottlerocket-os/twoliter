#![allow(clippy::default_trait_access)]

use semver::Version;
use snafu::{Backtrace, Snafu};
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Snafu)]
#[snafu(visibility = "pub(crate)")]
pub enum Error {
    #[snafu(display("Bad bound field: {}", bound_str))]
    BadBound {
        backtrace: Backtrace,
        source: std::num::ParseIntError,
        bound_str: String,
    },

    #[snafu(display("Invalid bound start: {}", key))]
    BadBoundKey {
        source: std::num::ParseIntError,
        key: String,
        backtrace: Backtrace,
    },

    #[snafu(display("Could not parse OS version: {}", key))]
    BadVersion {
        backtrace: Backtrace,
        key: String,
        source: semver::SemVerError,
    },

    #[snafu(display("Could not parse OS versions: {}", key))]
    BadDataVersionsFromTo { backtrace: Backtrace, key: String },

    #[snafu(display("Could not parse image version: {} - {}", key, value))]
    BadMapVersion {
        backtrace: Backtrace,
        key: String,
        value: String,
    },

    #[snafu(display("Migration {} matches regex but missing version", name))]
    BadRegexVersion { name: String },

    #[snafu(display("Migration {} matches regex but missing name", name))]
    BadRegexName { name: String },

    #[snafu(display(
        "Unable to parse 'start_after' offset from string '{}': {}",
        offset,
        source
    ))]
    BadOffset {
        offset: String,
        source: parse_datetime::Error,
    },

    #[snafu(display("Duplicate key ID: {}", keyid))]
    DuplicateKeyId { backtrace: Backtrace, keyid: u32 },

    #[snafu(display("Duplicate version key: {}", key))]
    DuplicateVersionKey { backtrace: Backtrace, key: String },

    #[snafu(display("Manifest not found in repository"))]
    ManifestNotFound { backtrace: Backtrace },

    #[snafu(display("Failed to parse manifest file: {}", source))]
    ManifestParse {
        source: serde_json::Error,
        backtrace: Backtrace,
    },

    #[snafu(display("Failed to read manifest file {}: {}", path.display(), source))]
    ManifestRead {
        path: PathBuf,
        source: std::io::Error,
        backtrace: Backtrace,
    },

    #[snafu(display("Failed to write manifest file {}: {}", path.display(), source))]
    ManifestWrite {
        path: PathBuf,
        source: std::io::Error,
        backtrace: Backtrace,
    },

    #[snafu(display("Manifest load error: {}", source))]
    ManifestLoad {
        source: tough::error::Error,
        backtrace: Backtrace,
    },

    #[snafu(display(
        "Migration {} given for {} but name implies it is for {}",
        name,
        to,
        version
    ))]
    MigrationInvalidTarget {
        backtrace: Backtrace,
        name: String,
        to: Version,
        version: Version,
    },

    #[snafu(display(
        "Migration name invalid; must follow format 'migrate_${{TO_VERSION}}_${{NAME}}'"
    ))]
    MigrationNaming { backtrace: Backtrace },

    #[snafu(display("Unable to get mutable reference to ({},{}) migrations", from, to))]
    MigrationMutable {
        backtrace: Backtrace,
        from: Version,
        to: Version,
    },

    #[snafu(display(
        "Reached end of migration chain at {} but target is {}",
        current,
        target
    ))]
    MissingMigration {
        backtrace: Backtrace,
        current: Version,
        target: Version,
    },

    #[snafu(display("Failed to serialize update information: {}", source))]
    UpdateSerialize {
        source: serde_json::Error,
        backtrace: Backtrace,
    },

    #[snafu(display("Waves are not ordered; percentages and dates must be in ascending order"))]
    WavesUnordered,

    #[snafu(display(
        "`fleet_percentage` must be a value between 1 - 100: value provided: {}",
        provided
    ))]
    InvalidFleetPercentage { provided: u32 },
}
