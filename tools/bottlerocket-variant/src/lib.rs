/*!
This library provides a structure for representing a Bottlerocket variant as well as functionality
useful in build scripts and other tooling that is variant-aware.
*/

use error::Error;
use serde::de::Error as SerdeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use snafu::{ensure, OptionExt, ResultExt};
use std::borrow::Borrow;
use std::convert::TryFrom;
use std::fmt::{Display, Formatter};
use std::ops::Deref;
use std::str::FromStr;

/// The name of the environment variable that tells us the current variant. Variant-sensitive crates
/// will need to be rebuilt if this changes. `Makefile.toml` emits the variant string in the
/// `BUILDSYS_VARIANT` environment variable. This is then passed to crate builds by the `Dockerfile`
/// as `VARIANT`.
pub const VARIANT_ENV: &str = "VARIANT";

/// The default `variant_version`. If the third position of a variant string tuple does not exist,
/// then the `variant_version` is `"undefined"`.
pub const DEFAULT_VARIANT_VERSION: &str = "0";

/// The default `variant_flavor`. If the fourth position of a variant string tuple does not exist,
/// then the variant_flavor cfg will be `"none"`.
pub const DEFAULT_VARIANT_FLAVOR: &str = "none";

pub type Result<T> = std::result::Result<T, error::Error>;

pub mod error {
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub(super)))]
    pub enum Error {
        #[snafu(display(
            "The 'VARIANT' environment variable is missing or unable to be read: {}",
            source
        ))]
        VariantEnv { source: std::env::VarError },

        #[snafu(display("The '{}' segment of the variant '{}' is missing", part_name, variant))]
        VariantPart { part_name: String, variant: String },

        #[snafu(display("The '{}' segment of the variant '{}' is empty", part_name, variant))]
        VariantPartEmpty { part_name: String, variant: String },
    }
}

/// # Variant
///
/// Represents a Bottlerocket variant string. These are in the form
/// `platform-runtime-[variant_version][-variant_flavor]`.
///
/// For example, here are some valid variant strings:
/// - aws-ecs-1
/// - vmware-k8s-1.32
/// - metal-dev
/// - aws-k8s-1.32-nvidia
///
/// The `platform` and `runtime` values are required. `variant_version` and `variant_flavor` values
/// are optional and will default to `"0"` and `"none"` respectively.
///
/// In a `build.rs` file, you may use the function `emit_cfgs()` if you need to conditionally
/// compile code based on variant characteristics.
///
/// # Example
///
/// ```rust
/// use bottlerocket_variant::{Variant, VARIANT_ENV};
/// std::env::set_var(VARIANT_ENV, "vmware-k8s-1.32");
/// let variant = Variant::from_env().unwrap();
///
/// assert_eq!(variant.version().unwrap(), "1.32");
///
/// // In a `build.rs` file, you may want to emit cfgs that you can use for conditional compilation.
/// variant.emit_cfgs();
/// ```
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Variant {
    variant: String,
    platform: String,
    runtime: String,
    family: String,
    version: Option<String>,
    variant_flavor: Option<String>,
}

impl Variant {
    /// Create a new `Variant` from a dash-delimited string. The first two tuple positions,
    /// `platform` and `runtime` are required. The next two, representing `variant_version` and
    /// `variant_flavor`, are optional.
    ///
    /// # Valid Values
    ///
    /// - `aws-dev`
    /// - `vmware-k8s-1.32`
    /// - `aws-k8s-1.32-nvidia`
    /// - `aws-k8s-1.32-nvidia-some-additional-ignored-tuple-positions`
    ///
    /// # Invalid Values
    ///
    /// - `aws`
    /// - `aws-dev-`
    ///
    /// # Example
    ///
    /// ```rust
    /// use bottlerocket_variant::Variant;
    /// let variant = Variant::new("aws-k8s").unwrap();
    /// assert_eq!(variant.family(), "aws-k8s");
    /// ```
    pub fn new<S: Into<String>>(value: S) -> Result<Self> {
        Self::parse(value)
    }

