/*!

These structs provide the CLI interface for buildsys which is called from Cargo.toml and accepts all
of its input arguments from environment variables.

!*/

use buildsys::manifest::SupportedArch;
use buildsys::BuildType;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use url::Url;

/// A list of environment variables and the type of build that should be rerun if that environment
/// variable changes. The build type is represented with bit flags so that we can easily list
/// multiple build types for a single variable. See `[BuildType]` and `[rerun_for_envs]` below to
/// see how this list is used.
const REBUILD_VARS: [(&str, u8); 16] = [
    ("BUILDSYS_ARCH", PACKAGE | KIT | VARIANT),
    ("BUILDSYS_CACERTS_BUNDLE_OVERRIDE", VARIANT),
    ("BUILDSYS_KITS_DIR", KIT),
    ("BUILDSYS_EXTERNAL_KITS_DIR", PACKAGE | KIT | VARIANT),
    ("BUILDSYS_NAME", VARIANT),
    ("BUILDSYS_OUTPUT_DIR", VARIANT),
    ("BUILDSYS_OUTPUT_GENERATION_ID", PACKAGE | KIT | VARIANT),
    ("BUILDSYS_PACKAGES_DIR", PACKAGE),
    ("BUILDSYS_PRETTY_NAME", VARIANT),
    ("BUILDSYS_ROOT_DIR", PACKAGE | KIT | VARIANT),
    ("BUILDSYS_STATE_DIR", PACKAGE | KIT | VARIANT),
    ("BUILDSYS_TIMESTAMP", VARIANT),
    ("BUILDSYS_VARIANT", VARIANT),
    ("BUILDSYS_VERSION_BUILD", KIT | VARIANT),
    ("BUILDSYS_VERSION_IMAGE", KIT | VARIANT),
    ("TLPRIVATE_SDK_IMAGE", PACKAGE | KIT | VARIANT),
];

/// A tool for building Bottlerocket images and artifacts.
#[derive(Debug, Parser)]
pub(crate) struct Buildsys {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Command {
    BuildPackage(Box<BuildPackageArgs>),
    BuildKit(Box<BuildKitArgs>),
    BuildVariant(Box<BuildVariantArgs>),
    RepackVariant(Box<RepackVariantArgs>),
}

impl Command {
    pub(crate) fn build_type(&self) -> BuildType {
        match self {
            Command::BuildPackage(_) => BuildType::Package,
            Command::BuildKit(_) => BuildType::Kit,
            Command::BuildVariant(_) => BuildType::Variant,
            Command::RepackVariant(_) => BuildType::Repack,
        }
    }
}

/// Arguments common to all subcommands.
#[derive(Debug, Parser)]
pub(crate) struct Common {
    #[arg(long, env = "BUILDSYS_ARCH")]
    pub(crate) arch: SupportedArch,

    #[arg(long, env = "BUILDSYS_CARGO_METADATA_PATH")]
    pub(crate) cargo_metadata_path: PathBuf,

    #[arg(long, env = "BUILDSYS_OUTPUT_DIR")]
    pub(crate) image_arch_variant_dir: PathBuf,

    #[arg(long, env = "BUILDSYS_ROOT_DIR")]
    pub(crate) root_dir: PathBuf,

    #[arg(long, env = "BUILDSYS_STATE_DIR")]
    pub(crate) state_dir: PathBuf,

    #[arg(long, env = "BUILDSYS_TIMESTAMP")]
    pub(crate) timestamp: String,

    #[arg(long, env = "BUILDSYS_VERSION_FULL")]
    pub(crate) version_full: String,

    #[arg(long, env = "CARGO_MANIFEST_DIR")]
    pub(crate) cargo_manifest_dir: PathBuf,

    #[arg(long, env = "TLPRIVATE_SDK_IMAGE")]
    pub(crate) sdk_image: String,

    #[arg(long, env = "TWOLITER_TOOLS_DIR")]
    pub(crate) tools_dir: PathBuf,

