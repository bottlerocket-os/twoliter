use super::{run_command, test_projects_dir, TWOLITER_PATH};

const EXPECTED_LOCKFILE: &str = r#"schema-version = 1

[sdk]
name = "bottlerocket-sdk"
version = "0.42.0"
vendor = "bottlerocket"
source = "public.ecr.aws/bottlerocket/bottlerocket-sdk:v0.42.0"
digest = "myHHKE41h9qfeyR6V6HB0BfiLPwj3QEFLUFy4TXcR10="

[[kit]]
name = "bottlerocket-core-kit"
version = "2.0.0"
vendor = "custom-vendor"
source = "public.ecr.aws/bottlerocket/bottlerocket-core-kit:v2.0.0"
digest = "vlTsAAbSCzXFZofVmw8pLLkRjnG/y8mtb2QsQBSz1zk="
"#;

#[tokio::test]
#[ignore]
/// Generates a Twoliter.lock file for the `external-kit` project using docker
async fn test_twoliter_update_docker() {
    let external_kit = test_projects_dir().join("external-kit");

    let lockfile = external_kit.join("Twoliter.lock");
    tokio::fs::remove_file(&lockfile).await.ok();

    let output = run_command(
        TWOLITER_PATH,
        [
            "update",
            "--project-path",
            external_kit.join("Twoliter.toml").to_str().unwrap(),
        ],
        [("TWOLITER_KIT_IMAGE_TOOL", "docker")],
    )
    .await;

    assert!(output.status.success());

    let lock_contents = tokio::fs::read_to_string(&lockfile).await.unwrap();
    assert_eq!(lock_contents, EXPECTED_LOCKFILE);

    tokio::fs::remove_file(&lockfile).await.ok();
}

#[tokio::test]
#[ignore]
/// Generates a Twoliter.lock file for the `external-kit` project using crane
async fn test_twoliter_update_crane() {
    let external_kit = test_projects_dir().join("external-kit");

    let lockfile = external_kit.join("Twoliter.lock");
    tokio::fs::remove_file(&lockfile).await.ok();

    let output = run_command(
        TWOLITER_PATH,
        [
            "update",
            "--project-path",
            external_kit.join("Twoliter.toml").to_str().unwrap(),
        ],
        [("TWOLITER_KIT_IMAGE_TOOL", "crane")],
    )
    .await;

    assert!(output.status.success());

    let lock_contents = tokio::fs::read_to_string(&lockfile).await.unwrap();
    assert_eq!(lock_contents, EXPECTED_LOCKFILE);

    tokio::fs::remove_file(&lockfile).await.ok();
}