    /// Create a new `Variant` from the `VARIANT` environment variable's value. The environment
    /// variable must exist and its value must be a valid variant string tuple.
    pub fn from_env() -> Result<Self> {
        let value = std::env::var(VARIANT_ENV).context(error::VariantEnvSnafu)?;
        Variant::new(value)
    }

    /// The variant's platform. This is the first member of the tuple. For example, in `vmware-dev`,
    /// `vmware` is the platform.
    pub fn platform(&self) -> &str {
        &self.platform
    }

    /// The variant's runtime. This is the second member of the tuple. For example, in
    /// `vmware-k8s-1.32`, `k8s` is the `runtime`.
    pub fn runtime(&self) -> &str {
        &self.runtime
    }

    /// The variant's family. This is the `platform` and `runtime` together. For example, in
    /// `aws-k8s-1.32`, `aws-k8s` is the `family`.
    pub fn family(&self) -> &str {
        &self.family
    }

    /// The variant's version. This is the optional third value in the variant string tuple. For
    /// example for `aws-ecs-1` the `version` is `1`. If the `version` does not exist,
    /// [`DEFAULT_VARIANT_VERSION`] is returned.
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// The variant's flavor. This is the optional fourth value in the variant string tuple. For
    /// example for `aws-k8s-1.32-nvidia` the `variant_flavor` is `nvidia`.
    pub fn variant_flavor(&self) -> Option<&str> {
        self.variant_flavor.as_deref()
    }

    /// This can be used in a `build.rs` file to tell cargo that the crate needs to be rebuilt if
    /// the variant changes.
    pub fn rerun_if_changed() {
        println!("cargo:rerun-if-env-changed={VARIANT_ENV}");
    }

    /// This can be used in a `build.rs` file to emit `cfg` values that can be used for conditional
    /// compilation based on variant characteristics. This function also emits rerun-if-changed so
    /// that variant-sensitive builds will rebuild if the variant changes.
    ///
    /// # Example
    ///
    /// Given a variant `aws-k8s-1.32`, if this function has been called in `build.rs`, then
    /// all of the following conditional complition checks would evaluate to `true`.
    ///
    /// `#[cfg(variant = "aws-k8s-1.32")]`
    /// `#[cfg(variant_platform = "aws")]`
    /// `#[cfg(variant_runtime = "k8s")]`
    /// `#[cfg(variant_family = "aws-k8s")]`
    /// `#[cfg(variant_version = "1.32")]`
    /// `#[cfg(variant_flavor = "none")]`
    pub fn emit_cfgs(&self) {
        Self::rerun_if_changed();
        println!("cargo:rustc-cfg=variant=\"{self}\"");
        println!("cargo:rustc-cfg=variant_platform=\"{}\"", self.platform());
        println!("cargo:rustc-cfg=variant_runtime=\"{}\"", self.runtime());
        println!("cargo:rustc-cfg=variant_family=\"{}\"", self.family());
        println!(
            "cargo:rustc-cfg=variant_version=\"{}\"",
            self.version().unwrap_or(DEFAULT_VARIANT_VERSION)
        );
        println!(
            "cargo:rustc-cfg=variant_flavor=\"{}\"",
            self.variant_flavor().unwrap_or(DEFAULT_VARIANT_FLAVOR)
        );
    }

