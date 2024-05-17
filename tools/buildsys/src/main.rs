/*!
This tool carries out a package or variant build using Docker.

It is meant to be called by a Cargo build script. To keep those scripts simple,
all of the configuration is taken from the environment, with the build type
specified as a command line argument.

The implementation is closely tied to the top-level Dockerfile.

*/
mod args;
mod builder;
mod cache;
mod gomod;
mod project;
mod spec;

use crate::args::{BuildPackageArgs, BuildVariantArgs, Buildsys, Command, RepackVariantArgs};
use crate::builder::DockerBuild;
use buildsys::manifest::{BundleModule, ImageFeature, Manifest, ManifestInfo, SupportedArch};
use cache::LookasideCache;
use clap::Parser;
use gomod::GoMod;
use project::ProjectInfo;
use snafu::{ensure, ResultExt};
use spec::SpecInfo;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process;

mod error {
    use buildsys::manifest::SupportedArch;
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub(super)))]
    pub(super) enum Error {
        #[snafu(display("{source}"))]
        ManifestParse { source: buildsys::manifest::Error },

        #[snafu(display("{source}"))]
        SpecParse { source: super::spec::error::Error },

        #[snafu(display("{source}"))]
        ExternalFileFetch { source: super::cache::error::Error },

        #[snafu(display("{source}"))]
        GoMod { source: super::gomod::error::Error },

        #[snafu(display("{source}"))]
        ProjectCrawl {
            source: super::project::error::Error,
        },

        #[snafu(display("{source}"))]
        BuildAttempt {
            source: super::builder::error::Error,
        },

        #[snafu(display("Unable to instantiate the builder: {source}"))]
        BuilderInstantiation {
            source: crate::builder::error::Error,
        },

        #[snafu(display(
            "Unsupported architecture {}, this variant supports {}",
            arch,
            supported_arches.join(", ")
        ))]
        UnsupportedArch {
            arch: SupportedArch,
            supported_arches: Vec<String>,
        },
    }
}

type Result<T> = std::result::Result<T, error::Error>;

// Returning a Result from main makes it print a Debug representation of the error, but with Snafu
// we have nice Display representations of the error, so we wrap "main" (run) and print any error.
// https://github.com/shepmaster/snafu/issues/110
fn main() {
    let args = Buildsys::parse();
    if let Err(e) = run(args) {
        eprintln!("{}", e);
        process::exit(1);
    }
}

fn run(args: Buildsys) -> Result<()> {
    args::rerun_for_envs(args.command.build_type());
    match args.command {
        Command::BuildPackage(args) => build_package(*args),
        Command::BuildVariant(args) => build_variant(*args),
        Command::RepackVariant(args) => repack_variant(*args),
    }
}

fn build_package(args: BuildPackageArgs) -> Result<()> {
    let manifest_file = "Cargo.toml";
    println!("cargo:rerun-if-changed={}", manifest_file);

    let manifest = Manifest::new(
        args.common.cargo_manifest_dir.join(manifest_file),
        &args.common.cargo_metadata_path,
    )
    .context(error::ManifestParseSnafu)?;

    let image_features = get_package_features_and_emit_cargo_watches_for_variant_sensitivity(
        &manifest,
        &args.common.root_dir,
        &args.variant,
        args.common.arch,
    )?;

    if let Some(files) = manifest.info().external_files() {
        let lookaside_cache = LookasideCache::new(
            &args.common.version_full,
            args.lookaside_cache.clone(),
            args.upstream_source_fallback == "true",
        );
        lookaside_cache
            .fetch(files)
            .context(error::ExternalFileFetchSnafu)?;
        for f in files {
            if f.bundle_modules.is_none() {
                continue;
            }

            for b in f.bundle_modules.as_ref().unwrap() {
                match b {
                    BundleModule::Go => GoMod::vendor(
                        &args.common.root_dir,
                        &args.common.cargo_manifest_dir,
                        f,
                        &args.common.sdk_image,
                    )
                    .context(error::GoModSnafu)?,
                }
            }
        }
    }

    if let Some(groups) = manifest.info().source_groups() {
        let dirs = groups
            .iter()
            .map(|d| args.sources_dir.join(d))
            .collect::<Vec<_>>();
        let info = ProjectInfo::crawl(&dirs).context(error::ProjectCrawlSnafu)?;
        for f in info.files {
            println!("cargo:rerun-if-changed={}", f.display());
        }
    }

    // Package developer can override name of package if desired, e.g. to name package with
    // characters invalid in Cargo crate names
    let package = manifest.info().package_name();
    let spec = format!("{}.spec", package);
    println!("cargo:rerun-if-changed={}", spec);

    let info = SpecInfo::new(PathBuf::from(&spec)).context(error::SpecParseSnafu)?;

    for f in info.sources {
        println!("cargo:rerun-if-changed={}", f.display());
    }

    for f in info.patches {
        println!("cargo:rerun-if-changed={}", f.display());
    }

    DockerBuild::new_package(args, &manifest, image_features)
        .context(error::BuilderInstantiationSnafu)?
        .build()
        .context(error::BuildAttemptSnafu)
}

