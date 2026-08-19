use include_env_compressed::{include_archive_from_env, Archive};

/// Compressed archive containing the built `ukisys` binary.
pub const UKISYS: Archive = include_archive_from_env!("CARGO_BIN_FILE_UKISYS");
