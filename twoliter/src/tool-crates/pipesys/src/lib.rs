use include_env_compressed::{include_archive_from_env, Archive};

pub const PIPESYS: Archive = include_archive_from_env!("CARGO_BIN_FILE_PIPESYS");
