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
mod rustmod;
mod spec;
mod vendormod;

use crate::args::{
    BuildKitArgs, BuildPackageArgs, BuildVariantArgs, Buildsys, Command, RepackVariantArgs,
};
use crate::builder::DockerBuild;
use buildsys::manifest::{
    resolved_image_layout, validate_image_features, BundleModule, ImageFeature, Manifest,
    ManifestInfo, SupportedArch,
};
use buildsys_config::EXTERNAL_KIT_METADATA;
use cache::LookasideCache;
use clap::Parser;
use filetime::FileTime;
use gomod::GoMod;
use project::ProjectInfo;
use rustmod::RustMod;
use snafu::{ensure, ResultExt};
use spec::SpecInfo;
use std::path::{Path, PathBuf};
use std::process;

mod error {
    use snafu::Snafu;
    use std::path::PathBuf;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub(super)))]
    pub(super) enum Error {
        #[snafu(display("{source}"))]
        ManifestParse { source: buildsys::manifest::Error },

        #[snafu(display("invalid image-feature combination: {source}"))]
        ImageFeatures { source: buildsys::manifest::Error },

        #[snafu(display("{source}"))]
        SpecParse { source: super::spec::error::Error },

        #[snafu(display("{source}"))]
        ExternalFileFetch { source: super::cache::error::Error },

        #[snafu(display("Failed to get metadata for '{}': {}", path.display(), source))]
        FileMetadata {
            path: PathBuf,
            source: std::io::Error,
        },

        #[snafu(display("{source}"))]
        GoMod { source: super::gomod::error::Error },

        #[snafu(display("{source}"))]
        RustMod {
            source: super::rustmod::error::Error,
        },

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
        "The manifest for package {} has a package.metadata.build-package.package-features \
            section. This functionality has been removed from the build system. Packages are no \
            longer allowed to be aware of what variant they are being built for. Please remove \
            this key from {}",
        name,
        path.display(),
        ))]
        PackageFeatures { name: String, path: PathBuf },

        #[snafu(display(
        "The manifest for package {} has a package.metadata.build-package.variant-sensitive \
            key. This functionality has been removed from the build system. Packages are no \
            longer allowed to be aware of what variant they are being built for. Please remove \
            this key from {}",
        name,
        path.display(),
        ))]
        VariantSensitive { name: String, path: PathBuf },
    }
}

type Result<T> = std::result::Result<T, error::Error>;

// Returning a Result from main makes it print a Debug representation of the error, but with Snafu
// we have nice Display representations of the error, so we wrap "main" (run) and print any error.
// https://github.com/shepmaster/snafu/issues/110
fn main() {
    let args = Buildsys::parse();
    if let Err(e) = run(args) {
        eprintln!("{e}");
        process::exit(1);
    }
}

fn run(args: Buildsys) -> Result<()> {
    args::rerun_for_envs(args.command.build_type());
    match args.command {
        Command::BuildPackage(args) => build_package(*args),
        Command::BuildKit(args) => build_kit(*args),
        Command::BuildVariant(args) => build_variant(*args),
        Command::RepackVariant(args) => repack_variant(*args),
    }
}

fn build_package(args: BuildPackageArgs) -> Result<()> {
    let manifest_file = "Cargo.toml";
    let manifest_path = args.common.cargo_manifest_dir.join(manifest_file);
    println!("cargo:rerun-if-changed={manifest_file}");
    println!(
        "cargo:rerun-if-changed={}",
        args.common.root_dir.join(EXTERNAL_KIT_METADATA).display()
    );

    let manifest = Manifest::new(&manifest_path, &args.common.cargo_metadata_path)
        .context(error::ManifestParseSnafu)?;

    // Check for a deprecated key and error if it is detected.
    ensure_package_is_not_variant_sensitive(&manifest, &manifest_path)?;

    if let Some(files) = manifest.info().external_files() {
        // We need the modification time for any external files or bundled modules to be no later
        // than the manifest's modification time, to avoid triggering spurious rebuilds.
        let metadata =
            std::fs::metadata(manifest_path.clone()).context(error::FileMetadataSnafu {
                path: manifest_path,
            })?;
        let mtime = FileTime::from_last_modification_time(&metadata);

        let lookaside_cache = LookasideCache::new(
            &args.common.version_full,
            args.lookaside_cache.clone(),
            args.upstream_source_fallback == "true",
        );

        lookaside_cache
            .fetch(files, mtime)
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
                        mtime,
                    )
                    .context(error::GoModSnafu)?,
                    BundleModule::Rust => RustMod::vendor(
                        &args.common.root_dir,
                        &args.common.cargo_manifest_dir,
                        f,
                        &args.common.sdk_image,
                        mtime,
                    )
                    .context(error::RustModSnafu)?,
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
    let spec = format!("{package}.spec");
    println!("cargo:rerun-if-changed={spec}");

    let info = SpecInfo::new(PathBuf::from(&spec)).context(error::SpecParseSnafu)?;

    for f in info.sources {
        println!("cargo:rerun-if-changed={}", f.display());
    }

    for f in info.patches {
        println!("cargo:rerun-if-changed={}", f.display());
    }

    if args.common.cicd_hack {
        return Ok(());
    }

    DockerBuild::new_package(args, &manifest)
        .context(error::BuilderInstantiationSnafu)?
        .build()
        .context(error::BuildAttemptSnafu)
}

