use super::{run_command, test_projects_dir, KitRegistry, TWOLITER_PATH};

const INFRA_TOML: &str = r#"
[vendor.bottlerocket]
registry = "localhost:5000"
"#;

const TWOLITER_OVERRIDE: &str = r#"
[custom-vendor.core-kit]
registry = "localhost:5000"
name = "core-kit-overridden"
"#;

#[test]
#[ignore]
/// Generates a Twoliter.lock file for the `external-kit` project using crane
fn test_twoliter_build_and_update() {
    let external_kit = test_projects_dir().join("external-kit");
    let lockfile = external_kit.join("Twoliter.lock");
    std::fs::remove_file(&lockfile).ok();
    let override_file = external_kit.join("Twoliter.override");
    std::fs::remove_file(&override_file).ok();

    // Build & push a local kit to the registry
    let registry = KitRegistry::new();
    LocalKit::build(&registry);

    // Point twoliter to the local registry as an override
    std::fs::write(&override_file, TWOLITER_OVERRIDE).unwrap();
    let output = run_command(
        TWOLITER_PATH,
        [
            "update",
            "--project-path",
            external_kit.join("Twoliter.toml").to_str().unwrap(),
        ],
        [
            ("TWOLITER_KIT_IMAGE_TOOL", "crane"),
            ("SSL_CERT_FILE", registry.cert_file().to_str().unwrap()),
        ],
    );

    assert!(output.status.success());

    // Assert that we successfully create a lock
    let lock_contents = std::fs::read_to_string(&lockfile).unwrap();
    let parsed: toml::Value = toml::from_str(&lock_contents).unwrap();
    let kits = parsed
        .as_table()
        .unwrap()
        .get("kit")
        .unwrap()
        .as_array()
        .unwrap();

    assert_eq!(kits.len(), 1);
    let core_kit = kits[0].as_table().unwrap();
    assert_eq!(core_kit.get("name").unwrap().as_str().unwrap(), "core-kit");
    assert_eq!(core_kit.get("version").unwrap().as_str().unwrap(), "1.0.0");
    assert_eq!(
        core_kit.get("vendor").unwrap().as_str().unwrap(),
        "custom-vendor"
    );
    assert_eq!(
        core_kit.get("source").unwrap().as_str().unwrap(),
        "definitely-wont-resolve/core-kit:v1.0.0"
    );

    std::fs::remove_file(&lockfile).ok();
    std::fs::remove_file(&override_file).ok();
}

struct LocalKit;

impl LocalKit {
    fn build(registry: &KitRegistry) {
        let local_kit = test_projects_dir().join("local-kit");

        run_command(
            TWOLITER_PATH,
            [
                "update",
                "--project-path",
                local_kit.join("Twoliter.toml").to_str().unwrap(),
            ],
            [],
        );

        run_command(
            TWOLITER_PATH,
            [
                "fetch",
                "--project-path",
                local_kit.join("Twoliter.toml").to_str().unwrap(),
            ],
            [],
        );

        run_command(
            TWOLITER_PATH,
            [
                "build",
                "kit",
                "core-kit",
                "--project-path",
                local_kit.join("Twoliter.toml").to_str().unwrap(),
            ],
            [],
        );

        std::fs::write(local_kit.join("Infra.toml"), INFRA_TOML).unwrap();
        run_command(
            TWOLITER_PATH,
            [
                "publish",
                "kit",
                "--project-path",
                local_kit.join("Twoliter.toml").to_str().unwrap(),
                "core-kit",
                "bottlerocket",
                "core-kit-overridden",
            ],
            [("SSL_CERT_FILE", registry.cert_file().to_str().unwrap())],
        );
    }
}

impl Drop for LocalKit {
    fn drop(&mut self) {
        let local_kit = test_projects_dir().join("local-kit");
        std::fs::remove_file(local_kit.join("Twoliter.lock")).ok();
        std::fs::remove_file(local_kit.join("Infra.toml")).ok();
    }
}
