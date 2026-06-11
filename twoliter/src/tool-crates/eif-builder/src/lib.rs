use include_env_compressed::{include_archive_from_env, Archive};

pub const EIF_BUILD_BIN: Archive =
    include_archive_from_env!("CARGO_BIN_FILE_EIF_BUILDER_eif-builder");
