//! The check_expirations module owns the 'check-repo-expirations' subcommand and provide methods for
//! checking the metadata expirations of a given TUF repository.

use super::RepoTransport;
use crate::repo::{error as repo_error, repo_urls};
use crate::Args;
use chrono::{DateTime, Utc};
use log::{error, info, trace, warn};
use parse_datetime::parse_datetime;
use pubsys_config::InfraConfig;
use snafu::{OptionExt, ResultExt};
use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use structopt::StructOpt;
use tempfile::tempdir;
use tough::{ExpirationEnforcement, Limits, Repository, Settings};
use url::Url;

/// Checks for metadata expirations for a set of TUF repositories
#[derive(Debug, StructOpt)]
#[structopt(setting = clap::AppSettings::DeriveDisplayOrder)]
pub(crate) struct CheckExpirationsArgs {
    #[structopt(long)]
    /// Use this named repo infrastructure from Infra.toml
    repo: String,

    #[structopt(long)]
    /// The architecture of the repo being checked for expirations
    arch: String,
    #[structopt(long)]
    /// The variant of the repo being checked for expirations
    variant: String,

    #[structopt(long, parse(from_os_str))]
    /// Path to root.json for this repo
    root_role_path: PathBuf,

    #[structopt(long, parse(try_from_str = parse_datetime))]
    /// Finds metadata files expiring between now and a specified time; RFC3339 date or "in X hours/days/weeks"
    expiration_limit: DateTime<Utc>,
}

/// Checks for upcoming role expirations, gathering them in a map of role to expiration datetime.
fn find_upcoming_metadata_expiration<T>(
    repo: &Repository<'_, T>,
    end_date: DateTime<Utc>,
) -> HashMap<tough::schema::RoleType, DateTime<Utc>>
where
    T: tough::Transport,
{
    let mut expirations = HashMap::new();
    info!(
        "Looking for metadata expirations happening from now to {}",
        end_date
    );
    if repo.root().signed.expires <= end_date {
        expirations.insert(tough::schema::RoleType::Root, repo.root().signed.expires);
    }
    if repo.snapshot().signed.expires <= end_date {
        expirations.insert(
            tough::schema::RoleType::Snapshot,
            repo.snapshot().signed.expires,
        );
    }
    if repo.targets().signed.expires <= end_date {
        expirations.insert(
            tough::schema::RoleType::Targets,
            repo.targets().signed.expires,
        );
    }
    if repo.timestamp().signed.expires <= end_date {
        expirations.insert(
            tough::schema::RoleType::Timestamp,
            repo.timestamp().signed.expires,
        );
    }

    expirations
}

fn check_expirations(
    transport: &RepoTransport,
    root_role_path: &PathBuf,
    metadata_url: &Url,
    targets_url: &Url,
    expiration_limit: DateTime<Utc>,
) -> Result<()> {
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
        // We're gonna check the expiration ourselves
        expiration_enforcement: ExpirationEnforcement::Unsafe,
    };

    // Load the repository
    let repo = Repository::load(transport, settings).context(repo_error::RepoLoad {
        metadata_base_url: metadata_url.clone(),
    })?;
    info!("Loaded TUF repo:\t{}", metadata_url);

    info!("Root expiration:\t{}", repo.root().signed.expires);
    info!("Snapshot expiration:\t{}", repo.snapshot().signed.expires);
    info!("Targets expiration:\t{}", repo.targets().signed.expires);
    info!("Timestamp expiration:\t{}", repo.timestamp().signed.expires);
    // Check for upcoming metadata expirations if a timeframe is specified
    let upcoming_expirations = find_upcoming_metadata_expiration(&repo, expiration_limit);
    if !upcoming_expirations.is_empty() {
        let now = Utc::now();
        for (role, expiration_date) in upcoming_expirations {
            if expiration_date < now {
                error!(
                    "Repo '{}': '{}' expired on {}",
                    metadata_url, role, expiration_date
                )
            } else {
                warn!(
                    "Repo '{}': '{}' expiring in {} at {}",
                    metadata_url,
                    role,
                    expiration_date - now,
                    expiration_date
                )
            }
        }
        return Err(Error::RepoExpirations {
            metadata_url: metadata_url.clone(),
        });
    }

    Ok(())
}

/// Common entrypoint from main()
pub(crate) fn run(args: &Args, check_expirations_args: &CheckExpirationsArgs) -> Result<()> {
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
        .get(&check_expirations_args.repo)
        .with_context(|| repo_error::MissingConfig {
            missing: format!("definition for repo {}", &check_expirations_args.repo),
        })?;

    let transport = RepoTransport::default();
    let repo_urls = repo_urls(
        &repo_config,
        &check_expirations_args.variant,
        &check_expirations_args.arch,
    )?
    .context(repo_error::MissingRepoUrls {
        repo: &check_expirations_args.repo,
    })?;
    check_expirations(
        &transport,
        &check_expirations_args.root_role_path,
        &repo_urls.0,
        repo_urls.1,
        check_expirations_args.expiration_limit,
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

        #[snafu(display("Found expiring/expired metadata in '{}'", metadata_url))]
        RepoExpirations { metadata_url: Url },
    }
}
pub(crate) use error::Error;

type Result<T> = std::result::Result<T, error::Error>;
