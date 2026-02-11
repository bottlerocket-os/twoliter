//! Advisory data models and deserialization logic.

use clap::Parser;
use nutype::nutype;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt::Display;
use std::path::PathBuf;
use std::sync::LazyLock;

const DEFAULT_EPOCH: &str = "0";

static CVE_ID_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^CVE-[0-9]{4}-[0-9]{4,19}$"#).expect("Failed to compile CVE_ID_REGEX")
});

static GHSA_ID_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^GHSA(-[23456789cfghjmpqrvwx]{4}){3}$").expect("Failed to compile GHSA_ID_REGEX")
});

fn is_integer(s: &str) -> bool {
    s.parse::<i64>().is_ok()
}

#[nutype(
    validate(predicate = is_integer),
    default = DEFAULT_EPOCH,
    derive(Debug, Clone, Deserialize, PartialEq, Display, AsRef, Default)
)]
pub struct Epoch(String);

/// Short for Epoch, Version. We don't use Release from EVR versioning scheme because every
/// package update aims at upgrading the version and BRSAs do not include Release field.
#[derive(Debug, Clone)]
pub struct EV {
    pub epoch: Epoch,
    pub version: String,
}

impl EV {
    pub fn new(epoch: &Epoch, version: impl Into<String>) -> Self {
        Self {
            epoch: epoch.clone(),
            version: version.into(),
        }
    }
}

impl Display for EV {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.epoch, self.version)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
pub struct PackageName(String);

impl Display for PackageName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl PackageName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

#[derive(Debug)]
pub struct PackageVersionManifest {
    packages: HashMap<PackageName, EV>,
}

impl PackageVersionManifest {
    pub fn new() -> Self {
        Self {
            packages: HashMap::new(),
        }
    }

    pub fn insert(&mut self, name: PackageName, evr: EV) {
        self.packages.insert(name, evr);
    }

    pub fn get(&self, name: &PackageName) -> Option<&EV> {
        self.packages.get(name)
    }

    pub fn merge(&mut self, other: PackageVersionManifest) {
        self.packages.extend(other.packages);
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Advisory {
    #[serde(rename = "advisory")]
    pub advisory_info: AdvisoryInfo,
    #[serde(rename = "updateinfo")]
    pub update_info: UpdateInfo,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[expect(dead_code)]
pub struct AdvisoryInfo {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub end_of_life: bool,
    pub cve: Option<CveId>,
    pub ghsa: Option<GhsaId>,
    pub severity: Severity,
    pub description: String,
    pub products: Vec<Product>,
}

#[nutype(
    validate(regex = CVE_ID_REGEX),
    derive(Debug, Clone, Deserialize, PartialEq)
)]
pub struct CveId(String);

#[nutype(
    validate(regex = GHSA_ID_REGEX),
    derive(Debug, Clone, Deserialize, PartialEq)
)]
pub struct GhsaId(String);

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct UpdateInfo {
    pub issue_date: toml::value::Datetime,
    pub version: String,
}

impl Ord for UpdateInfo {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.issue_date
            .to_string()
            .cmp(&other.issue_date.to_string())
    }
}

impl PartialOrd for UpdateInfo {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Low,
    #[serde(alias = "medium")]
    Moderate,
    High,
    Critical,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Product {
    pub package_name: PackageName,
    pub patched_version: String,
    #[serde(default = "default_epoch")]
    pub patched_epoch: Epoch,
}

fn default_epoch() -> Epoch {
    Epoch::default()
}

#[derive(Debug, Parser)]
#[clap(about, long_about = None, version)]
pub(crate) struct Args {
    #[arg(long)]
    pub(crate) advisories_dir: PathBuf,
    #[arg(long)]
    pub(crate) packages_dir: PathBuf,
}
