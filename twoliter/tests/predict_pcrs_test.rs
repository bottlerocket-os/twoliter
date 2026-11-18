//! Integration tests for predict-pcrs subcommand.
//!
//! Run with:
//! ```
//! PREDICT_PCRS_TEST_AMI=<ami> cargo test --release --locked -p twoliter --test predict_pcrs_test -- --ignored predict_pcrs_owned_ami
//! ```

/// Test predict-pcrs with an AMI the caller owns.
///
/// Requires AWS credentials and an AMI in the caller's account.
#[tokio::test]
#[ignore]
async fn predict_pcrs_owned_ami() {
    use std::process::Command;

    let ami_id = std::env::var("PREDICT_PCRS_TEST_AMI")
        .expect("set PREDICT_PCRS_TEST_AMI to an AMI in your account");
    let binary = env!("CARGO_BIN_EXE_twoliter");

    let output = Command::new(binary)
        .args(["predict-pcrs", "--ami-id", &ami_id, "--region", "us-west-2"])
        .output()
        .expect("failed to run twoliter predict-pcrs");

    assert!(
        output.status.success(),
        "predict-pcrs failed with exit code {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout_str = String::from_utf8(output.stdout).expect("invalid UTF-8 in stdout");
    let json: serde_json::Value =
        serde_json::from_str(&stdout_str).expect("stdout is not valid JSON");

    let pcrs = json
        .get("pcrs")
        .expect("JSON output missing 'pcrs' field")
        .as_object()
        .expect("'pcrs' is not an object");

    assert!(!pcrs.is_empty(), "pcrs object is empty");
}
