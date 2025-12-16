use crate::run_command;
use std::path::PathBuf;
use tempfile::TempDir;

fn imghelper_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("twoliter/embedded/imghelper")
}

fn test_sanity_checks(sbom_arg: Option<&str>, expect_success: bool) {
    let temp_dir = TempDir::new().unwrap();
    let ovf_template = temp_dir.path().join("test.ovf");
    std::fs::write(&ovf_template, "dummy ovf content").unwrap();

    let imghelper = imghelper_path();
    let mut script = format!(
        r#"
        source "{}"
        sanity_checks "raw" "split" "{}" "no" {}
        "#,
        imghelper.display(),
        ovf_template.display(),
        sbom_arg.map(|s| format!("\"{}\"", s)).unwrap_or_default()
    );

    let output = run_command(
        "bash",
        ["-c", &script],
        [
            ("IMAGE_NAME", "test"),
            ("VARIANT", "test"),
            ("ARCH", "x86_64"),
            ("VERSION_ID", "1.0.0"),
            ("BUILD_ID", "1"),
        ],
    );

    if expect_success {
        assert!(output.status.success(), "sanity_checks should succeed");
    } else {
        assert!(!output.status.success(), "sanity_checks should fail");
    }
}

// Tests for optional sbom_package_dir argument
// sanity_checks is called by rpm2img and img2img
// rpm2img requires calling it with 5 args, while
// img2img requires calling it with 4 args.

#[test]
fn test_sanity_checks_without_sbom_dir() {
    // Omitting sbom_package_dir should succeed (optional arg)
    test_sanity_checks(None, true);
}

#[test]
fn test_sanity_checks_with_empty_sbom_dir() {
    // Empty string should succeed (treated as unset)
    test_sanity_checks(Some(""), true);
}

#[test]
fn test_sanity_checks_with_valid_sbom_dir() {
    // Valid directory arg should succeed
    let temp_dir = TempDir::new().unwrap();
    test_sanity_checks(Some(temp_dir.path().to_str().unwrap()), true);
}

#[test]
fn test_sanity_checks_with_nonexistent_sbom_dir() {
    // Non-existent path should fail (validation still works when provided)
    test_sanity_checks(Some("/nonexistent/path"), false);
}
