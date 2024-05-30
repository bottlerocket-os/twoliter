use crate::test::copy_project_to_temp_dir;
use assert_cmd::Command;
use std::path::Path;

const PROJECT: &str = "local-kit";

fn expect_kit(project_dir: &Path, name: &str, arch: &str, packages: &[&str]) {
    let build = project_dir.join("build");
    let kit_output_dir = build.join("kits").join(name).join(arch).join("Packages");
    assert!(
        kit_output_dir.is_dir(),
        "Expected to find output dir for {} at {}",
        name,
        kit_output_dir.display()
    );

    for package in packages {
        let rpm = kit_output_dir.join(&format!("bottlerocket-{package}-0.0-0.{arch}.rpm"));
        assert!(
            rpm.is_file(),
            "Expected to find RPM for {}, for {} at {}",
            package,
            name,
            rpm.display()
        );
    }
}

#[tokio::test]
async fn build_core_kit() {
    let kit_name = "core-kit";
    let arch = "aarch64";
    let temp_dir = copy_project_to_temp_dir(PROJECT);
    let project_dir = temp_dir.path();
    let mut cmd = Command::cargo_bin("twoliter").unwrap();
    let assert = cmd
        .arg("build")
        .arg("kit")
        .arg(kit_name)
        .arg("--project-path")
        .arg(&project_dir.join("Twoliter.toml").display().to_string())
        .arg("--arch")
        .arg(arch)
        .assert();

    assert.success();

    expect_kit(&project_dir, "core-kit", arch, &["pkg-a"]);
}

#[tokio::test]
async fn build_extra_1_kit() {
    let kit_name = "extra-1-kit";
    let arch = "x86_64";
    let temp_dir = copy_project_to_temp_dir(PROJECT);
    let project_dir = temp_dir.path();
    let mut cmd = Command::cargo_bin("twoliter").unwrap();
    let assert = cmd
        .arg("build")
        .arg("kit")
        .arg(kit_name)
        .arg("--project-path")
        .arg(&project_dir.join("Twoliter.toml").display().to_string())
        .arg("--arch")
        .arg(arch)
        .assert();

    assert.success();

    expect_kit(&project_dir, "core-kit", arch, &["pkg-a"]);
    expect_kit(&project_dir, "extra-1-kit", arch, &["pkg-b", "pkg-d"]);
}

#[tokio::test]
async fn build_extra_2_kit() {
    let kit_name = "extra-2-kit";
    let arch = "aarch64";
    let temp_dir = copy_project_to_temp_dir(PROJECT);
    let project_dir = temp_dir.path();
    let mut cmd = Command::cargo_bin("twoliter").unwrap();
    let assert = cmd
        .arg("build")
        .arg("kit")
        .arg(kit_name)
        .arg("--project-path")
        .arg(&project_dir.join("Twoliter.toml").display().to_string())
        .arg("--arch")
        .arg(arch)
        .assert();

    assert.success();

    expect_kit(&project_dir, "core-kit", arch, &["pkg-a"]);
    expect_kit(&project_dir, "extra-2-kit", arch, &["pkg-c"]);
}

#[tokio::test]
async fn build_extra_3_kit() {
    let kit_name = "extra-3-kit";
    let arch = "x86_64";
    let temp_dir = copy_project_to_temp_dir(PROJECT);
    let project_dir = temp_dir.path();
    let mut cmd = Command::cargo_bin("twoliter").unwrap();
    let assert = cmd
        .arg("build")
        .arg("kit")
        .arg(kit_name)
        .arg("--project-path")
        .arg(&project_dir.join("Twoliter.toml").display().to_string())
        .arg("--arch")
        .arg(arch)
        .assert();

    assert.success();

    expect_kit(&project_dir, "core-kit", arch, &["pkg-a"]);
    expect_kit(&project_dir, "extra-1-kit", arch, &["pkg-b", "pkg-d"]);
    expect_kit(&project_dir, "extra-2-kit", arch, &["pkg-c"]);
    expect_kit(
        &project_dir,
        "extra-3-kit",
        arch,
        &["pkg-e", "pkg-f", "pkg-g"],
    );
}
