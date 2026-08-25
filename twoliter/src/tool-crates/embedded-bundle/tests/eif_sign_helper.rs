use std::path::PathBuf;
use std::process::Command;

#[test]
fn eif_sign_helper_bash_tests() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let script = manifest_dir.join("embedded/tests/test_eif_sign_helper.sh");

    assert!(
        script.is_file(),
        "test_eif_sign_helper.sh not found at {}",
        script.display(),
    );

    let output = Command::new("bash")
        .arg(&script)
        .output()
        .expect("failed to invoke bash");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("--- test_eif_sign_helper.sh stdout ---\n{stdout}");
    if !stderr.is_empty() {
        println!("--- test_eif_sign_helper.sh stderr ---\n{stderr}");
    }

    assert!(
        output.status.success(),
        "test_eif_sign_helper.sh failed with status {:?}",
        output.status,
    );
}
