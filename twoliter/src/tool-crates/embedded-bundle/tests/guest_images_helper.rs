//! Integration test that runs the bash-side `guest-images-helper` test suite.
//!
//! The actual assertions live in
//! `embedded/tests/test_guest_images_helper.sh`, which exercises:
//!
//! * the `imghelper` <-> `guest-images-helper` drift-detection contract
//!   (the `IMAGE_ARTIFACT_SUFFIXES` allowlist is the single source of
//!   truth for embeddable image artifacts);
//! * `compress_image`'s rejection of unknown extensions;
//! * every `compress_image "<ext>"` call site in `rpm2img` and `img2img`
//!   uses a declared suffix;
//! * `copy_guest_image_artifacts` copies allowlisted files, drops
//!   sidecar metadata (SBOM, inventory, artifact-metadata), and
//!   preserves symlinks.
//!
//! This Rust wrapper exists so the bash tests run under `cargo test`
//! alongside the other embedded-bundle and buildsys tests.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn guest_images_helper_bash_tests() {
    // The `embedded` directory is a symlink to `twoliter/embedded` at the
    // crate root, so `CARGO_MANIFEST_DIR` reaches the script either way.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let script = manifest_dir.join("embedded/tests/test_guest_images_helper.sh");

    assert!(
        script.is_file(),
        "test_guest_images_helper.sh not found at {}",
        script.display(),
    );

    let output = Command::new("bash")
        .arg(&script)
        .output()
        .expect("failed to invoke bash");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Always print so failures show what the bash runner saw.
    println!("--- test_guest_images_helper.sh stdout ---\n{stdout}");
    if !stderr.is_empty() {
        println!("--- test_guest_images_helper.sh stderr ---\n{stderr}");
    }

    assert!(
        output.status.success(),
        "test_guest_images_helper.sh failed with status {:?}",
        output.status,
    );
}
