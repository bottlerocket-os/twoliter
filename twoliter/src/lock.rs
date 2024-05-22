use crate::schema_version::SchemaVersion;
use semver::Version;
use serde::{Deserialize, Serialize};

/// Represents the structure of a `Twoliter.lock` lock file.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Lock {
    /// The version of the Twoliter.toml this was generated from
    pub schema_version: SchemaVersion<1>,
    /// The workspace release version
    pub release_version: String,
    /// The resolved bottlerocket sdk
    pub sdk: LockedImage,
    /// Resolved kit dependencies
    pub kit: Vec<LockedImage>,
}

/// Represents a locked dependency on an image
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub(crate) struct LockedImage {
    /// The name of the dependency
    pub name: String,
    /// The version of the dependency
    pub version: Version,
    /// The vendor this dependency came from
    pub vendor: String,
    /// The resolved image uri of the dependency
    pub source: String,
}
