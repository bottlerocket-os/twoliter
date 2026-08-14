//! Integration test that runs the bash-side `uki_bootconfig_cmdline` test
//! suite.
//!
//! The actual assertions live in `embedded/tests/test_uki_bootconfig.sh`,
//! which sources `imghelper` as a library and exercises
//! `uki_bootconfig_cmdline` across UKI+FIPS, UKI+non-FIPS, and non-UKI
//! variant names. This Rust wrapper exists so the bash tests run under
//! `cargo test` alongside the validator tests in the `buildsys` crate.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn uki_bootconfig_bash_tests() {
    // The `embedded` directory is a symlink to `twoliter/embedded` at the
    // crate root, so `CARGO_MANIFEST_DIR` reaches the script either way.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let script = manifest_dir.join("embedded/tests/test_uki_bootconfig.sh");

    assert!(
        script.is_file(),
        "test_uki_bootconfig.sh not found at {}",
        script.display(),
    );

    let output = Command::new("bash")
        .arg(&script)
        .output()
        .expect("failed to invoke bash");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Always print so failures show what the bash runner saw.
    println!("--- test_uki_bootconfig.sh stdout ---\n{stdout}");
    if !stderr.is_empty() {
        println!("--- test_uki_bootconfig.sh stderr ---\n{stderr}");
    }

    assert!(
        output.status.success(),
        "test_uki_bootconfig.sh failed with status {:?}",
        output.status,
    );
}