    /// cicd_hack is used to suppress builds from running after all the cargo-related metadata is
    /// emitted. This allows cargo to create a fresh crate, and assumes that the corresponding
    /// build artifacts are already present. It is intended for use in a CI/CD scenario where some
    /// other process populates the build directory from a cache. Other uses may lead to unexpected
    /// build failures that are difficult to troubleshoot.
    #[arg(long, env = "BUILDSYS_CICD_HACK")]
    pub(crate) cicd_hack: bool,
}

/// Build RPMs from a spec file and sources.
#[derive(Debug, Parser)]
pub(crate) struct BuildPackageArgs {
    #[arg(long, env = "BUILDSYS_PACKAGES_DIR")]
    pub(crate) packages_dir: PathBuf,

    /// version_build is used along with version_build_timestamp in setting the Release of a Package. The Release is
    /// set in the form "<timestamp of latest project commit>.<latest project commit short sha>.br1" in RPMs.
    /// The value defaults to the latest commit of a project.
    #[arg(long, env = "BUILDSYS_VERSION_BUILD")]
    pub(crate) version_build: String,

    /// version_build_timestamp is used along with version_build in setting the Release of a Package. The Release is
    /// set in the form "<timestamp of latest project commit>.<latest project commit short sha>.br1" in RPMs.
    /// The value defaults to the timestamp in Unix ms of the latest commit of a project.
    #[arg(long, env = "BUILDSYS_VERSION_BUILD_TIMESTAMP")]
    pub(crate) version_build_timestamp: String,

    #[arg(long, env = "BUILDSYS_SOURCES_DIR")]
    pub(crate) sources_dir: PathBuf,

    #[arg(long, env = "BUILDSYS_LOOKASIDE_CACHE")]
    pub(crate) lookaside_cache: Url,

    #[arg(long, env = "BUILDSYS_UPSTREAM_SOURCE_FALLBACK")]
    pub(crate) upstream_source_fallback: String,

    #[command(flatten)]
    pub(crate) common: Common,
}

/// Place the required RPMs into a kit (directory) and make a yum repo.
#[derive(Debug, Parser)]
pub(crate) struct BuildKitArgs {
    /// The directory where built RPMs go, e.g. build/rpms
    #[arg(long, env = "BUILDSYS_PACKAGES_DIR")]
    pub(crate) packages_dir: PathBuf,

    /// The directory where built kits go, e.g. build/kits
    #[arg(long, env = "BUILDSYS_KITS_DIR")]
    pub(crate) kits_dir: PathBuf,

    /// The directory where external kits are fetched, e.g. build/external-kits
    #[arg(long, env = "BUILDSYS_EXTERNAL_KITS_DIR")]
    pub(crate) external_kits_dir: PathBuf,

    /// Build id of the workspace
    #[arg(long, env = "BUILDSYS_VERSION_BUILD")]
    pub(crate) version_build: String,

    /// Version number for the workspace
    #[arg(long, env = "BUILDSYS_VERSION_IMAGE")]
    pub(crate) version_image: String,

    #[command(flatten)]
    pub(crate) common: Common,
}

/// Build filesystem and disk images from RPMs.
#[derive(Debug, Parser)]
pub(crate) struct BuildVariantArgs {
    #[arg(long, env = "BUILDSYS_NAME")]
    pub(crate) name: String,

    #[arg(long, env = "BUILDSYS_PRETTY_NAME")]
    pub(crate) pretty_name: String,

    #[arg(long, env = "BUILDSYS_VARIANT")]
    pub(crate) variant: String,

    #[arg(long, env = "BUILDSYS_VARIANT_PLATFORM")]
    pub(crate) variant_platform: String,

    #[arg(long, env = "BUILDSYS_VARIANT_RUNTIME")]
    pub(crate) variant_runtime: String,

    #[arg(long, env = "BUILDSYS_VARIANT_FAMILY")]
    pub(crate) variant_family: String,

    #[arg(long, env = "BUILDSYS_VARIANT_FLAVOR")]
    pub(crate) variant_flavor: String,

    #[arg(long, env = "BUILDSYS_VERSION_BUILD")]
    pub(crate) version_build: String,

