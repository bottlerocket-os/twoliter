//! Resolve an AMI to the build that produced it.

use crate::error::Result;
use aws_sdk_ec2::types::ArchitectureValues;
use aws_sdk_ec2::Client as Ec2Client;
use regex::Regex;
use serde::{Deserialize, Serialize};
use snafu::{ensure_whatever, whatever, OptionExt, ResultExt};
use std::sync::LazyLock;

const ROOT_DEVICE: &str = "/dev/xvda";
const DATA_DEVICE: &str = "/dev/xvdb";

/// Architectures supported by Bottlerocket builds, using their artifact names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Architecture {
    X86_64,
    Aarch64,
}

impl Architecture {
    /// Each architecture, its artifact name and its EC2 architecture.
    const ALL: [(Self, &'static str, ArchitectureValues); 2] = [
        (Self::X86_64, "x86_64", ArchitectureValues::X8664),
        (Self::Aarch64, "aarch64", ArchitectureValues::Arm64),
    ];

    fn parse(name: &str) -> Option<Self> {
        Self::ALL.iter().find(|a| a.1 == name).map(|a| a.0)
    }

    fn from_ec2(arch: &ArchitectureValues) -> Result<Self> {
        Self::ALL
            .iter()
            .find(|a| a.2 == *arch)
            .map(|a| a.0)
            .with_whatever_context(|| format!("unsupported EC2 architecture '{}'", arch.as_str()))
    }
}

impl std::fmt::Display for Architecture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = Self::ALL.iter().find(|a| a.0 == *self).expect("listed").1;
        f.write_str(name)
    }
}

/// Matches the default AMI name `<name>-<variant>-<arch>-v<version>-<build_id>` and the
/// snapshot description `<name>-<variant>-<arch>-<version>-<build_id>.img`.
static NAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?P<name>[A-Za-z0-9_]+)-(?P<variant>[A-Za-z0-9_.-]+)-(?P<arch>x86_64|aarch64)-v?(?P<version>\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?)-(?P<build_id>[0-9a-f]{8,40}(?:-dirty)?)(?:\.img)?$",
    )
    .expect("valid regex")
});

/// Build identity encoded in an AMI or snapshot name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildName {
    pub name: String,
    pub variant: String,
    pub arch: Architecture,
    pub version: String,
    pub build_id: String,
}

impl BuildName {
    /// Parse a name produced by the default Bottlerocket naming scheme.
    pub fn parse(s: &str) -> Option<Self> {
        let caps = NAME_RE.captures(s.trim())?;
        Some(Self {
            name: caps["name"].to_string(),
            variant: caps["variant"].to_string(),
            arch: Architecture::parse(&caps["arch"])?,
            version: caps["version"].to_string(),
            build_id: caps["build_id"].to_string(),
        })
    }

    pub fn is_dirty(&self) -> bool {
        self.build_id.ends_with("-dirty")
    }

    /// File name prefix used for the build's image artifacts.
    pub fn filename_prefix(&self) -> String {
        format!(
            "{}-{}-{}-{}-{}",
            self.name, self.variant, self.arch, self.version, self.build_id
        )
    }
}

/// Everything known about an AMI that is needed to reproduce it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmiInfo {
    pub ami_id: String,
    pub ami_name: String,
    pub name: String,
    pub variant: String,
    pub arch: Architecture,
    pub version: String,
    pub build_id: String,
    pub filename_prefix: String,
    pub root_snapshot: String,
    pub data_snapshot: Option<String>,
    pub owner: String,
    pub owned_by_caller: bool,
}

/// Find the snapshot backing `device` in the image's block device mappings.
fn snapshot_for(image: &aws_sdk_ec2::types::Image, device: &str) -> Option<String> {
    image
        .block_device_mappings()
        .iter()
        .find(|bdm| bdm.device_name() == Some(device))
        .and_then(|bdm| bdm.ebs())
        .and_then(|ebs| ebs.snapshot_id())
        .map(String::from)
}

