use include_env_compressed::{include_archive_from_env, Archive};

pub const PCRSYS: Archive = include_archive_from_env!("CARGO_BIN_FILE_PCRSYS");
