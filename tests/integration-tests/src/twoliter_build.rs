use super::{run_command, test_projects_dir, TWOLITER_PATH};
use std::path::Path;
use tempfile::TempDir;

#[test]
#[ignore]
fn test_workspace_symlinks_not_followed() {
    // Ensure a symlinked `Twoliter.toml` does not trick us into putting our build directory into
    // the symlink target's parent directory.

    let target_kit = test_projects_dir().join("local-kit");
    let work_dir = copy_project_to_temp_dir(&target_kit);

    let work_dir_path = work_dir.path();

    let working_twoliter_toml = work_dir_path.join("Twoliter.toml");

    // Replace Twoliter.toml with a symlink
    std::fs::remove_file(&working_twoliter_toml).unwrap();
    std::os::unix::fs::symlink(
        target_kit.join("Twoliter.toml"),
        work_dir_path.join("Twoliter.toml"),
    )
    .unwrap();

    assert!(!work_dir_path.join("build").is_dir());

    run_command(
        TWOLITER_PATH,
        [
            "update",
            "--project-path",
            working_twoliter_toml.to_str().unwrap(),
        ],
        [],
    );
    run_command(
        TWOLITER_PATH,
        [
            "fetch",
            "--project-path",
            working_twoliter_toml.to_str().unwrap(),
        ],
        [],
    );

    assert!(work_dir_path.join("build").is_dir());
}

pub(crate) fn copy_project_to_temp_dir(project: impl AsRef<Path>) -> TempDir {
    let temp_dir = TempDir::new().unwrap();
    copy_most_dirs_recursively(project, &temp_dir);
    temp_dir
}

/// Copy dirs recursively except for some of the larger "ignoreable" dirs that may exist in the
/// user's checkout.
fn copy_most_dirs_recursively(src: impl AsRef<Path>, dst: impl AsRef<Path>) {
    let src = src.as_ref();
    let dst = dst.as_ref();
    for entry in std::fs::read_dir(src).unwrap() {
        std::fs::create_dir_all(dst).unwrap();
        let entry = entry.unwrap();
        let file_type = entry.file_type().unwrap();
        if file_type.is_dir() {
            let name = entry.file_name().to_str().unwrap().to_string();
            if matches!(name.as_ref(), "target" | "build" | ".gomodcache" | ".cargo") {
                continue;
            }
            copy_most_dirs_recursively(entry.path(), dst.join(entry.file_name()));
        } else {
            std::fs::copy(entry.path(), dst.join(entry.file_name())).unwrap();
        }
    }
}