fn build_kit(args: BuildKitArgs) -> Result<()> {
    let manifest_file = "Cargo.toml";
    println!("cargo:rerun-if-changed={manifest_file}");
    println!(
        "cargo:rerun-if-changed={}",
        args.common.root_dir.join(EXTERNAL_KIT_METADATA).display()
    );

    let manifest = Manifest::new(
        args.common.cargo_manifest_dir.join(manifest_file),
        &args.common.cargo_metadata_path,
    )
    .context(error::ManifestParseSnafu)?;

    if args.common.cicd_hack {
        return Ok(());
    }

    DockerBuild::new_kit(args, &manifest)
        .context(error::BuilderInstantiationSnafu)?
        .build()
        .context(error::BuildAttemptSnafu)
}

fn build_variant(args: BuildVariantArgs) -> Result<()> {
    let manifest_file = "Cargo.toml";
    println!("cargo:rerun-if-changed={manifest_file}");
    println!(
        "cargo:rerun-if-changed={}",
        args.common.root_dir.join(EXTERNAL_KIT_METADATA).display()
    );

    let manifest = Manifest::new(
        args.common.cargo_manifest_dir.join(manifest_file),
        &args.common.cargo_metadata_path,
    )
    .context(error::ManifestParseSnafu)?;

    check_arch_support(manifest.info(), args.common.arch);
    validate_standalone_image_or_warn(manifest.info())?;

    // Resolve declared guest-images and emit a rerun-if-changed for each guest's image
    // directory, so rebuilding a guest variant triggers a rebuild of this host variant.
    let guest_images = manifest
        .guest_image_variant_deps()
        .context(error::ManifestParseSnafu)?;
    let arch = args.common.arch.to_string();
    for (guest, _install_path) in &guest_images {
        let guest_image_dir = args
            .image_dir
            .join(format!("{arch}-{guest}"))
            .join(&args.common.version_full);
        println!("cargo:rerun-if-changed={}", guest_image_dir.display());
    }

    if args.common.cicd_hack {
        return Ok(());
    }

    DockerBuild::new_variant(args, &manifest, guest_images)
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

    check_arch_support(manifest.info(), args.common.arch);
    validate_standalone_image_or_warn(manifest.info())?;

    if args.common.cicd_hack {
        return Ok(());
    }

    DockerBuild::repack_variant(args, &manifest)
        .context(error::BuilderInstantiationSnafu)?
        .build()
        .context(error::BuildAttemptSnafu)
}

/// Ensure that the current arch is supported by the current variant
fn check_arch_support(manifest: &ManifestInfo, arch: SupportedArch) {
    if let Some(supported_arches) = manifest.supported_arches() {
        if !supported_arches.contains(&arch) {
            let supported_arches = supported_arches
                .iter()
                .map(|a| a.to_string())
                .collect::<Vec<String>>();
            println!("cargo:warning={arch} is not one of the supported architectures ({supported_arches:?})");
            std::process::exit(0);
        }
    }
}

/// Re-validate image features against the resolved image layout, and emit a
/// multi-line warning when `standalone-image = true` so that build logs
/// make the implications obvious.
fn validate_standalone_image_or_warn(manifest: &ManifestInfo) -> Result<()> {
    let features = manifest.image_features().unwrap_or_default();
    let raw_layout = manifest.image_layout().cloned().unwrap_or_default();
    let image_format = manifest.image_format();
    let layout = resolved_image_layout(&raw_layout, &features, image_format);
    validate_image_features(&features, &layout, image_format).context(error::ImageFeaturesSnafu)?;

    if features.contains(&ImageFeature::StandaloneImage) {
        // `cargo:warning` is line-oriented, so emit one line per call.
        for line in [
            "WARNING: image feature `standalone-image` is enabled.",
            "With `standalone-image = true`, the resulting image will NOT contain the",
            "Bottlerocket datastore, settings subsystem, host-containers, or any",
            "first-party Bottlerocket management software. You are responsible for",
            "owning all system configuration yourself. The image will not have a",
            "BOTTLEROCKET-PRIVATE partition or a BOTTLEROCKET-DATA partition. If your",
            "variant ships components that expect persistent storage at",
            "partlabel=BOTTLEROCKET-DATA, you may optionally attach such a volume",
            "at runtime; otherwise no extra volume is required.",
        ] {
            println!("cargo:warning={line}");
        }
    }
    Ok(())
}

/// Prior to the release of Kits as a build feature, packages could, and did, declare themselves
/// sensitive to various Variant features so that they could be conditionally compiled based on
/// what variant was being built. This is no longer the case, so we enforce that these keys are no
/// longer supported in package Cargo.toml files.
fn ensure_package_is_not_variant_sensitive(
    manifest: &Manifest,
    manifest_path: &Path,
) -> Result<()> {
    ensure!(
        manifest.info().package_features().is_none(),
        error::PackageFeaturesSnafu {
            name: manifest.info().manifest_name(),
            path: manifest_path
        }
    );

    ensure!(
        manifest.info().variant_sensitive().is_none(),
        error::VariantSensitiveSnafu {
            name: manifest.info().manifest_name(),
            path: manifest_path
        }
    );

    Ok(())
}