    /// Apply overrides to create a new Variant with the specified attributes replaced.
    ///
    /// If an override is `Some`, it replaces the original value. If `None`, the original is kept.
    /// Family is recomputed from final platform/runtime UNLESS family was explicitly overridden.
    ///
    /// The variant identity string (i.e. its name as used by `Display`, `AsRef<str>`, etc.) is
    /// always preserved from the original variant.  Overrides only affect the individual attribute
    /// accessors (`platform()`, `runtime()`, `family()`, `version()`, `variant_flavor()`).  This
    /// is important because the identity string is used for the `bottlerocket-variant(name)` RPM
    /// Provides/Requires contract; rewriting it from overridden attributes breaks that contract.
    pub fn with_overrides(&self, overrides: &VariantOverrides) -> Result<Self> {
        let platform = overrides
            .platform
            .clone()
            .unwrap_or_else(|| self.platform.clone());
        ensure!(
            !platform.is_empty(),
            error::VariantPartEmptySnafu {
                part_name: "platform",
                variant: self.variant.clone()
            }
        );
        let runtime = overrides
            .runtime
            .clone()
            .unwrap_or_else(|| self.runtime.clone());
        ensure!(
            !runtime.is_empty(),
            error::VariantPartEmptySnafu {
                part_name: "runtime",
                variant: self.variant.clone()
            }
        );
        let family = overrides
            .family
            .clone()
            .unwrap_or_else(|| format!("{platform}-{runtime}"));
        ensure!(
            !family.is_empty(),
            error::VariantPartEmptySnafu {
                part_name: "family",
                variant: self.variant.clone()
            }
        );
        let version = overrides.version.clone().or_else(|| self.version.clone());
        if let Some(ref v) = version {
            ensure!(
                !v.is_empty(),
                error::VariantPartEmptySnafu {
                    part_name: "variant_version",
                    variant: self.variant.clone()
                }
            );
        }
        let variant_flavor = overrides
            .flavor
            .clone()
            .or_else(|| self.variant_flavor.clone());
        if let Some(ref f) = variant_flavor {
            ensure!(
                !f.is_empty(),
                error::VariantPartEmptySnafu {
                    part_name: "variant_flavor",
                    variant: self.variant.clone()
                }
            );
        }

        Ok(Self {
            variant: self.variant.clone(),
            platform,
            runtime,
            family,
            version,
            variant_flavor,
        })
    }

    fn parse<S: Into<String>>(value: S) -> Result<Self> {
        let variant = value.into();
        let mut parts = variant.split('-');
        let platform = parts
            .next()
            .with_context(|| error::VariantPartSnafu {
                part_name: "platform",
                variant: variant.clone(),
            })?
            .to_string();
        ensure!(
            !platform.is_empty(),
            error::VariantPartEmptySnafu {
                part_name: "platform",
                variant: variant.clone()
            }
        );
        let runtime = parts
            .next()
            .with_context(|| error::VariantPartSnafu {
                part_name: "runtime",
                variant: variant.clone(),
            })?
            .to_string();
        ensure!(
            !runtime.is_empty(),
            error::VariantPartEmptySnafu {
                part_name: "runtime",
                variant: variant.clone()
            }
        );
        let variant_family = format!("{platform}-{runtime}");
        let variant_version = parts.next().map(|s| s.to_string());
        if let Some(value) = variant_version.as_ref() {
            ensure!(
                !value.is_empty(),
                error::VariantPartEmptySnafu {
                    part_name: "variant_version",
                    variant: variant.clone()
                }
            );
        }
        let variant_flavor = parts.next().map(|s| s.to_string());
        if let Some(value) = variant_flavor.as_ref() {
            ensure!(
                !value.is_empty(),
                error::VariantPartEmptySnafu {
                    part_name: "variant_flavor",
                    variant: variant.clone()
                }
            );
        }
        Ok(Self {
            variant,
            platform,
            runtime,
            family: variant_family,
            version: variant_version,
            variant_flavor,
        })
    }
}

impl FromStr for Variant {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Variant::new(s)
    }
}

impl TryFrom<String> for Variant {
    type Error = Error;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        Variant::new(value)
    }
}

impl TryFrom<&str> for Variant {
    type Error = Error;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        Variant::new(value)
    }
}

impl Serialize for Variant {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.variant)
    }
}

impl<'de> Deserialize<'de> for Variant {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Variant, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Variant::new(value).map_err(|e| D::Error::custom(format!("Error parsing variant: {e}")))
    }
}

impl Deref for Variant {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.variant
    }
}

impl Borrow<String> for Variant {
    fn borrow(&self) -> &String {
        &self.variant
    }
}

impl Borrow<str> for Variant {
    fn borrow(&self) -> &str {
        &self.variant
    }
}

impl AsRef<str> for Variant {
    fn as_ref(&self) -> &str {
        &self.variant
    }
}

impl Display for Variant {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.variant, f)
    }
}

impl From<Variant> for String {
    fn from(x: Variant) -> Self {
        x.variant
    }
}

impl PartialEq<str> for Variant {
    fn eq(&self, other: &str) -> bool {
        self.variant == other
    }
}

