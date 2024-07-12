//! The fetch_variant module owns the 'fetch-variant' subcommand and provides methods for fetching
//! a given variant and download its image targets.

use crate::repo::{error as repo_error, repo_urls};
use crate::{repo, Args};
use clap::Parser;
use log::{debug, info, trace};
use pubsys_config::InfraConfig;
use snafu::{OptionExt, ResultExt};
use std::path::PathBuf;
use tempfile::tempdir_in;
use tough::{Prefix, Repository, RepositoryLoader, TargetName};
use url::Url;

use buildsys::manifest::{ImageFormat, ManifestInfo, PartitionPlan};

/// fetching and downdloaing the image targets of a given variant
#[derive(Debug, Parser)]
pub(crate) struct FetchVariantArgs {
    #[arg(long)]
    /// Use this named repo infrastructure from Infra.toml
    repo: String,

    #[arg(long)]
    /// The architecture of the repo being validated
    arch: String,

    #[arg(long)]
    /// The variant of the repo being validated
    variant: String,

    #[arg(long)]
    /// The version of the repo being validated
    version: String,

    #[arg(long)]
    /// The build of the repo being validated
    build: String,

    #[arg(long)]
    /// Path to root.json for this repo
    root_role_path: PathBuf,

    #[arg(long)]
    /// Where to store the downloaded img files
    outdir: PathBuf,

    #[arg(long)]
    /// The variant name without extension
    filename_prefix: String,

    #[arg(long)]
    /// The manifest of the variant
    variant_manifest: PathBuf,
}

/// Download targets
async fn handle_download(
    repository: &Repository,
    outdir: &PathBuf,
    raw_names: &[String],
) -> Result<(), Error> {
    let target_names: Result<Vec<TargetName>, Error> = raw_names
        .iter()
        .map(|s| TargetName::new(s).context(error::InvalidTargetNameSnafu))
        .collect();
    let target_names = target_names?;

    if target_names.is_empty() {
        return error::MissingTargetNamesSnafu.fail();
    };

    // Attempt to make a temporary directory in the parent directory of the outdir
    let tempdir = match outdir.parent() {
        Some(in_dir) => tempdir_in(in_dir).context(error::CreateTempDirSnafu)?,
        None => return error::InvalidOutdirSnafu { path: outdir }.fail(),
    };
    let tempdir_path = tempdir.path();

    let download_target = |name: TargetName| async move {
        info!("\t-> {}", name.raw());
        repository
            .save_target(&name, tempdir_path, Prefix::None)
            .await
            .context(error::SaveTargetSnafu)
    };

    info!("Downloading targets to {tempdir_path:?}");
    for target in target_names.clone() {
        download_target(target).await?;
    }

    debug!("Cleaning up {outdir:?}");
    tokio::fs::remove_dir_all(outdir)
        .await
        .context(error::CleanDirSnafu { path: outdir })?;

    info!("Moving targets to {outdir:?}");
    tokio::fs::create_dir_all(outdir)
        .await
        .context(error::CreateDirSnafu { path: outdir })?;
    for target in target_names {
        let mut tmpdir_target_path = PathBuf::from(tempdir.path());
        tmpdir_target_path.push(target.raw());
        let mut outdir_target_path = outdir.clone();
        outdir_target_path.push(target.raw());
        tokio::fs::rename(tmpdir_target_path, outdir_target_path)
            .await
            .context(error::MoveTargetSnafu)?;
    }

    tempdir.close().context(error::CloseTempDirSnafu)?;
    Ok(())
}

