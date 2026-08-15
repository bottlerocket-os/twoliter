use crate::{run_command, TWOLITER_PATH};
use duct::cmd;
use std::path::Path;
use tempfile::TempDir;
use which::which;

/// Create a test project by cloning bottlerocket
async fn create_test_project() -> TempDir {
    let tmp_dir = TempDir::new().expect("failed to create temporary directory");
    let git = which("git").expect("failed to find git");
    let inside = tmp_dir.path().to_str().unwrap();

    cmd!(
        git,
        "clone",
        "--depth",
        "1",
        "https://github.com/bottlerocket-os/bottlerocket.git",
        inside
    )
    .run()
    .expect("failed to clone bottlerocket");

    tmp_dir
}

/// Run twoliter make with a specific target
fn twoliter_make(
    project_dir: &Path,
    target: &str,
    variant: &str,
    arch: &str,
) -> std::process::Output {
    let cargo_home = project_dir.join(".cargo");
    std::fs::create_dir_all(&cargo_home).unwrap();

    run_command(
        TWOLITER_PATH,
        [
            "make",
            "--project-path",
            project_dir.join("Twoliter.toml").to_str().unwrap(),
            "--cargo-home",
            cargo_home.to_str().unwrap(),
            "--arch",
            arch,
            target,
        ],
        [("BUILDSYS_VARIANT", variant)],
    )
}

/// Test that build-variant runs successfully via twoliter make.
#[tokio::test]
#[ignore]
async fn test_twoliter_build_variant() {
    let bob_src = create_test_project().await;
    let project_path = bob_src.path().join("Twoliter.toml");
    let arch = std::env::consts::ARCH;
    let variant = "aws-ecs-2";

    // Update
    let output = run_command(
        TWOLITER_PATH,
        ["update", "--project-path", project_path.to_str().unwrap()],
        [],
    );
    assert!(output.status.success(), "twoliter update failed");

    // Fetch
    let output = run_command(
        TWOLITER_PATH,
        [
            "fetch",
            "--project-path",
            project_path.to_str().unwrap(),
            "--arch",
            arch,
        ],
        [],
    );
    assert!(output.status.success(), "twoliter fetch failed");

    // Build variant
    let output = twoliter_make(bob_src.path(), "build-variant", variant, arch);
    assert!(
        output.status.success(),
        "twoliter make build-variant failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Test that repack-variant runs successfully via twoliter make.
/// This requires a previously built variant image to repack.
#[tokio::test]
#[ignore]
async fn test_twoliter_repack_variant() {
    let bob_src = create_test_project().await;
    let project_path = bob_src.path().join("Twoliter.toml");
    let arch = std::env::consts::ARCH;
    let variant = "aws-ecs-2";

    // Update
    let output = run_command(
        TWOLITER_PATH,
        ["update", "--project-path", project_path.to_str().unwrap()],
        [],
    );
    assert!(output.status.success(), "twoliter update failed");

    // Fetch
    let output = run_command(
        TWOLITER_PATH,
        [
            "fetch",
            "--project-path",
            project_path.to_str().unwrap(),
            "--arch",
            arch,
        ],
        [],
    );
    assert!(output.status.success(), "twoliter fetch failed");

    // First build the variant
    let output = twoliter_make(bob_src.path(), "build-variant", variant, arch);
    assert!(
        output.status.success(),
        "twoliter make build-variant failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Then repack it
    let output = twoliter_make(bob_src.path(), "repack-variant", variant, arch);
    assert!(
        output.status.success(),
        "twoliter make repack-variant failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