impl PartialEq<String> for Variant {
    fn eq(&self, other: &String) -> bool {
        &self.variant == other
    }
}

impl PartialEq<&str> for Variant {
    fn eq(&self, other: &&str) -> bool {
        &self.variant == other
    }
}

impl PartialEq<Variant> for str {
    fn eq(&self, other: &Variant) -> bool {
        self == other.variant
    }
}

impl PartialEq<Variant> for String {
    fn eq(&self, other: &Variant) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<Variant> for &str {
    fn eq(&self, other: &Variant) -> bool {
        self == &other.variant
    }
}

#[test]
fn parse_ok() {
    struct Test {
        input: &'static str,
        platform: &'static str,
        runtime: &'static str,
        variant_family: &'static str,
        variant_version: Option<&'static str>,
        variant_flavor: Option<&'static str>,
    }

    let tests = vec![
        Test {
            input: "aws-k8s-1.21",
            platform: "aws",
            runtime: "k8s",
            variant_family: "aws-k8s",
            variant_version: Some("1.21"),
            variant_flavor: None,
        },
        Test {
            input: "metal-dev",
            platform: "metal",
            runtime: "dev",
            variant_family: "metal-dev",
            variant_version: None,
            variant_flavor: None,
        },
        Test {
            input: "aws-ecs-1",
            platform: "aws",
            runtime: "ecs",
            variant_family: "aws-ecs",
            variant_version: Some("1"),
            variant_flavor: None,
        },
        Test {
            input: "aws-k8s-1.32-nvidia-some-additional-ignored-tuple-positions",
            platform: "aws",
            runtime: "k8s",
            variant_family: "aws-k8s",
            variant_version: Some("1.32"),
            variant_flavor: Some("nvidia"),
        },
    ];

    for test in tests {
        let parsed = Variant::new(test.input).unwrap();
        assert_eq!(parsed, test.input);
        assert_eq!(test.input, parsed);
        assert_eq!(parsed.platform(), test.platform.to_string());
        assert_eq!(parsed.runtime(), test.runtime);
        assert_eq!(parsed.family(), test.variant_family);
        assert_eq!(parsed.version(), test.variant_version);
        assert_eq!(parsed.variant_flavor(), test.variant_flavor);
    }
}

#[test]
fn parse_err() {
    let tests = vec!["aws", "aws-", "aws-dev-", "aws-k8s-1.32-"];
    for test in tests {
        let result = Variant::new(test);
        assert!(
            result.is_err(),
            "Expected Variant::new(\"{}\") to return an error",
            test
        );
    }
}

/// Overrides for variant attributes that can be specified in a package's Cargo.toml.
///
/// These overrides are read from `[package.metadata.bottlerocket-variant]` and allow
/// packages to override the variant attributes derived from the variant string.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
pub struct VariantOverrides {
    pub platform: Option<String>,
    pub runtime: Option<String>,
    pub family: Option<String>,
    pub version: Option<String>,
    pub flavor: Option<String>,
}

impl VariantOverrides {
    /// Returns true if no overrides are set.
    pub fn is_empty(&self) -> bool {
        self.platform.is_none()
            && self.runtime.is_none()
            && self.family.is_none()
            && self.version.is_none()
            && self.flavor.is_none()
    }
}

#[cfg(test)]
mod with_overrides_tests {
    use super::*;

    #[test]
    fn test_with_overrides_platform_only() {
        let variant = Variant::new("aws-k8s-1.32").unwrap();
        let overrides = VariantOverrides {
            platform: Some("metal".to_string()),
            ..Default::default()
        };
        let result = variant.with_overrides(&overrides).unwrap();
        assert_eq!(result.platform(), "metal");
        assert_eq!(result.runtime(), "k8s");
        assert_eq!(result.family(), "metal-k8s");
        assert_eq!(result.version(), Some("1.32"));
    }

    #[test]
    fn test_with_overrides_runtime_only() {
        let variant = Variant::new("aws-k8s-1.32").unwrap();
        let overrides = VariantOverrides {
            runtime: Some("ecs".to_string()),
            ..Default::default()
        };
        let result = variant.with_overrides(&overrides).unwrap();
        assert_eq!(result.platform(), "aws");
        assert_eq!(result.runtime(), "ecs");
        assert_eq!(result.family(), "aws-ecs");
        assert_eq!(result.version(), Some("1.32"));
    }

