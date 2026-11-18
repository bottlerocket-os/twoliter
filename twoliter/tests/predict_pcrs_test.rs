//! Integration tests for predict-pcrs subcommand.
//!
//! Run with:
//! ```
//! cargo test --release --locked -p twoliter --test predict_pcrs_test -- --ignored predict_pcrs_not_owned_ami
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

/// Test predict-pcrs with an AMI not owned by the caller.
///
/// Requires AWS credentials. Creates no resources.
#[tokio::test]
#[ignore]
async fn predict_pcrs_not_owned_ami() {
    use std::process::Command;

    // Resolve official AMI from SSM
    let ssm_output = Command::new("aws")
        .args([
            "ssm",
            "get-parameter",
            "--name",
            "/aws/service/bottlerocket/aws-mantle-1/x86_64/latest/image_id",
            "--region",
            "us-west-2",
            "--query",
            "Parameter.Value",
            "--output",
            "text",
        ])
        .output()
        .expect("failed to run aws ssm get-parameter");

    assert!(
        ssm_output.status.success(),
        "aws ssm get-parameter failed: {}",
        String::from_utf8_lossy(&ssm_output.stderr)
    );

    let ami_id = String::from_utf8(ssm_output.stdout)
        .expect("invalid UTF-8 in SSM output")
        .trim()
        .to_string();

    let binary = env!("CARGO_BIN_EXE_twoliter");
    let output = Command::new(binary)
        .args(["predict-pcrs", "--ami-id", &ami_id, "--region", "us-west-2"])
        .output()
        .expect("failed to run twoliter predict-pcrs");

    assert!(
        !output.status.success(),
        "predict-pcrs should fail with official AMI"
    );

    let stderr_str = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr_str.contains("pcrsys can only read AMIs in your account"),
        "stderr should contain ownership error message"
    );
    assert!(
        stderr_str.contains("aws ec2 copy-image"),
        "stderr should contain copy-image command"
    );

    assert!(output.stdout.is_empty(), "stdout should be empty on error");
}
