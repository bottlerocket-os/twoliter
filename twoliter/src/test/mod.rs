/*!

This directory and module are for tests, test data, and re-usable test code. This module should only
be compiled for `cfg(test)`, which is accomplished at its declaration in `main.rs`.

!*/

#![allow(unused)]

#[cfg(feature = "integ-tests")]
mod build_kit;
#[cfg(feature = "integ-tests")]
mod cargo_make;

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

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

pub(crate) fn project_dir(name: &str) -> PathBuf {
    let path = projects_dir().join(name);
    path.canonicalize()
        .expect(&format!("Unable to canonicalize '{}'", path.display()))
}

fn copy_project_to_temp_dir(project: &str) -> TempDir {
    let temp_dir = TempDir::new().unwrap();
    let src = project_dir(project);
    let dst = temp_dir.path();
    copy_most_dirs_recursively(&src, dst);
    temp_dir
}

/// Copy dirs recursively except for some of the larger "ignoreable" dirs that may exist in the
/// user's checkout.
fn copy_most_dirs_recursively(src: &Path, dst: &Path) {
    for entry in fs::read_dir(src).unwrap() {
        fs::create_dir_all(&dst).unwrap();
        let entry = entry.unwrap();
        let file_type = entry.file_type().unwrap();
        if file_type.is_dir() {
            let name = entry.file_name().to_str().unwrap().to_string();
            if matches!(name.as_ref(), "target" | "build" | ".gomodcache" | ".cargo") {
                continue;
            }
            copy_most_dirs_recursively(&entry.path(), &dst.join(entry.file_name()));
        } else {
            fs::copy(entry.path(), dst.join(entry.file_name())).unwrap();
        }
    }
}