    #[test]
    fn test_with_overrides_family_explicit() {
        let variant = Variant::new("aws-k8s-1.32").unwrap();
        let overrides = VariantOverrides {
            platform: Some("metal".to_string()),
            family: Some("custom-family".to_string()),
            ..Default::default()
        };
        let result = variant.with_overrides(&overrides).unwrap();
        assert_eq!(result.platform(), "metal");
        assert_eq!(result.runtime(), "k8s");
        assert_eq!(result.family(), "custom-family");
    }

    #[test]
    fn test_with_overrides_family_computed() {
        let variant = Variant::new("aws-k8s-1.32").unwrap();
        let overrides = VariantOverrides {
            platform: Some("vmware".to_string()),
            runtime: Some("dev".to_string()),
            ..Default::default()
        };
        let result = variant.with_overrides(&overrides).unwrap();
        assert_eq!(result.family(), "vmware-dev");
    }

    #[test]
    fn test_with_overrides_all_fields() {
        let variant = Variant::new("aws-k8s-1.32").unwrap();
        let overrides = VariantOverrides {
            platform: Some("metal".to_string()),
            runtime: Some("dev".to_string()),
            family: Some("custom".to_string()),
            version: Some("2.0".to_string()),
            flavor: Some("nvidia".to_string()),
        };
        let result = variant.with_overrides(&overrides).unwrap();
        assert_eq!(result.platform(), "metal");
        assert_eq!(result.runtime(), "dev");
        assert_eq!(result.family(), "custom");
        assert_eq!(result.version(), Some("2.0"));
        assert_eq!(result.variant_flavor(), Some("nvidia"));
    }

    #[test]
    fn test_with_overrides_none() {
        let variant = Variant::new("aws-k8s-1.32-nvidia").unwrap();
        let overrides = VariantOverrides::default();
        let result = variant.with_overrides(&overrides).unwrap();
        assert_eq!(result.platform(), "aws");
        assert_eq!(result.runtime(), "k8s");
        assert_eq!(result.family(), "aws-k8s");
        assert_eq!(result.version(), Some("1.32"));
        assert_eq!(result.variant_flavor(), Some("nvidia"));
    }

    #[test]
    fn test_existing_parsing_unchanged() {
        // Ensure original parsing still works correctly
        let variant = Variant::new("aws-k8s-1.32-nvidia").unwrap();
        assert_eq!(variant.platform(), "aws");
        assert_eq!(variant.runtime(), "k8s");
        assert_eq!(variant.family(), "aws-k8s");
        assert_eq!(variant.version(), Some("1.32"));
        assert_eq!(variant.variant_flavor(), Some("nvidia"));

        let variant2 = Variant::new("metal-dev").unwrap();
        assert_eq!(variant2.platform(), "metal");
        assert_eq!(variant2.runtime(), "dev");
        assert_eq!(variant2.family(), "metal-dev");
        assert_eq!(variant2.version(), None);
        assert_eq!(variant2.variant_flavor(), None);
    }

    // Identity-preservation: with_overrides must not rewrite the variant name.

    /// Scenario 1: multiple overrides that diverge from the parsed segments.
    #[test]
    fn test_with_overrides_preserves_identity_customer_scenario() {
        // "aws-dev-gpu" parses as platform=aws, runtime=dev, version=gpu
        let variant = Variant::new("aws-dev-gpu").unwrap();
        assert_eq!(variant.platform(), "aws");
        assert_eq!(variant.runtime(), "dev");
        assert_eq!(variant.version(), Some("gpu"));

        let overrides = VariantOverrides {
            platform: Some("aws".to_string()),
            runtime: Some("custom-runtime".to_string()),
            flavor: Some("gpu".to_string()),
            ..Default::default()
        };
        let result = variant.with_overrides(&overrides).unwrap();

        // Identity must be preserved, not reconstructed from overridden attributes.
        assert_eq!(
            result.as_ref(),
            "aws-dev-gpu",
            "variant identity must be preserved, not reconstructed from overridden attributes"
        );
        assert_eq!(
            result.to_string(),
            "aws-dev-gpu",
            "Display must reflect the preserved identity"
        );

        // Overridden attributes must still take effect.
        assert_eq!(result.platform(), "aws");
        assert_eq!(result.runtime(), "custom-runtime");
        assert_eq!(result.family(), "aws-custom-runtime");
        assert_eq!(result.version(), Some("gpu")); // inherited from original, not overridden
        assert_eq!(result.variant_flavor(), Some("gpu"));
    }