/// The root and optional data snapshots of the standard Bottlerocket device layout.
fn snapshots(image: &aws_sdk_ec2::types::Image) -> Result<(String, Option<String>)> {
    let devices: Vec<_> = image
        .block_device_mappings()
        .iter()
        .map(|bdm| bdm.device_name().unwrap_or("<missing>"))
        .collect();
    ensure_whatever!(
        image.root_device_name() == Some(ROOT_DEVICE),
        "unsupported AMI root device '{}'; expected {ROOT_DEVICE} (optional data device \
         {DATA_DEVICE}); observed devices: {}",
        image.root_device_name().unwrap_or("<missing>"),
        devices.join(", ")
    );
    for (i, bdm) in image.block_device_mappings().iter().enumerate() {
        let device = bdm.device_name().unwrap_or("<missing>");
        ensure_whatever!(
            !devices[..i].contains(&device),
            "AMI has duplicate mappings for {device}"
        );
        if bdm.ebs().is_some() {
            ensure_whatever!(
                matches!(device, ROOT_DEVICE | DATA_DEVICE),
                "unsupported AMI EBS device '{device}'; expected {ROOT_DEVICE} and optional \
                 {DATA_DEVICE}; observed devices: {}",
                devices.join(", ")
            );
        }
        if matches!(device, ROOT_DEVICE | DATA_DEVICE) {
            ensure_whatever!(
                bdm.ebs().and_then(|ebs| ebs.snapshot_id()).is_some(),
                "AMI device {device} has no snapshot"
            );
        }
    }
    let root = snapshot_for(image, ROOT_DEVICE)
        .whatever_context(format!("AMI has no root snapshot at {ROOT_DEVICE}"))?;
    Ok((root, snapshot_for(image, DATA_DEVICE)))
}

/// Return the description of a snapshot.
async fn snapshot_description(ec2: &Ec2Client, snapshot_id: &str) -> Result<Option<String>> {
    let response = ec2
        .describe_snapshots()
        .snapshot_ids(snapshot_id)
        .send()
        .await
        .whatever_context(format!("failed to describe snapshot {snapshot_id}"))?;
    Ok(response
        .snapshots()
        .first()
        .and_then(|s| s.description())
        .map(String::from))
}

/// Match the build identity, from the AMI name as `from_name` parsed it or else
/// the root snapshot's description, and explain why it cannot be used for reproduction.
fn resolve_build_name(
    ami_id: &str,
    ami_name: &str,
    from_name: Option<BuildName>,
    description: Option<&str>,
    arch: Architecture,
) -> Result<BuildName> {
    let (parsed, source, matched_name) = match from_name {
        Some(parsed) => (parsed, "AMI name", ami_name),
        None => {
            match description.and_then(|name| BuildName::parse(name).map(|parsed| (parsed, name))) {
                Some((parsed, name)) => (parsed, "root snapshot description", name),
                None => {
                    whatever!(
                        "could not determine the build for AMI {ami_id}: neither AMI name \
                     '{ami_name}' nor root snapshot description '{}' matches the expected format \
                     '<name>-<variant>-<arch>-v<version>-<build_id>' (AMI) or \
                     '<name>-<variant>-<arch>-<version>-<build_id>.img' (snapshot). \
                     Architecture must be x86_64 or aarch64; version must be major.minor.patch \
                     with an optional prerelease; build ID must be 8-40 lowercase hexadecimal \
                     characters, optionally followed by '-dirty'",
                        description.unwrap_or("<missing>")
                    );
                }
            }
        }
    };

    ensure_whatever!(
        !parsed.is_dirty(),
        "cannot reproduce AMI {ami_id}: matched {source} '{matched_name}', but build ID '{}' \
         ends with '-dirty', indicating uncommitted source changes. Checking out commit '{}' \
         alone cannot reproduce those changes; use an AMI built from a clean working tree",
        parsed.build_id,
        parsed.build_id.trim_end_matches("-dirty")
    );
    ensure_whatever!(
        parsed.arch == arch,
        "cannot reproduce AMI {ami_id}: EC2 architecture '{arch}' does not match architecture \
         '{}' parsed from {source} '{matched_name}'",
        parsed.arch
    );
    Ok(parsed)
}

