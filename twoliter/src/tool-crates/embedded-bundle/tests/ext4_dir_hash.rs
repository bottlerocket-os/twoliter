//! Runs `embedded/tests/test_ext4_dir_hash.sh` under `cargo test`.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn ext4_dir_hash_bash_tests() {
    // `embedded` is a symlink to `twoliter/embedded`.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let script = manifest_dir.join("embedded/tests/test_ext4_dir_hash.sh");

    assert!(
        script.is_file(),
        "test_ext4_dir_hash.sh not found at {}",
        script.display(),
    );

    let output = Command::new("bash")
        .arg(&script)
        .output()
        .expect("failed to invoke bash");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("--- test_ext4_dir_hash.sh stdout ---\n{stdout}");
    if !stderr.is_empty() {
        println!("--- test_ext4_dir_hash.sh stderr ---\n{stderr}");
    }

    assert!(
        output.status.success(),
        "test_ext4_dir_hash.sh failed with status {:?}",
        output.status,
    );
}
