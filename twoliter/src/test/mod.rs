/*!

This directory and module are for tests, test data, and re-usable test code. This module should only
be compiled for `cfg(test)`, which is accomplished at its declaration in `main.rs`.

!*/
mod cargo_make;

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

pub(crate) fn projects_dir() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.join("tests").join("projects").canonicalize().unwrap()
}
