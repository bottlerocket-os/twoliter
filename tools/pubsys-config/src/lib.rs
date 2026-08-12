//! The config module owns the definition and loading process for our configuration sources.
pub mod vmware;

use crate::vmware::VmwareConfig;
use jiff::SignedDuration;
use log::info;
use parse_datetime::parse_offset;
use serde::{Deserialize, Deserializer, Serialize};
use snafu::{OptionExt, ResultExt};
use std::collections::{HashMap, VecDeque};
use std::convert::TryFrom;
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use url::Url;

/// Configuration needed to load and create repos
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InfraConfig {
    // Repo subcommand config
    pub repo: Option<HashMap<String, RepoConfig>>,

    // Config for AWS specific subcommands
    pub aws: Option<AwsConfig>,

    // Config for VMware specific subcommands
    pub vmware: Option<VmwareConfig>,

    // Config for container registries
    pub vendor: Option<HashMap<String, Vendor>>,

    // EIF (Nitro Enclave Image Format) signing config
    pub eif: Option<EifConfig>,
}

impl InfraConfig {
    /// Deserializes an InfraConfig from a given path
    pub fn from_path<P>(path: P) -> Result<Self>
    where
        P: AsRef<Path>,
    {
        let path = path.as_ref();
        let infra_config_str = fs::read_to_string(path).context(error::FileSnafu { path })?;
        toml::from_str(&infra_config_str).context(error::InvalidTomlSnafu { path })
    }

    /// Deserializes an InfraConfig from a Infra.lock file at a given path
    pub fn from_lock_path<P>(path: P) -> Result<Self>
    where
        P: AsRef<Path>,
    {
        let path = path.as_ref();
        let infra_config_str = fs::read_to_string(path).context(error::FileSnafu { path })?;
        serde_yaml::from_str(&infra_config_str).context(error::InvalidLockSnafu { path })
    }

    /// Deserializes an InfraConfig from a given path, if it exists, otherwise builds a default
    /// config
    pub fn from_path_or_default<P>(path: P) -> Result<Self>
    where
        P: AsRef<Path>,
    {
        if path.as_ref().exists() {
            Self::from_path(path)
        } else {
            Ok(Self::default())
        }
    }

    /// Deserializes an InfraConfig from Infra.lock, if it exists, otherwise uses Infra.toml
    /// If the default flag is true, will create a default config if Infra.toml doesn't exist
    pub fn from_path_or_lock(path: &Path, default: bool) -> Result<Self> {
        let lock_path = Self::compute_lock_path(path)?;
        if lock_path.exists() {
            info!("Found infra config at path: {}", lock_path.display());
            Self::from_lock_path(lock_path)
        } else if default {
            Self::from_path_or_default(path)
        } else {
            info!("Found infra config at path: {}", path.display());
            Self::from_path(path)
        }
    }

    /// Looks for a file named `Infra.lock` in the same directory as the file named by
    /// `infra_config_path`. Returns true if the `Infra.lock` file exists, or if `infra_config_path`
    /// exists. Returns an error if the directory of `infra_config_path` cannot be found.
    pub fn lock_or_infra_config_exists<P>(infra_config_path: P) -> Result<bool>
    where
        P: AsRef<Path>,
    {
        let lock_path = Self::compute_lock_path(&infra_config_path)?;
        Ok(lock_path.exists() || infra_config_path.as_ref().exists())
    }

    /// Returns the file path to a file named `Infra.lock` in the same directory as the file named
    /// by `infra_config_path`.
    pub fn compute_lock_path<P>(infra_config_path: P) -> Result<PathBuf>
    where
        P: AsRef<Path>,
    {
        Ok(infra_config_path
            .as_ref()
            .parent()
            .context(error::ParentSnafu {
                path: infra_config_path.as_ref(),
            })?
            .join("Infra.lock"))
    }
}

/// Container registry vendor
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq, Clone)]
pub struct Vendor {
    pub registry: String,
}

/// S3-specific TUF infrastructure configuration
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq, Clone)]
pub struct S3Config {
    pub region: Option<String>,
    #[serde(default)]
    pub s3_prefix: String,
    pub vpc_endpoint_id: Option<String>,
    pub stack_arn: Option<String>,
    pub bucket_name: Option<String>,
}

