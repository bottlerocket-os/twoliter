//! Runs `embedded/tests/test_deterministic_ids.sh` under `cargo test`.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn deterministic_ids_bash_tests() {
    // The `embedded` directory is a symlink to `twoliter/embedded` at the
    // crate root, so `CARGO_MANIFEST_DIR` reaches the script either way.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let script = manifest_dir.join("embedded/tests/test_deterministic_ids.sh");

    assert!(
        script.is_file(),
        "test_deterministic_ids.sh not found at {}",
        script.display(),
    );

    let output = Command::new("bash")
        .arg(&script)
        .output()
        .expect("failed to invoke bash");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("--- test_deterministic_ids.sh stdout ---\n{stdout}");
    if !stderr.is_empty() {
        println!("--- test_deterministic_ids.sh stderr ---\n{stderr}");
    }

    assert!(
        output.status.success(),
        "test_deterministic_ids.sh failed with status {:?}",
        output.status,
    );
}