/// Look up the AMI and work out which build produced it.
pub async fn resolve(ec2: &Ec2Client, caller_account: &str, ami_id: &str) -> Result<AmiInfo> {
    let response = ec2
        .describe_images()
        .image_ids(ami_id)
        .send()
        .await
        .whatever_context(format!("failed to describe AMI {ami_id}"))?;
    let image = response
        .images()
        .first()
        .whatever_context(format!("AMI {ami_id} not found in this region"))?;

    let ami_name = image.name().unwrap_or_default().to_string();
    let owner = image.owner_id().unwrap_or_default().to_string();
    let ec2_arch = image
        .architecture()
        .whatever_context("AMI has no architecture")?;
    let arch = Architecture::from_ec2(ec2_arch)?;

    let (root_snapshot, data_snapshot) = snapshots(image)?;

    let from_name = BuildName::parse(&ami_name);
    let description = match from_name {
        None => snapshot_description(ec2, &root_snapshot).await?,
        Some(_) => None,
    };
    let parsed = resolve_build_name(ami_id, &ami_name, from_name, description.as_deref(), arch)?;

    Ok(AmiInfo {
        ami_id: ami_id.to_string(),
        ami_name,
        filename_prefix: parsed.filename_prefix(),
        name: parsed.name,
        variant: parsed.variant,
        arch,
        version: parsed.version,
        build_id: parsed.build_id,
        root_snapshot,
        data_snapshot,
        owned_by_caller: owner == caller_account,
        owner,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_build_names() {
        use Architecture::{Aarch64, X86_64};
        for (name, expected) in [
            (
                "bottlerocket-aws-k8s-1.32-nvidia-x86_64-v1.40.0-a1b2c3d4",
                Some(("aws-k8s-1.32-nvidia", X86_64, "1.40.0", "a1b2c3d4")),
            ),
            // A snapshot description.
            (
                "bottlerocket-aws-dev-aarch64-1.2.3-0123abcd.img",
                Some(("aws-dev", Aarch64, "1.2.3", "0123abcd")),
            ),
            (
                "bottlerocket-aws-ecs-2-x86_64-v1.2.3-0123abcd-dirty",
                Some(("aws-ecs-2", X86_64, "1.2.3", "0123abcd-dirty")),
            ),
            (
                "bottlerocket-aws-dev-x86_64-v1.2.3-rc.1-0123abcd",
                Some(("aws-dev", X86_64, "1.2.3-rc.1", "0123abcd")),
            ),
            ("my-custom-ami", None),
            ("bottlerocket-aws-dev-riscv64-v1.2.3-0123abcd", None),
            ("bottlerocket-aws-dev-x86_64-v1.2.3-XYZ", None),
            ("", None),
        ] {
            let parsed = BuildName::parse(name);
            let parsed = parsed.as_ref().map(|n| {
                assert_eq!(n.name, "bottlerocket");
                (
                    n.variant.as_str(),
                    n.arch,
                    n.version.as_str(),
                    n.build_id.as_str(),
                )
            });
            assert_eq!(parsed, expected, "{name}");
        }
        let n = BuildName::parse("bottlerocket-aws-dev-x86_64-v1.2.3-0123abcd").unwrap();
        assert_eq!(
            n.filename_prefix(),
            "bottlerocket-aws-dev-x86_64-1.2.3-0123abcd"
        );
    }

    fn image(root: Option<&str>, mappings: &[(&str, Option<&str>)]) -> aws_sdk_ec2::types::Image {
        use aws_sdk_ec2::types::{BlockDeviceMapping, EbsBlockDevice};
        aws_sdk_ec2::types::Image::builder()
            .set_root_device_name(root.map(String::from))
            .set_block_device_mappings(Some(
                mappings
                    .iter()
                    .map(|(device, snapshot)| {
                        BlockDeviceMapping::builder()
                            .device_name(*device)
                            .ebs(
                                EbsBlockDevice::builder()
                                    .set_snapshot_id(snapshot.map(String::from))
                                    .build(),
                            )
                            .build()
                    })
                    .collect(),
            ))
            .build()
    }

    #[test]
    fn snapshot_layouts() {
        let root = (ROOT_DEVICE, Some("snap-root"));
        let data = (DATA_DEVICE, Some("snap-data"));
        assert_eq!(
            snapshots(&image(Some(ROOT_DEVICE), &[root])).unwrap(),
            ("snap-root".into(), None)
        );
        assert_eq!(
            snapshots(&image(Some(ROOT_DEVICE), &[data, root])).unwrap(),
            ("snap-root".into(), Some("snap-data".into()))
        );
        for (root_device, mappings) in [
            (None, &[root][..]),
            (Some("/dev/sda"), &[root]),
            // An unknown EBS device.
            (Some(ROOT_DEVICE), &[root, ("/dev/sdb", Some("snap-data"))]),
            (Some(ROOT_DEVICE), &[]),
            (Some(ROOT_DEVICE), &[(ROOT_DEVICE, None)]),
            (Some(ROOT_DEVICE), &[root, (DATA_DEVICE, None)]),
            (Some(ROOT_DEVICE), &[root, (ROOT_DEVICE, Some("snap-2"))]),
        ] {
            let image = image(root_device, mappings);
            assert!(snapshots(&image).is_err(), "{mappings:?}");
        }
    }

    #[test]
    fn resolve_build_name_refuses_unusable_builds() {
        let resolve = |name: &str, description: Option<&str>| {
            let parsed = BuildName::parse(name);
            resolve_build_name("ami-test", name, parsed, description, Architecture::X86_64)
        };
        let dirty = "bottlerocket-aws-dev-x86_64-v1.2.3-0123abcd-dirty";
        assert!(BuildName::parse(dirty).unwrap().is_dirty());
        assert!(resolve(dirty, None).is_err());
        // A snapshot of another architecture.
        let aarch64 = "bottlerocket-aws-dev-aarch64-1.2.3-0123abcd.img";
        assert!(resolve("custom-ami", Some(aarch64)).is_err());
        assert!(resolve("custom-ami", Some("custom-snapshot")).is_err());
        assert!(resolve("bottlerocket-aws-dev-x86_64-v1.2.3-0123abcd", None).is_ok());
    }
}