/// AWS-specific infrastructure configuration
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq, Clone)]
#[serde(deny_unknown_fields)]
pub struct AwsConfig {
    #[serde(default)]
    pub regions: VecDeque<String>,
    pub role: Option<String>,
    pub profile: Option<String>,
    #[serde(default)]
    pub region: HashMap<String, AwsRegionConfig>,
    pub ssm_prefix: Option<String>,
    pub s3: Option<HashMap<String, S3Config>>,
}

/// AWS region-specific configuration
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone)]
#[serde(deny_unknown_fields)]
pub struct AwsRegionConfig {
    pub role: Option<String>,
}

/// Location of signing keys
// These variant names are lowercase because they have to match the text in Infra.toml, and it's
// more common for TOML config to be lowercase.
#[allow(non_camel_case_types)]
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum SigningKeyConfig {
    file {
        path: PathBuf,
    },
    kms {
        key_id: Option<String>,
        #[serde(flatten)]
        config: Option<KMSKeyConfig>,
    },
    ssm {
        parameter: String,
    },
    env {
        var_name: String,
    },
}

/// AWS region-specific configuration
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
//#[serde(deny_unknown_fields)]
pub struct KMSKeyConfig {
    #[serde(default)]
    pub available_keys: HashMap<String, String>,
    pub key_alias: Option<String>,
    #[serde(default)]
    pub regions: VecDeque<String>,
    #[serde(default)]
    pub key_stack_arns: HashMap<String, String>,
}

impl TryFrom<SigningKeyConfig> for Url {
    type Error = ();
    fn try_from(key: SigningKeyConfig) -> std::result::Result<Self, Self::Error> {
        match key {
            SigningKeyConfig::file { path } => Url::from_file_path(path),
            // We don't support passing profiles to tough in the name of the key/parameter, so for
            // KMS and SSM we prepend a slash if there isn't one present.
            SigningKeyConfig::kms { key_id, .. } => {
                let mut key_id = key_id.unwrap_or_default();
                key_id = if key_id.starts_with('/') {
                    key_id.to_string()
                } else {
                    format!("/{key_id}")
                };
                Url::parse(&format!("aws-kms://{key_id}")).map_err(|_| ())
            }
            SigningKeyConfig::ssm { parameter } => {
                let parameter = if parameter.starts_with('/') {
                    parameter
                } else {
                    format!("/{parameter}")
                };
                Url::parse(&format!("aws-ssm://{parameter}")).map_err(|_| ())
            }
            SigningKeyConfig::env { var_name } => {
                Url::parse(&format!("env://{var_name}")).map_err(|_| ())
            }
        }
    }
}

/// EIF (Nitro Enclave Image Format) signing configuration.
///
/// Signs an EIF's PCR0 with a dedicated cert/key pair. Independent of
/// `signing_keys` (TUF metadata) and `sbkeys` (UEFI Secure Boot).
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq, Clone)]
#[serde(deny_unknown_fields)]
pub struct EifConfig {
    /// PEM-encoded X.509 cert whose public key matches `signing_key`.
    /// Embedded in the EIF's COSE_Sign1 signature section.
    pub signing_cert: PathBuf,
    /// Signing key backend: local PEM file or AWS KMS.
    pub signing_key: EifSigningKey,
}

/// Backend for the EIF signing key. Only `file` and `kms` are supported;
/// these map to `eif-builder`'s `--signing-key` and `--kms-key-id` flags.
#[allow(non_camel_case_types)]
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum EifSigningKey {
    file {
        path: PathBuf,
    },
    kms {
        key_id: String,
        /// AWS region for the KMS `Sign` call. When `None`, the KMS backend
        /// resolves the region from the ambient AWS SDK chain (env, profile, IMDS).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<String>,
    },
}

impl Default for EifSigningKey {
    // Placeholder to satisfy `EifConfig: Default`; never observed via serde.
    fn default() -> Self {
        EifSigningKey::file {
            path: PathBuf::new(),
        }
    }
}

/// Represents a Bottlerocket repo's location and the metadata needed to update the repo
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepoConfig {
    pub root_role_url: Option<Url>,
    pub root_role_sha512: Option<String>,
    pub signing_keys: Option<SigningKeyConfig>,
    pub root_keys: Option<SigningKeyConfig>,
    pub metadata_base_url: Option<Url>,
    pub targets_url: Option<Url>,
    pub file_hosting_config_name: Option<String>,
    pub root_key_threshold: Option<NonZeroUsize>,
    pub pub_key_threshold: Option<NonZeroUsize>,
}

