//! The refresh_repo module owns the 'refresh-repo' subcommand and provide methods for
//! refreshing and re-signing the metadata files of a given TUF repository.

use super::RepoTransport;
use crate::repo::{
    error as repo_error, get_signing_key_source, repo_urls, set_expirations, set_versions,
};
use crate::Args;
use chrono::{DateTime, Utc};
use lazy_static::lazy_static;
use log::{info, trace};
use pubsys_config::{InfraConfig, RepoExpirationPolicy};
use snafu::{ensure, OptionExt, ResultExt};
use std::fs;
use std::fs::File;
use std::path::{Path, PathBuf};
use structopt::StructOpt;
use tempfile::tempdir;
use tough::editor::RepositoryEditor;
use tough::key_source::KeySource;
use tough::{ExpirationEnforcement, Limits, Repository, Settings};
use url::Url;

lazy_static! {
    static ref EXPIRATION_START_TIME: DateTime<Utc> = Utc::now();
}

/// Refreshes and re-sign TUF repositories' non-root metadata files with new expiration dates
#[derive(Debug, StructOpt)]
#[structopt(setting = clap::AppSettings::DeriveDisplayOrder)]
pub(crate) struct RefreshRepoArgs {
    #[structopt(long)]
    /// Use this named repo infrastructure from Infra.toml
    repo: String,

    #[structopt(long)]
    /// The architecture of the repo being refreshed and re-signed
    arch: String,
    #[structopt(long)]
    /// The variant of the repo being refreshed and re-signed
    variant: String,

    #[structopt(long, parse(from_os_str))]
    /// Path to root.json for this repo
    root_role_path: PathBuf,

    #[structopt(long, parse(from_os_str))]
    /// Path to file that defines when repo non-root metadata should expire
    repo_expiration_policy_path: PathBuf,

    #[structopt(long, parse(from_os_str))]
    /// Where to store the refresh/re-signed repository (just the metadata files)
    outdir: PathBuf,

    #[structopt(long)]
    /// If this flag is set, repositories will succeed in loading and be refreshed even if they have
    /// expired metadata files.
    unsafe_refresh: bool,
}

fn refresh_repo(
    transport: &RepoTransport,
    root_role_path: &PathBuf,
    metadata_out_dir: &PathBuf,
    metadata_url: &Url,
    targets_url: &Url,
    key_source: Box<dyn KeySource>,
    expiration: &RepoExpirationPolicy,
    unsafe_refresh: bool,
) -> Result<(), Error> {
    // If the given metadata directory exists, throw an error.  We don't want to overwrite a user's
    // existing repository.
    ensure!(
        !Path::exists(&metadata_out_dir),
        repo_error::RepoExists {
            path: metadata_out_dir
        }
    );

    // Create a temporary directory where the TUF client can store metadata
    let workdir = tempdir().context(repo_error::TempDir)?;
    let settings = Settings {
        root: File::open(root_role_path).context(repo_error::File {
            path: root_role_path,
        })?,
        datastore: workdir.path(),
        metadata_base_url: metadata_url.as_str(),
        targets_base_url: targets_url.as_str(),
        limits: Limits::default(),
        expiration_enforcement: if unsafe_refresh {
            ExpirationEnforcement::Unsafe
        } else {
            ExpirationEnforcement::Safe
        },
    };

    // Load the repository and get the repo editor for it
    let repo = Repository::load(transport, settings).context(repo_error::RepoLoad {
        metadata_base_url: metadata_url.clone(),
    })?;
    let mut repo_editor =
        RepositoryEditor::from_repo(&root_role_path, repo).context(repo_error::EditorFromRepo)?;
    info!("Loaded TUF repo: {}", metadata_url);

    // Refresh the expiration dates of all non-root metadata files
    set_expirations(&mut repo_editor, &expiration, *EXPIRATION_START_TIME)?;

    // Refresh the versions of all non-root metadata files
    set_versions(&mut repo_editor)?;

    // Sign the repository
    let signed_repo = repo_editor
        .sign(&[key_source])
        .context(repo_error::RepoSign)?;

    // Write out the metadata files for the repository
    info!("Writing repo metadata to: {}", metadata_out_dir.display());
    fs::create_dir_all(&metadata_out_dir).context(repo_error::CreateDir {
        path: &metadata_out_dir,
    })?;
    signed_repo
        .write(&metadata_out_dir)
        .context(repo_error::RepoWrite {
            path: &metadata_out_dir,
        })?;

    Ok(())
}

/// Common entrypoint from main()
pub(crate) fn run(args: &Args, refresh_repo_args: &RefreshRepoArgs) -> Result<(), Error> {
    info!(
        "Using infra config from path: {}",
        args.infra_config_path.display()
    );
    let infra_config =
        InfraConfig::from_path(&args.infra_config_path).context(repo_error::Config)?;
    trace!("Parsed infra config: {:?}", infra_config);

    let repo_config = infra_config
        .repo
        .as_ref()
        .context(repo_error::MissingConfig {
            missing: "repo section",
        })?
        .get(&refresh_repo_args.repo)
        .context(repo_error::MissingConfig {
            missing: format!("definition for repo {}", &refresh_repo_args.repo),
        })?;

    // Get signing key config from repository configuration
    let signing_key_config =
        repo_config
            .signing_keys
            .as_ref()
            .context(repo_error::MissingConfig {
                missing: "signing_keys",
            })?;
    let key_source = get_signing_key_source(signing_key_config);

    // Get the expiration policy
    info!(
        "Using repo expiration policy from path: {}",
        refresh_repo_args.repo_expiration_policy_path.display()
    );
    let expiration =
        RepoExpirationPolicy::from_path(&refresh_repo_args.repo_expiration_policy_path)
            .context(repo_error::Config)?;

    let transport = RepoTransport::default();
    let repo_urls = repo_urls(
        &repo_config,
        &refresh_repo_args.variant,
        &refresh_repo_args.arch,
    )?
    .context(repo_error::MissingRepoUrls {
        repo: &refresh_repo_args.repo,
    })?;
    refresh_repo(
        &transport,
        &refresh_repo_args.root_role_path,
        &refresh_repo_args
            .outdir
            .join(&refresh_repo_args.variant)
            .join(&refresh_repo_args.arch),
        &repo_urls.0,
        repo_urls.1,
        key_source,
        &expiration,
        refresh_repo_args.unsafe_refresh,
    )?;

    Ok(())
}

mod error {
    use snafu::Snafu;
    use url::Url;

    #[derive(Debug, Snafu)]
    #[snafu(visibility = "pub(super)")]
    pub(crate) enum Error {
        #[snafu(context(false), display("{}", source))]
        Repo { source: crate::repo::Error },

        #[snafu(display("Failed to refresh & re-sign metadata for: {:#?}", list_of_urls))]
        RepoRefresh { list_of_urls: Vec<Url> },
    }
}
pub(crate) use error::Error;
