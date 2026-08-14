//! Integration test that runs the bash-side `provide_loader_config` test
//! suite.
//!
//! The actual assertions live in
//! `embedded/tests/test_provide_loader_config.sh`, which exercises:
//!
//! * `provide_loader_config` (defined in `imghelper`) fails hard with an
//!   actionable error naming the searched `loader/loader.conf` path when
//!   the systemd-boot loader configuration is missing from the ESP
//!   staging mount, instead of silently continuing with systemd-boot's
//!   compiled-in defaults.
//!
//! This Rust wrapper exists so the bash tests run under `cargo test`
//! alongside the other embedded-bundle and buildsys tests.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn provide_loader_config_bash_tests() {
    // The `embedded` directory is a symlink to `twoliter/embedded` at the
    // crate root, so `CARGO_MANIFEST_DIR` reaches the script either way.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let script = manifest_dir.join("embedded/tests/test_provide_loader_config.sh");

    assert!(
        script.is_file(),
        "test_provide_loader_config.sh not found at {}",
        script.display(),
    );

    let output = Command::new("bash")
        .arg(&script)
        .output()
        .expect("failed to invoke bash");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Always print so failures show what the bash runner saw.
    println!("--- test_provide_loader_config.sh stdout ---\n{stdout}");
    if !stderr.is_empty() {
        println!("--- test_provide_loader_config.sh stderr ---\n{stderr}");
    }

    assert!(
        output.status.success(),
        "test_provide_loader_config.sh failed with status {:?}",
        output.status,
    );
}