/// How long it takes for each metadata type to expire
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoExpirationPolicy {
    #[serde(deserialize_with = "deserialize_offset")]
    pub snapshot_expiration: SignedDuration,
    #[serde(deserialize_with = "deserialize_offset")]
    pub targets_expiration: SignedDuration,
    #[serde(deserialize_with = "deserialize_offset")]
    pub timestamp_expiration: SignedDuration,
}

impl RepoExpirationPolicy {
    /// Deserializes a RepoExpirationPolicy from a given path
    pub fn from_path<P>(path: P) -> Result<RepoExpirationPolicy>
    where
        P: AsRef<Path>,
    {
        let path = path.as_ref();
        let expiration_str = fs::read_to_string(path).context(error::FileSnafu { path })?;
        toml::from_str(&expiration_str).context(error::InvalidTomlSnafu { path })
    }
}

/// Deserializes a Duration in the form of "in X hours/days/weeks"
fn deserialize_offset<'de, D>(deserializer: D) -> std::result::Result<SignedDuration, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    parse_offset(&s).map_err(serde::de::Error::custom)
}

mod error {
    use snafu::Snafu;
    use std::io;
    use std::path::PathBuf;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub(super)))]
    pub enum Error {
        #[snafu(display("Failed to read '{}': {}", path.display(), source))]
        File { path: PathBuf, source: io::Error },

        #[snafu(display("Invalid config file at '{}': {}", path.display(), source))]
        InvalidToml {
            path: PathBuf,
            source: toml::de::Error,
        },

        #[snafu(display("Invalid lock file at '{}': {}", path.display(), source))]
        InvalidLock {
            path: PathBuf,
            source: serde_yaml::Error,
        },

        #[snafu(display("Missing config: {}", what))]
        MissingConfig { what: String },

        #[snafu(display("Failed to get parent of path: {}", path.display()))]
        Parent { path: PathBuf },
    }
}
pub use error::Error;
pub type Result<T> = std::result::Result<T, error::Error>;

#[test]
fn eif_config_file_backend_parses() {
    let toml_str = r#"
[eif]
signing_cert = "/opt/eif/signing.crt"
signing_key = { file = { path = "/opt/eif/signing.key" } }
"#;
    let cfg: InfraConfig = toml::from_str(toml_str).unwrap();
    let eif = cfg.eif.expect("eif section missing");
    assert_eq!(eif.signing_cert, PathBuf::from("/opt/eif/signing.crt"));
    match eif.signing_key {
        EifSigningKey::file { path } => {
            assert_eq!(path, PathBuf::from("/opt/eif/signing.key"))
        }
        _ => panic!("expected file backend"),
    }
}

#[test]
fn eif_config_kms_backend_parses() {
    let toml_str = r#"
[eif]
signing_cert = "/opt/eif/signing.crt"
signing_key = { kms = { key_id = "alias/my-eif-key" } }
"#;
    let cfg: InfraConfig = toml::from_str(toml_str).unwrap();
    let eif = cfg.eif.expect("eif section missing");
    match eif.signing_key {
        EifSigningKey::kms { key_id, region } => {
            assert_eq!(key_id, "alias/my-eif-key");
            assert_eq!(region, None);
        }
        _ => panic!("expected kms backend"),
    }
}

#[test]
fn eif_config_kms_with_region_parses() {
    let toml_str = r#"
[eif]
signing_cert = "/opt/eif/signing.crt"
signing_key = { kms = { key_id = "alias/my-eif-key", region = "us-west-2" } }
"#;
    let cfg: InfraConfig = toml::from_str(toml_str).unwrap();
    let eif = cfg.eif.expect("eif section missing");
    match eif.signing_key {
        EifSigningKey::kms { key_id, region } => {
            assert_eq!(key_id, "alias/my-eif-key");
            assert_eq!(region.as_deref(), Some("us-west-2"));
        }
        _ => panic!("expected kms backend"),
    }
}

#[test]
fn eif_config_absent_is_ok() {
    // An Infra.toml with no [eif] must still parse (unsigned-EIF path).
    let toml_str = r#"
[aws]
regions = ["us-west-2"]
"#;
    let cfg: InfraConfig = toml::from_str(toml_str).unwrap();
    assert!(cfg.eif.is_none());
}

#[test]
fn repo_expiration_deserialization_test() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("pubsys")
        .join("policies")
        .join("repo-expiration")
        .join("2w-2w-1w.toml");
    let _ = RepoExpirationPolicy::from_path(path).unwrap();
}