fn build_variant(args: BuildVariantArgs) -> Result<()> {
    let manifest_file = "Cargo.toml";
    println!("cargo:rerun-if-changed={}", manifest_file);

    let manifest = Manifest::new(
        args.common.cargo_manifest_dir.join(manifest_file),
        &args.common.cargo_metadata_path,
    )
    .context(error::ManifestParseSnafu)?;

    supported_arch(manifest.info(), args.common.arch)?;

    DockerBuild::new_variant(args, &manifest)
        .context(error::BuilderInstantiationSnafu)?
        .build()
        .context(error::BuildAttemptSnafu)
}

fn repack_variant(args: RepackVariantArgs) -> Result<()> {
    let manifest_file = "Cargo.toml";

    let manifest = Manifest::new(
        args.common.cargo_manifest_dir.join(manifest_file),
        &args.common.cargo_metadata_path,
    )
    .context(error::ManifestParseSnafu)?;

    supported_arch(manifest.info(), args.common.arch)?;

    DockerBuild::repack_variant(args, &manifest)
        .context(error::BuilderInstantiationSnafu)?
        .build()
        .context(error::BuildAttemptSnafu)
}

/// Ensure that the current arch is supported by the current variant
fn supported_arch(manifest: &ManifestInfo, arch: SupportedArch) -> Result<()> {
    if let Some(supported_arches) = manifest.supported_arches() {
        ensure!(
            supported_arches.contains(&arch),
            error::UnsupportedArchSnafu {
                arch,
                supported_arches: supported_arches
                    .iter()
                    .map(|a| a.to_string())
                    .collect::<Vec<String>>()
            }
        )
    }
    Ok(())
}

fn get_package_features_and_emit_cargo_watches_for_variant_sensitivity(
    manifest: &Manifest,
    root_dir: &Path,
    variant: &str,
    arch: SupportedArch,
) -> Result<HashSet<ImageFeature>> {
    let package_features = manifest.info().package_features();

    // Load the Variant manifest to find image features that may affect the package build.
    let variant_manifest_path = root_dir.join("variants").join(variant).join("Cargo.toml");

    let variant_manifest =
        ManifestInfo::new(variant_manifest_path).context(error::ManifestParseSnafu)?;
    supported_arch(&variant_manifest, arch)?;
    let mut image_features = variant_manifest.image_features();

    // For any package feature specified in the package manifest, track the corresponding
    // environment variable for changes to the ambient set of image features for the current
    // variant.
    if let Some(package_features) = &package_features {
        for package_feature in package_features {
            println!(
                "cargo:rerun-if-env-changed=BUILDSYS_VARIANT_IMAGE_FEATURE_{}",
                package_feature
            );
        }
    }

    // Keep only the image features that the package has indicated that it tracks, if any.
    if let Some(image_features) = &mut image_features {
        match package_features {
            Some(package_features) => image_features.retain(|k| package_features.contains(k)),
            None => image_features.clear(),
        }
    }

    // If manifest has package.metadata.build-package.variant-sensitive set, then track the
    // appropriate environment variable for changes.
    if let Some(sensitivity) = manifest.info().variant_sensitive() {
        use buildsys::manifest::{SensitivityType::*, VariantSensitivity::*};
        fn emit_variant_env(suffix: Option<&str>) {
            if let Some(suffix) = suffix {
                println!(
                    "cargo:rerun-if-env-changed=BUILDSYS_VARIANT_{}",
                    suffix.to_uppercase()
                );
            } else {
                println!("cargo:rerun-if-env-changed=BUILDSYS_VARIANT");
            }
        }
        match sensitivity {
            Any(false) => (),
            Any(true) => emit_variant_env(None),
            Specific(Platform) => emit_variant_env(Some("platform")),
            Specific(Runtime) => emit_variant_env(Some("runtime")),
            Specific(Family) => emit_variant_env(Some("family")),
            Specific(Flavor) => emit_variant_env(Some("flavor")),
        }
    }

    Ok(image_features.unwrap_or_default())
}
