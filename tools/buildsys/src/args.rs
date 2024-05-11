/*!

These structs provide the CLI interface for buildsys which is called from Cargo.toml and accepts all
of its input arguments from environment variables.

!*/

use buildsys::manifest::SupportedArch;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use url::Url;

/// A list of environment variables and the type of build that should be rerun if that environment
/// variable changes. The build type is represented with bit flags so that we can easily list
/// multiple build types for a single variable. See `[BuildType]` and `[rerun_for_envs]` below to
/// see how this list is used.
const REBUILD_VARS: [(&str, u8); 12] = [
    ("BUILDSYS_ARCH", PACKAGE | VARIANT),
    ("BUILDSYS_NAME", VARIANT),
    ("BUILDSYS_OUTPUT_DIR", VARIANT),
    ("BUILDSYS_PACKAGES_DIR", PACKAGE),
    ("BUILDSYS_PRETTY_NAME", VARIANT),
    ("BUILDSYS_ROOT_DIR", PACKAGE | VARIANT),
    ("BUILDSYS_STATE_DIR", PACKAGE | VARIANT),
    ("BUILDSYS_TIMESTAMP", VARIANT),
    ("BUILDSYS_VARIANT", VARIANT),
    ("BUILDSYS_VERSION_BUILD", VARIANT),
    ("BUILDSYS_VERSION_IMAGE", VARIANT),
    ("TLPRIVATE_SDK_IMAGE", PACKAGE | VARIANT),
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
    BuildVariant(Box<BuildVariantArgs>),
    RepackVariant(Box<RepackVariantArgs>),
}

impl Command {
    pub(crate) fn build_type(&self) -> BuildType {
        match self {
            Command::BuildPackage(_) => BuildType::Package,
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
}

/// Build RPMs from a spec file and sources.
#[derive(Debug, Parser)]
pub(crate) struct BuildPackageArgs {
    #[arg(long, env = "BUILDSYS_PACKAGES_DIR")]
    pub(crate) packages_dir: PathBuf,

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

    #[arg(long, env = "PUBLISH_REPO")]
    pub(crate) publish_repo: String,

    #[arg(long, env = "BUILDSYS_SOURCES_DIR")]
    pub(crate) sources_dir: PathBuf,

    #[arg(long, env = "BUILDSYS_LOOKASIDE_CACHE")]
    pub(crate) lookaside_cache: Url,

    #[arg(long, env = "BUILDSYS_UPSTREAM_SOURCE_FALLBACK")]
    pub(crate) upstream_source_fallback: String,

    #[arg(long, env = "CARGO_PKG_NAME")]
    pub(crate) cargo_package_name: String,

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
fn sensitive_env_vars(build_type: BuildType) -> impl Iterator<Item = &'static str> {
    REBUILD_VARS
        .into_iter()
        .filter(move |(_, flags)| build_type.includes(*flags))
        .map(|(var, _)| var)
}

/// Emits the cargo directives for a the list of sensitive environment variables for a given
/// `[BuildType]`, unless the `[BuildType]` is Repack.
pub(crate) fn rerun_for_envs(build_type: BuildType) {
    match build_type {
        BuildType::Repack => (),
        _ => {
            for var in sensitive_env_vars(build_type) {
                println!("cargo:rerun-if-env-changed={}", var)
            }
        }
    }
}

/// The thing that buildsys is building.
#[repr(u8)]
#[derive(Debug, Clone, Copy)]
pub(crate) enum BuildType {
    Repack = 0b00000000,
    Package = 0b00000001,
    Variant = 0b00000010,
}

impl BuildType {
    fn includes(&self, flags: u8) -> bool {
        let this = *self as u8;
        let and = flags & this;
        and == this
    }
}

const PACKAGE: u8 = BuildType::Package as u8;
const VARIANT: u8 = BuildType::Variant as u8;

#[test]
fn build_type_includes_test() {
    // true
    assert!(BuildType::Repack.includes(0));
    assert!(BuildType::Package.includes(PACKAGE | VARIANT));
    assert!(BuildType::Variant.includes(VARIANT));
    assert!(BuildType::Variant.includes(VARIANT | PACKAGE));

    // false
    assert!(BuildType::Repack.includes(PACKAGE));
    assert!(BuildType::Repack.includes(VARIANT));
    assert!(!BuildType::Package.includes(VARIANT));
    assert!(!BuildType::Variant.includes(PACKAGE));
    assert!(!BuildType::Variant.includes(32));
    assert!(!BuildType::Variant.includes(0));
}

#[test]
fn test_sensitive_env_vars_variant() {
    let list: Vec<&str> = sensitive_env_vars(BuildType::Variant).collect();
    assert!(list.contains(&"BUILDSYS_ARCH"));
    assert!(list.contains(&"BUILDSYS_VARIANT"));
    assert!(!list.contains(&"BUILDSYS_PACKAGES_DIR"));
}

#[test]
fn test_sensitive_env_vars_package() {
    let list: Vec<&str> = sensitive_env_vars(BuildType::Package).collect();
    assert!(list.contains(&"BUILDSYS_ARCH"));
    assert!(list.contains(&"BUILDSYS_PACKAGES_DIR"));
    assert!(!list.contains(&"BUILDSYS_VARIANT"));
}
