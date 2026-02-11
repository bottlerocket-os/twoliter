use include_env_compressed::{include_archive_from_env, Archive};

pub const ADVISORY_CHECKER: Archive = include_archive_from_env!("CARGO_BIN_FILE_ADVISORY_CHECKER");
