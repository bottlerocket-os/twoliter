use std::{fs, process::Command};

#[test]
fn rpm2img_wires_plain_and_fips_bootconfig_before_uki_assembly() {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let embedded = workspace.join("twoliter/embedded");
    let dir = tempfile::tempdir().unwrap();
    let result = Command::new("bash")
        .arg(embedded.join("tests/test_uki_cmdline.sh"))
        .arg(&embedded)
        .arg(workspace.join("tools/ukisys/tests/fixtures"))
        .arg(env!("CARGO_BIN_EXE_ukisys"))
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn invalid_input_does_not_publish_output() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("bad.data");
    let output = dir.path().join("cmdline");
    fs::write(&input, b"not a bootconfig").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_ukisys"))
        .args(["cmdline", "--bootconfig"])
        .arg(input)
        .args(["--kernel-args", "", "--init-args", "", "--output"])
        .arg(&output)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!output.exists());
}