    /// Scenario 2: platform-only override on a non-standard name.
    #[test]
    fn test_with_overrides_preserves_identity_platform_only_override() {
        // "prod-1" parses as platform=prod, runtime=1
        let variant = Variant::new("prod-1").unwrap();
        assert_eq!(variant.platform(), "prod");
        assert_eq!(variant.runtime(), "1");
        assert_eq!(variant.version(), None);

        let overrides = VariantOverrides {
            platform: Some("aws".to_string()),
            ..Default::default()
        };
        let result = variant.with_overrides(&overrides).unwrap();

        assert_eq!(
            result.as_ref(),
            "prod-1",
            "overriding only platform must not rewrite identity from prod-1 to aws-1"
        );
        assert_eq!(result.to_string(), "prod-1");

        assert_eq!(result.platform(), "aws");
        assert_eq!(result.runtime(), "1");
        assert_eq!(result.family(), "aws-1");
        assert_eq!(result.version(), None);
        assert_eq!(result.variant_flavor(), None);
    }

    /// Scenario 3: flavor override duplicates an existing parsed segment.
    #[test]
    fn test_with_overrides_preserves_identity_flavor_adds_duplicate_segment() {
        // "aws-dev-gpu" parses version=gpu, flavor=None
        let variant = Variant::new("aws-dev-gpu").unwrap();
        assert_eq!(variant.platform(), "aws");
        assert_eq!(variant.runtime(), "dev");
        assert_eq!(variant.version(), Some("gpu"));
        assert_eq!(variant.variant_flavor(), None);

        let overrides = VariantOverrides {
            platform: Some("aws".to_string()),
            flavor: Some("gpu".to_string()),
            ..Default::default()
        };
        let result = variant.with_overrides(&overrides).unwrap();

        assert_eq!(
            result.as_ref(),
            "aws-dev-gpu",
            "overriding flavor to 'gpu' when version is already 'gpu' must not produce aws-dev-gpu-gpu"
        );
        assert_eq!(result.to_string(), "aws-dev-gpu");

        assert_eq!(result.platform(), "aws");
        assert_eq!(result.runtime(), "dev");
        assert_eq!(result.family(), "aws-dev");
        assert_eq!(result.version(), Some("gpu"));
        assert_eq!(result.variant_flavor(), Some("gpu"));
    }

    /// All identity accessors return the original name after overrides.
    #[test]
    fn test_with_overrides_preserves_identity_all_accessors() {
        let variant = Variant::new("aws-k8s-1.32").unwrap();
        let overrides = VariantOverrides {
            platform: Some("metal".to_string()),
            runtime: Some("dev".to_string()),
            ..Default::default()
        };
        let result = variant.with_overrides(&overrides).unwrap();

        assert_eq!(result.as_ref(), "aws-k8s-1.32");
        assert_eq!(result.to_string(), "aws-k8s-1.32");
        assert_eq!(&*result, "aws-k8s-1.32"); // Deref
        let owned: String = result.into();
        assert_eq!(owned, "aws-k8s-1.32"); // Into<String>
    }

    /// Empty overrides must not truncate trailing ignored segments.
    #[test]
    fn test_with_overrides_none_preserves_full_identity() {
        let input = "aws-k8s-1.32-nvidia-some-additional-ignored-tuple-positions";
        let variant = Variant::new(input).unwrap();
        let result = variant
            .with_overrides(&VariantOverrides::default())
            .unwrap();

        assert_eq!(
            result.as_ref(),
            input,
            "empty overrides must not truncate trailing variant string segments"
        );
    }