/// Fetch the variant and download its image targets
async fn fetch_variant(
    root_role_path: &PathBuf,
    metadata_url: Url,
    targets_url: &Url,
    outdir: &PathBuf,
    filename_prefix: &str,
    variant_manifest: &PathBuf,
    variant: &str,
) -> Result<(), Error> {
    // Load the repository
    let repo = RepositoryLoader::new(
        &repo::root_bytes(root_role_path).await?,
        metadata_url.clone(),
        targets_url.clone(),
    )
    .load()
    .await
    .context(repo_error::RepoLoadSnafu {
        metadata_base_url: metadata_url.clone(),
    })?;

    let manifest_info = ManifestInfo::new(variant_manifest).context(error::ManifestParseSnafu)?;

    let image_layout = manifest_info
        .image_layout()
        .context(error::MissingImageLayoutSnafu { variant })?;
    let image_format = manifest_info.image_format();
    let image_ext = match image_format {
        Some(ImageFormat::Raw) | None => "img.lz4",
        Some(ImageFormat::Qcow2) => "qcow2",
        Some(ImageFormat::Vmdk) => "ova",
    };

    let targets = match image_format {
        // Since the OVA will contain all of the necessary VMDKs, the partition plan is irrelevant.
        Some(ImageFormat::Vmdk) => {
            vec![format!("{filename_prefix}.{image_ext}")]
        }
        _ => match image_layout.partition_plan {
            PartitionPlan::Split => {
                vec![
                    format!("{filename_prefix}.{image_ext}"),
                    format!("{filename_prefix}-data.{image_ext}"),
                ]
            }
            PartitionPlan::Unified => vec![format!("{filename_prefix}.{image_ext}")],
        },
    };
    handle_download(&repo, outdir, &targets).await
}

/// Common entrypoint from main()
pub(crate) async fn run(args: &Args, fetch_variant_args: &FetchVariantArgs) -> Result<(), Error> {
    let infra_config =
        InfraConfig::from_path(&args.infra_config_path).context(repo_error::ConfigSnafu)?;
    trace!("Parsed infra config: {:?}", infra_config);
    let repo_config = infra_config
        .repo
        .as_ref()
        .context(repo_error::MissingConfigSnafu {
            missing: "repo section",
        })?
        .get(&fetch_variant_args.repo)
        .context(repo_error::MissingConfigSnafu {
            missing: format!("definition for repo {}", &fetch_variant_args.repo),
        })?;

    let repo_urls = repo_urls(
        repo_config,
        &fetch_variant_args.variant,
        &fetch_variant_args.arch,
    )?
    .context(repo_error::MissingRepoUrlsSnafu {
        repo: &fetch_variant_args.repo,
    })?;

    let version_full = format!(
        "{}-{}",
        &fetch_variant_args.version, &fetch_variant_args.build
    );
    let mut versioned_outdir = fetch_variant_args.outdir.clone();
    versioned_outdir.push(version_full);

    fetch_variant(
        &fetch_variant_args.root_role_path,
        repo_urls.0,
        repo_urls.1,
        &versioned_outdir,
        &fetch_variant_args.filename_prefix,
        &fetch_variant_args.variant_manifest,
        &fetch_variant_args.variant,
    )
    .await
}

mod error {
    use snafu::Snafu;
    use std::io;
    use std::path::PathBuf;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub(crate)))]
    pub(crate) enum Error {
        #[snafu(display("Failed to clean directory '{}': {}", path.display(), source))]
        CleanDir {
            path: PathBuf,
            source: io::Error,
        },

        #[snafu(display("Failed to delete temporary directory: {}", source))]
        CloseTempDir {
            source: io::Error,
        },

        #[snafu(display("Failed to create directory '{}': {}", path.display(), source))]
        CreateDir {
            path: PathBuf,
            source: io::Error,
        },

        #[snafu(display("Failed to create temporary directory: {}", source))]
        CreateTempDir {
            source: io::Error,
        },

        #[snafu(display("Invalid target name: {}", source))]
        InvalidTargetName {
            source: tough::error::Error,
        },

        #[snafu(display("Invalid output directory '{}'", path.display()))]
        InvalidOutdir {
            path: PathBuf,
        },

        ManifestParse {
            source: buildsys::manifest::Error,
        },

        #[snafu(display("Could not find image layout for {}", variant))]
        MissingImageLayout {
            variant: String,
        },

        #[snafu(display("Target names are not set."))]
        MissingTargetNames,

        #[snafu(display("Failed to move target: {}", source))]
        MoveTarget {
            source: io::Error,
        },

        #[snafu(context(false), display("{}", source))]
        Repo {
            #[snafu(source(from(crate::repo::Error, Box::new)))]
            source: Box<crate::repo::Error>,
        },

        #[snafu(display("Failed to save target: {}", source))]
        SaveTarget {
            source: tough::error::Error,
        },
    }
}

pub(crate) use error::Error;
