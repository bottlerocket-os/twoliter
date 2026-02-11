use crate::run_command;
use std::fs::{create_dir, create_dir_all, remove_dir, write};
use std::process::Output;
use tempfile::TempDir;

const ADVISORY_CHECKER_PATH: &str = env!("CARGO_BIN_FILE_ADVISORY_CHECKER");
const PACKAGES_DIR: &str = "packages";
const ADVISORIES_DIR: &str = "advisories";

fn create_spec(name: &str, version: &str) -> String {
    format!(
        r#"Name: {name}
Version: {version}
Release: 1
Summary: Test package
License: MIT

%description
Test package
"#
    )
}

fn create_advisory(id: &str, cve: &str, severity: &str, pkg: &str, version: &str) -> String {
    format!(
        r#"[advisory]
id = "{id}"
title = "Test Advisory"
cve = "{cve}"
severity = "{severity}"
description = "Test vulnerability"

[[advisory.products]]
package-name = "{pkg}"
patched-version = "{version}"
patched-epoch = "0"

[updateinfo]
issue-date = 2026-01-01
version = "1"
"#
    )
}

// This function creates a dummy kit. As input we provide packages as tuples
// (package name, version) and advisories as a tuple (advisory path, advisory toml).
// The output is the temporary directory where the kit is created.
fn create_test_kit(packages: &[(&str, &str)], advisories: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().unwrap();

    let packages_dir = dir.path().join(PACKAGES_DIR);
    let advisories_dir = dir.path().join(ADVISORIES_DIR);
    create_dir(&packages_dir).unwrap();
    create_dir(&advisories_dir).unwrap();

    for (name, version) in packages {
        let pkg_dir = packages_dir.join(name);
        create_dir_all(&pkg_dir).unwrap();
        let content = create_spec(name, version);
        write(pkg_dir.join("test.spec"), content).unwrap();
    }

    for (filename, content) in advisories {
        let path = advisories_dir.join(filename);
        create_dir_all(path.parent().unwrap()).unwrap();
        write(&path, content).unwrap();
    }

    dir
}

fn run_advisory_checker(kit: &TempDir) -> Output {
    run_command(
        ADVISORY_CHECKER_PATH,
        [
            "--packages-dir",
            kit.path().join(PACKAGES_DIR).to_str().unwrap(),
            "--advisories-dir",
            kit.path().join(ADVISORIES_DIR).to_str().unwrap(),
        ],
        [],
    )
}

#[test]
#[ignore]
fn test_missing_advisories_dir_succeeds() {
    // Given a kit with a package but no advisories directory
    let kit = create_test_kit(&[("testpkg", "1.0.0")], &[]);
    remove_dir(kit.path().join(ADVISORIES_DIR)).unwrap();

    // When we run the advisory checker
    let output = run_advisory_checker(&kit);

    // Then it succeeds and reports that advisory checks were skipped
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("skipping"));
}

#[test]
#[ignore]
fn test_empty_advisories_dir_succeeds() {
    // Given a kit with a package and an empty advisories directory
    let kit = create_test_kit(&[("testpkg", "1.0.0")], &[]);

    // When we run the advisory checker
    let output = run_advisory_checker(&kit);

    // Then it succeeds with no violations
    assert!(output.status.success());
}

#[test]
#[ignore]
fn test_ignores_non_toml_files() {
    // Given a kit whose advisories directory contains only non-toml files
    let kit = create_test_kit(&[("testpkg", "1.0.0")], &[("staging/.gitkeep", "")]);

    // When we run the advisory checker
    let output = run_advisory_checker(&kit);

    // Then it succeeds since non-toml files are ignored
    assert!(output.status.success());
}

#[test]
#[ignore]
fn test_advisory_violation_fails() {
    // Given a package at version 1.0.0 and an advisory requiring 2.0.0
    let advisory = create_advisory("BRSA-test123", "CVE-2024-12345", "high", "testpkg", "2.0.0");
    let kit = create_test_kit(
        &[("testpkg", "1.0.0")],
        &[("v01/BRSA-test.toml", &advisory)],
    );

    // When we run the advisory checker
    let output = run_advisory_checker(&kit);

    // Then it fails and reports the advisory violation
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Advisory violations found"));
}

#[test]
#[ignore]
fn test_advisory_satisfied_succeeds() {
    // Given a package at version 3.0.0 and an advisory requiring 2.0.0
    let advisory = create_advisory(
        "BRSA-test456",
        "CVE-2024-67890",
        "moderate",
        "testpkg",
        "2.0.0",
    );
    let kit = create_test_kit(
        &[("testpkg", "3.0.0")],
        &[("v01/BRSA-test.toml", &advisory)],
    );

    // When we run the advisory checker
    let output = run_advisory_checker(&kit);

    // Then it succeeds
    assert!(output.status.success());
}

#[test]
#[ignore]
fn test_advisory_for_removed_package_fails() {
    // Given an advisory for "testpkg" but only "otherpkg" exists in the kit
    let advisory = create_advisory(
        "BRSA-test789",
        "CVE-2024-11111",
        "critical",
        "testpkg",
        "2.0.0",
    );
    let kit = create_test_kit(
        &[("otherpkg", "1.0.0")],
        &[("v01/BRSA-test.toml", &advisory)],
    );

    // When we run the advisory checker
    let output = run_advisory_checker(&kit);

    // Then it fails because the advisory's package has no end-of-life marker
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("not end-of-life"));
}

#[test]
#[ignore]
fn test_parse_advisory_error_invalid_toml() {
    // Given a kit with an advisory file containing invalid TOML
    let kit = create_test_kit(
        &[("testpkg", "1.0.0")],
        &[("v01/BRSA-invalid.toml", "not valid toml {{{")],
    );

    // When we run the advisory checker
    let output = run_advisory_checker(&kit);

    // Then it fails with a parse error
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Failed to parse advisory"));
}