    /// PartialEq impls compare the preserved identity, not overridden attrs.
    #[test]
    fn test_with_overrides_preserves_identity_equality() {
        let variant = Variant::new("aws-dev-gpu").unwrap();
        let overrides = VariantOverrides {
            runtime: Some("ecs".to_string()),
            ..Default::default()
        };
        let result = variant.with_overrides(&overrides).unwrap();

        assert_eq!(result, "aws-dev-gpu");
        assert_eq!("aws-dev-gpu", result);
        assert_ne!(result, "aws-ecs-gpu");
    }

    /// Overriding every field still preserves the original name.
    #[test]
    fn test_with_overrides_all_fields_preserves_identity() {
        let variant = Variant::new("aws-k8s-1.32").unwrap();
        let overrides = VariantOverrides {
            platform: Some("metal".to_string()),
            runtime: Some("dev".to_string()),
            family: Some("custom".to_string()),
            version: Some("2.0".to_string()),
            flavor: Some("nvidia".to_string()),
        };
        let result = variant.with_overrides(&overrides).unwrap();

        assert_eq!(
            result.as_ref(),
            "aws-k8s-1.32",
            "even with every attribute overridden, the identity must stay as the original name"
        );

        // Attributes still reflect overrides.
        assert_eq!(result.platform(), "metal");
        assert_eq!(result.runtime(), "dev");
        assert_eq!(result.family(), "custom");
        assert_eq!(result.version(), Some("2.0"));
        assert_eq!(result.variant_flavor(), Some("nvidia"));
    }

    /// Two-segment variant with overrides preserves its identity.
    #[test]
    fn test_with_overrides_two_segment_preserves_identity() {
        let variant = Variant::new("metal-dev").unwrap();
        let overrides = VariantOverrides {
            platform: Some("aws".to_string()),
            runtime: Some("k8s".to_string()),
            version: Some("1.32".to_string()),
            ..Default::default()
        };
        let result = variant.with_overrides(&overrides).unwrap();

        assert_eq!(result.as_ref(), "metal-dev");
        assert_eq!(result.platform(), "aws");
        assert_eq!(result.runtime(), "k8s");
        assert_eq!(result.version(), Some("1.32"));
    }

    /// Borrow impls return the preserved identity.
    #[test]
    fn test_with_overrides_borrow_preserves_identity() {
        use std::borrow::Borrow;

        let variant = Variant::new("aws-dev-gpu").unwrap();
        let overrides = VariantOverrides {
            runtime: Some("ecs".to_string()),
            ..Default::default()
        };
        let result = variant.with_overrides(&overrides).unwrap();

        let s: &str = result.borrow();
        assert_eq!(s, "aws-dev-gpu");

        let s: &String = result.borrow();
        assert_eq!(s, "aws-dev-gpu");
    }

    // Empty override validation tests.

    #[test]
    fn test_with_overrides_rejects_empty_platform() {
        let variant = Variant::new("aws-dev").unwrap();
        let overrides = VariantOverrides {
            platform: Some(String::new()),
            ..Default::default()
        };
        assert!(variant.with_overrides(&overrides).is_err());
    }

    #[test]
    fn test_with_overrides_rejects_empty_runtime() {
        let variant = Variant::new("aws-dev").unwrap();
        let overrides = VariantOverrides {
            runtime: Some(String::new()),
            ..Default::default()
        };
        assert!(variant.with_overrides(&overrides).is_err());
    }

    #[test]
    fn test_with_overrides_rejects_empty_family() {
        let variant = Variant::new("aws-dev").unwrap();
        let overrides = VariantOverrides {
            family: Some(String::new()),
            ..Default::default()
        };
        assert!(variant.with_overrides(&overrides).is_err());
    }

    #[test]
    fn test_with_overrides_rejects_empty_version() {
        let variant = Variant::new("aws-dev").unwrap();
        let overrides = VariantOverrides {
            version: Some(String::new()),
            ..Default::default()
        };
        assert!(variant.with_overrides(&overrides).is_err());
    }

    #[test]
    fn test_with_overrides_rejects_empty_flavor() {
        let variant = Variant::new("aws-dev").unwrap();
        let overrides = VariantOverrides {
            flavor: Some(String::new()),
            ..Default::default()
        };
        assert!(variant.with_overrides(&overrides).is_err());
    }
}