    #[arg(long, env = "BUILDSYS_VERSION_IMAGE")]
    pub(crate) version_image: String,

    #[command(flatten)]
    pub(crate) common: Common,
}

/// Repack variant from prebuilt images.
#[derive(Debug, Parser)]
pub(crate) struct RepackVariantArgs {
    #[arg(long, env = "BUILDSYS_NAME")]
    pub(crate) name: String,

    #[arg(long, env = "BUILDSYS_VARIANT")]
    pub(crate) variant: String,

    #[arg(long, env = "BUILDSYS_VERSION_BUILD")]
    pub(crate) version_build: String,

    #[arg(long, env = "BUILDSYS_VERSION_IMAGE")]
    pub(crate) version_image: String,

    #[command(flatten)]
    pub(crate) common: Common,
}

/// Returns the environment variables that need to be watched for a given `[BuildType]`.
fn sensitive_env_vars(build_type: BuildFlags) -> impl Iterator<Item = &'static str> {
    REBUILD_VARS
        .into_iter()
        .filter(move |(_, flags)| build_type.includes(*flags))
        .map(|(var, _)| var)
}

/// Emits the cargo directives for the list of sensitive environment variables for a given
/// `[BuildType]`.
pub(crate) fn rerun_for_envs(build_type: BuildType) {
    let build_flags: BuildFlags = build_type.into();
    for var in sensitive_env_vars(build_flags) {
        println!("cargo:rerun-if-env-changed={}", var)
    }
}

/// The thing that buildsys is building. This is an internal representation that includes `u8` flags
/// to help us manage lists of environment variables and what types of build that need to be rebuilt
/// when they change.
#[repr(u8)]
#[derive(Debug, Clone, Copy)]
enum BuildFlags {
    Package = 0b00000001,
    Kit = 0b00000010,
    Variant = 0b00000100,
    Repack = 0b0001000,
}

impl From<BuildType> for BuildFlags {
    fn from(value: BuildType) -> Self {
        match value {
            BuildType::Package => BuildFlags::Package,
            BuildType::Kit => BuildFlags::Kit,
            BuildType::Variant => BuildFlags::Variant,
            BuildType::Repack => BuildFlags::Repack,
        }
    }
}

impl BuildFlags {
    fn includes(&self, flags: u8) -> bool {
        let this = *self as u8;
        let and = flags & this;
        and == this
    }
}

#[allow(dead_code)]
const REPACK: u8 = BuildFlags::Repack as u8;
const PACKAGE: u8 = BuildFlags::Package as u8;
const KIT: u8 = BuildFlags::Kit as u8;
const VARIANT: u8 = BuildFlags::Variant as u8;

#[test]
fn build_type_includes_test() {
    // true
    assert!(BuildFlags::Repack.includes(REPACK));
    assert!(BuildFlags::Package.includes(PACKAGE | VARIANT));
    assert!(BuildFlags::Variant.includes(VARIANT));
    assert!(BuildFlags::Variant.includes(VARIANT | PACKAGE));

    // false
    assert!(!BuildFlags::Repack.includes(PACKAGE | VARIANT));
    assert!(!BuildFlags::Package.includes(VARIANT));
    assert!(!BuildFlags::Variant.includes(PACKAGE));
    assert!(!BuildFlags::Variant.includes(32));
    assert!(!BuildFlags::Variant.includes(0));
}

#[test]
fn test_sensitive_env_vars_variant() {
    let list: Vec<&str> = sensitive_env_vars(BuildFlags::Variant).collect();
    assert!(list.contains(&"BUILDSYS_ARCH"));
    assert!(list.contains(&"BUILDSYS_VARIANT"));
    assert!(!list.contains(&"BUILDSYS_PACKAGES_DIR"));
}

#[test]
fn test_sensitive_env_vars_package() {
    let list: Vec<&str> = sensitive_env_vars(BuildFlags::Package).collect();
    assert!(list.contains(&"BUILDSYS_ARCH"));
    assert!(list.contains(&"BUILDSYS_PACKAGES_DIR"));
    assert!(!list.contains(&"BUILDSYS_VARIANT"));
}
