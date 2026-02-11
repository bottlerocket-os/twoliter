//! Validates rpmspec files against security advisories to ensure packages meet
//! minimum version requirements for known vulnerabilities.

mod error;
mod models;

use clap::Parser;
use error::{error::*, Result};
use models::{Advisory, Args, Epoch, PackageName, PackageVersionManifest, EV};
use rpm::rpm_evr_compare;
use snafu::{ensure, ResultExt};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::Command;

// This is the last time when we removed a package from the kits without an
// EOL advisory. We make sure that any advisory published post this date
// either has a corresponding package or an EOL advisory.
const LAST_UNTRACKED_PACKAGE_REMOVAL_DATE: &str = "2025-07-26";

fn command<I, S>(bin_path: &str, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new(bin_path);
    cmd.args(args);
    let output = cmd.output().context(ExecutionFailureSnafu)?;

    ensure!(
        output.status.success(),
        CommandFailureSnafu { bin_path, output }
    );

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn get_rpm_metadata(spec_path: &Path) -> Result<PackageVersionManifest> {
    let pkg_prefix = command("rpm", ["--eval", "%{_cross_os}"])?
        .trim()
        .to_string();
    let spec_path_str = spec_path.to_string_lossy();

    let rpm_metadata = command(
        "rpmspec",
        [
            "-q",
            "--qf",
            "%{Name}|%{Epoch}|%{Version}\n",
            &spec_path_str,
        ],
    )?;

    parse_rpm_metadata(pkg_prefix, rpm_metadata)
}

fn parse_rpm_metadata(pkg_prefix: String, rpm_metadata: String) -> Result<PackageVersionManifest> {
    let mut manifest = PackageVersionManifest::new();
    for metadata in rpm_metadata.lines() {
        let parts: Vec<&str> = metadata.split('|').collect();
        ensure!(
            parts.len() == 3,
            RpmSpecFormatSnafu {
                spec_output: metadata
            }
        );

        let name = parts[0].strip_prefix(&pkg_prefix).unwrap_or(parts[0]);
        let epoch_str = if parts[1] == "(none)" { "0" } else { parts[1] };
        let epoch = Epoch::try_new(epoch_str).expect("epoch should be valid integer");
        let version = parts[2];

        manifest.insert(PackageName::new(name), EV::new(&epoch, version));
    }

    Ok(manifest)
}

fn find_spec_file(package_dir: &Path) -> Option<std::path::PathBuf> {
    fs::read_dir(package_dir).ok()?.find_map(|entry| {
        let path = entry.ok()?.path();
        if path.extension() == Some(OsStr::new("spec")) {
            Some(path)
        } else {
            None
        }
    })
}

fn build_all_package_metadata(packages_dir: &Path) -> Result<PackageVersionManifest> {
    let mut manifest = PackageVersionManifest::new();
    if !packages_dir.exists() {
        return Ok(manifest);
    }

    for entry in fs::read_dir(packages_dir).context(ReadDirSnafu { path: packages_dir })? {
        let entry = entry.context(ReadDirEntrySnafu)?;
        if entry.path().is_dir() {
            // Inside the package directory, we extract metadata from the spec file
            if let Some(spec_path) = find_spec_file(&entry.path()) {
                let pkg_manifest = get_rpm_metadata(&spec_path)?;
                manifest.merge(pkg_manifest);
            }
        }
    }

    Ok(manifest)
}

fn collect_advisories(advisories_dir: &Path) -> Result<HashMap<PackageName, Vec<Advisory>>> {
    let mut advisories_by_product: HashMap<PackageName, Vec<Advisory>> = HashMap::new();

    if !advisories_dir.exists() {
        return Ok(advisories_by_product);
    }

    for entry in fs::read_dir(advisories_dir).context(ReadDirSnafu {
        path: advisories_dir,
    })? {
        let version_dir = entry.context(ReadDirEntrySnafu)?;
        if version_dir.path().is_dir() {
            // Inside the version directory, we read the advisories and store them
            for brsa in fs::read_dir(version_dir.path()).context(ReadDirSnafu {
                path: advisories_dir,
            })? {
                let brsa = brsa.context(ReadDirEntrySnafu)?;
                if brsa.path().extension() == Some(OsStr::new("toml")) {
                    let content = fs::read_to_string(brsa.path())
                        .context(ReadFileSnafu { path: brsa.path() })?;
                    let advisory: Advisory = toml::from_str(&content)
                        .context(ParseAdvisorySnafu { path: brsa.path() })?;
                    for product in &advisory.advisory_info.products {
                        advisories_by_product
                            .entry(product.package_name.clone())
                            .or_default()
                            .push(advisory.clone());
                    }
                }
            }
        }
    }

    Ok(advisories_by_product)
}

fn validate_advisories(advisories_dir: &Path, packages_dir: &Path) -> Result<()> {
    // We go through all the packages in package directory and create a map with
    // package as key and Epoch:Version as value. We create a separate map from
    // the advisories with package as key and an array of advisories as value.
    let pkg_metadata = build_all_package_metadata(packages_dir)?;
    let advisories_by_product = collect_advisories(advisories_dir)?;

    let mut violations = Vec::new();

    // For each advisory, we check if the corresponding package exists and the
    // package is patched or if its an end of life advisory.
    for (package_name, mut advisories) in advisories_by_product {
        advisories.sort_by(|a, b| a.update_info.cmp(&b.update_info));

        if let Some(spec_ev) = pkg_metadata.get(&package_name) {
            // Package exists - check version compliance
            for advisory in &advisories {
                for product in &advisory.advisory_info.products {
                    if product.package_name != package_name {
                        continue;
                    }
                    let advisory_ev = EV::new(&product.patched_epoch, &product.patched_version);
                    if rpm_evr_compare(&spec_ev.to_string(), &advisory_ev.to_string()).is_lt() {
                        violations.push(format!(
                            "BRSA ID: {}\n package name: {}\n package version: {spec_ev}\n min. expected version: {advisory_ev}",
                            advisory.advisory_info.id, package_name
                        ));
                    }
                }
            }
        } else {
            // Package missing - check end-of-life
            if let Some(latest) = advisories.last() {
                let issue_date = latest.update_info.issue_date.to_string();

                // If an advisory is created after LAST_UNTRACKED_PACKAGE_REMOVAL_DATE we make sure that
                // either a package exists in the kit for that advisory or there is an end of life
                // advisory. If this test fails, it indicates that there is a typo in the package name.
                if issue_date.as_str() > LAST_UNTRACKED_PACKAGE_REMOVAL_DATE
                    && !latest.advisory_info.end_of_life
                {
                    violations.push(format!(
                        "Advisory Product '{}' has no package and latest advisory '{}' is not end-of-life",
                        package_name, latest.advisory_info.id
                    ));
                }
            }
        }
    }

    if !violations.is_empty() {
        println!("Advisory violations found:\n{}", violations.join("\n"));
        return Err(error::Error::AdvisoryViolations { violations });
    }

    println!("All advisories validated. No errors were found.");
    Ok(())
}

#[snafu::report]
fn main() -> Result<()> {
    let args = Args::parse();
    if !args.advisories_dir.exists() || !args.advisories_dir.is_dir() {
        println!(
            "Advisories directory '{}' not found, skipping advisory check",
            args.advisories_dir.display()
        );
        return Ok(());
    }
    validate_advisories(&args.advisories_dir, &args.packages_dir)?;
    println!(
        "Advisory validation passed for {}",
        args.advisories_dir.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use models::CveId;
    use test_case::test_case;

    fn make_advisory_toml(id_field: &str, id_value: &str) -> String {
        format!(
            r#"
[advisory]
id = "TEST-001"
title = "Test Advisory"
severity = "high"
description = "Test description"
{id_field} = "{id_value}"
[[advisory.products]]
package-name = "test"
patched-version = "1.0"

[updateinfo]
issue-date = 2024-01-01
version = "1"
"#
        )
    }

    #[test_case("CVE-2024-1234"; "basic 4 digit")]
    #[test_case("CVE-2024-12345"; "5 digit")]
    #[test_case("CVE-2025-472688"; "6 digit")]
    #[test_case("CVE-1999-0001"; "old year")]
    fn valid_cve(cve: &str) {
        let toml = make_advisory_toml("cve", cve);
        assert!(
            toml::from_str::<Advisory>(&toml).is_ok(),
            "Failed to parse valid CVE: {cve}"
        );
    }

    #[test_case("CVE-2024-001"; "too few digits")]
    #[test_case("CVE-2024-INVALID"; "non numeric")]
    #[test_case("CVE-24-1234"; "year too short")]
    #[test_case("CVE-2024-0"; "single digit")]
    #[test_case("CVE–2024–1234"; "en dash")]
    #[test_case("CVE—2024—1234"; "em dash")]
    #[test_case("GHSA-xxxx-xxxx-xxxx"; "wrong format")]
    fn invalid_cve(cve: &str) {
        let toml = make_advisory_toml("cve", cve);
        assert!(
            toml::from_str::<Advisory>(&toml).is_err(),
            "Should reject invalid CVE: {cve}"
        );
    }

    #[test_case("GHSA-23fg-6c23-wxrv"; "standard")]
    #[test_case("GHSA-2222-3333-4444"; "all numeric")]
    #[test_case("GHSA-cfgh-jmpq-rvwx"; "all alpha")]
    fn valid_ghsa(ghsa: &str) {
        let toml = make_advisory_toml("ghsa", ghsa);
        assert!(
            toml::from_str::<Advisory>(&toml).is_ok(),
            "Failed to parse valid GHSA: {ghsa}"
        );
    }

    #[test_case("GHSA-123-456-789"; "too short")]
    #[test_case("GHSA-12345-67890-12345"; "too long")]
    #[test_case("CVE-2024-1234"; "wrong format")]
    fn invalid_ghsa(ghsa: &str) {
        let toml = make_advisory_toml("ghsa", ghsa);
        assert!(
            toml::from_str::<Advisory>(&toml).is_err(),
            "Should reject invalid GHSA: {ghsa}"
        );
    }

    #[test]
    fn complete_advisory() {
        let toml = make_advisory_toml("cve", "CVE-2025-12345");
        let advisory: Advisory = toml::from_str(&toml).expect("Failed to parse complete advisory");
        assert_eq!(advisory.advisory_info.id, "TEST-001");
        assert_eq!(
            advisory.advisory_info.cve,
            CveId::try_new("CVE-2025-12345").ok()
        );
        assert_eq!(advisory.advisory_info.products.len(), 1);
        assert_eq!(
            advisory.advisory_info.products[0].package_name,
            PackageName::new("test")
        );
        assert_eq!(
            advisory.advisory_info.products[0].patched_epoch,
            Epoch::try_new("0").unwrap()
        );
    }

    #[test_case("testpkg|0|1.0.0\n"; "Epoch present")]
    #[test_case("testpkg|(none)|1.0.0\n"; "Epoch not present")]
    #[test_case("testpkg|(none)|1.0.0\ntestpkg-bin|(none)|1.0.0\ntestpkg-lib|(none)|1.0.0\n"; "Multiple sub-packages")]
    fn parse_rpm_metadata_basic(rpm_metadata: &str) {
        let output = rpm_metadata.to_string();
        let result = parse_rpm_metadata("".to_string(), output).unwrap();
        let evr = result.get(&PackageName::new("testpkg")).unwrap();
        assert_eq!(evr.to_string(), "0:1.0.0");
    }

    #[test_case("testpkg\n"; "invalid output 1")]
    #[test_case("testpkg|(none)\n"; "invalid output 2")]
    fn parse_invalid_rpm_metadata(rpm_metadata: &str) {
        let result = parse_rpm_metadata("".to_string(), rpm_metadata.to_string());
        assert!(result.is_err());
    }

    #[test]
    fn parse_rpm_metadata_with_prefix() {
        let output = "bottlerocket-testpkg|1|3.0.0\n".to_string();
        let result = parse_rpm_metadata("bottlerocket-".to_string(), output).unwrap();
        let evr = result.get(&PackageName::new("testpkg")).unwrap();
        assert_eq!(evr.to_string(), "1:3.0.0");
    }

    #[test]
    fn parse_rpm_metadata_multiple_packages() {
        let output = "pkg1|0|1.0\npkg2|1|2.0\n".to_string();
        let result = parse_rpm_metadata("".to_string(), output).unwrap();

        assert_eq!(
            result.get(&PackageName::new("pkg1")).unwrap().to_string(),
            "0:1.0"
        );
        assert_eq!(
            result.get(&PackageName::new("pkg2")).unwrap().to_string(),
            "1:2.0"
        );
    }
}
