//! The validate_repo module owns the 'validate-repo' subcommand and provides methods for validating
//! a given TUF repository by attempting to load the repository and download its targets.

use crate::repo::{error as repo_error, repo_urls};
use crate::{read_stream, repo, Args};
use clap::Parser;
use futures::{stream, StreamExt};
use log::{info, trace};
use pubsys_config::InfraConfig;
use snafu::{OptionExt, ResultExt};
use std::io::Cursor;
use std::path::PathBuf;
use tokio::io;
use tough::{Repository, RepositoryLoader, TargetName};
use url::Url;

/// Validates a set of TUF repositories
#[derive(Debug, Parser)]
pub(crate) struct ValidateRepoArgs {
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
    /// Path to root.json for this repo
    root_role_path: PathBuf,

    #[arg(long)]
    /// Specifies whether to validate all listed targets by attempting to download them
    validate_targets: bool,
}

/// If we are on a machine with a large number of cores, then we limit the number of simultaneous
/// downloads to this arbitrarily chosen maximum.
const MAX_DOWNLOAD_THREADS: usize = 16;

/// Retrieves listed targets and attempts to download them for validation purposes.
async fn retrieve_targets(repo: &Repository) -> Result<(), Error> {
    let targets = repo.targets().signed.targets.clone();
    let download_futures = stream::iter(
        targets
            .keys()
            .map(|target_name| download_target(repo.clone(), target_name.clone())),
    );
    let mut buffered = download_futures.buffer_unordered(MAX_DOWNLOAD_THREADS);
    while let Some(result) = buffered.next().await {
        let _ = result?;
    }
    Ok(())
}

async fn download_target(repo: Repository, target: TargetName) -> Result<u64, Error> {
    info!("Downloading target: {}", target.raw());
    let stream = match repo.read_target(&target).await {
        Ok(Some(stream)) => stream,
        Ok(None) => {
            return error::TargetMissingSnafu {
                target: target.raw(),
            }
            .fail()
        }
        Err(e) => {
            return Err(e).context(error::TargetReadSnafu {
                target: target.raw(),
            })
        }
    };
    let mut bytes = Cursor::new(read_stream(stream).await.context(error::StreamSnafu)?);
    // tough's `Read` implementation validates the target as it's being downloaded
    io::copy(&mut bytes, &mut io::sink())
        .await
        .context(error::TargetDownloadSnafu {
            target: target.raw(),
        })
}

async fn validate_repo(
    root_role_path: &PathBuf,
    metadata_url: Url,
    targets_url: &Url,
    validate_targets: bool,
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
    info!("Loaded TUF repo: {}", metadata_url);
    if validate_targets {
        // Try retrieving listed targets
        retrieve_targets(&repo).await?;
    }

    Ok(())
}

/// Common entrypoint from main()
pub(crate) async fn run(args: &Args, validate_repo_args: &ValidateRepoArgs) -> Result<(), Error> {
    // If a lock file exists, use that, otherwise use Infra.toml
    let infra_config = InfraConfig::from_path_or_lock(&args.infra_config_path, false)
        .context(repo_error::ConfigSnafu)?;
    trace!("Parsed infra config: {:?}", infra_config);
    let repo_config = infra_config
        .repo
        .as_ref()
        .context(repo_error::MissingConfigSnafu {
            missing: "repo section",
        })?
        .get(&validate_repo_args.repo)
        .context(repo_error::MissingConfigSnafu {
            missing: format!("definition for repo {}", &validate_repo_args.repo),
        })?;

    let repo_urls = repo_urls(
        repo_config,
        &validate_repo_args.variant,
        &validate_repo_args.arch,
    )?
    .context(repo_error::MissingRepoUrlsSnafu {
        repo: &validate_repo_args.repo,
    })?;
    validate_repo(
        &validate_repo_args.root_role_path,
        repo_urls.0,
        repo_urls.1,
        validate_repo_args.validate_targets,
    )
    .await
}

mod error {
    use snafu::Snafu;
    use std::io;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub(super)))]
    pub(crate) enum Error {
        #[snafu(context(false), display("{}", source))]
        Repo {
            #[snafu(source(from(crate::repo::Error, Box::new)))]
            source: Box<crate::repo::Error>,
        },

        #[snafu(display("Error reading bytes from stream: {}", source))]
        Stream { source: tough::error::Error },

        #[snafu(display("Failed to download and write target '{}': {}", target, source))]
        TargetDownload { target: String, source: io::Error },

        #[snafu(display("Missing target: {}", target))]
        TargetMissing { target: String },

        #[snafu(display("Failed to read target '{}' from repo: {}", target, source))]
        TargetRead {
            target: String,
            #[snafu(source(from(tough::error::Error, Box::new)))]
            source: Box<tough::error::Error>,
        },
    }
}
pub(crate) use error::Error;
