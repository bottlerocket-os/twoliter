/*!

This directory and module are for tests, test data, and re-usable test code. This module should only
be compiled for `cfg(test)`, which is accomplished at its declaration in `main.rs`.

!*/

use std::path::PathBuf;

/// Return the canonical path to the directory where we store test data.
pub(crate) fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("test")
        .join("data")
        .canonicalize()
        .unwrap()
}
