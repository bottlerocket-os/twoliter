/*!
`pubsys` simplifies the process of publishing Bottlerocket updates.

Currently implemented:
* building repos, whether starting from an existing repo or from scratch
* registering and copying EC2 AMIs
* Marking EC2 AMIs public (or private again)
* setting SSM parameters based on built AMIs
* promoting SSM parameters from versioned entries to named (e.g. 'latest')

To be implemented:
* high-level document describing pubsys usage with examples

Configuration comes from:
* command-line parameters, to specify basic options and paths to the below files
* Infra.toml, for repo and AMI configuration
* Release.toml, for migrations
* Policy files for repo metadata expiration and update wave timing
*/

#![deny(rust_2018_idioms)]

mod aws;
mod repo;

use semver::Version;
use simplelog::{Config as LogConfig, LevelFilter, TermLogger, TerminalMode};
use snafu::ResultExt;
use std::path::PathBuf;
use std::process;
use structopt::StructOpt;
use tokio::runtime::Runtime;

fn run() -> Result<()> {
    // Parse and store the args passed to the program
    let args = Args::from_args();

    // TerminalMode::Mixed will send errors to stderr and anything less to stdout.
    TermLogger::init(args.log_level, LogConfig::default(), TerminalMode::Mixed)
        .context(error::Logger)?;

    match args.subcommand {
        SubCommand::Repo(ref repo_args) => repo::run(&args, &repo_args).context(error::Repo),
        SubCommand::Ami(ref ami_args) => {
            let mut rt = Runtime::new().context(error::Runtime)?;
            rt.block_on(async { aws::ami::run(&args, &ami_args).await.context(error::Ami) })
        }
        SubCommand::PublishAmi(ref publish_args) => {
            let mut rt = Runtime::new().context(error::Runtime)?;
            rt.block_on(async {
                aws::publish_ami::run(&args, &publish_args)
                    .await
                    .context(error::PublishAmi)
            })
        }
        SubCommand::Ssm(ref ssm_args) => {
            let mut rt = Runtime::new().context(error::Runtime)?;
            rt.block_on(async { aws::ssm::run(&args, &ssm_args).await.context(error::Ssm) })
        }
        SubCommand::PromoteSsm(ref promote_args) => {
            let mut rt = Runtime::new().context(error::Runtime)?;
            rt.block_on(async {
                aws::promote_ssm::run(&args, &promote_args)
                    .await
                    .context(error::PromoteSsm)
            })
        }
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("{}", e);
        process::exit(1);
    }
}

/// Automates publishing of Bottlerocket updates
#[derive(Debug, StructOpt)]
#[structopt(setting = clap::AppSettings::DeriveDisplayOrder)]
struct Args {
    #[structopt(global = true, long, default_value = "INFO")]
    /// How much detail to log; from least to most: ERROR, WARN, INFO, DEBUG, TRACE
    log_level: LevelFilter,

    #[structopt(long, parse(from_os_str))]
    /// Path to Infra.toml  (NOTE: must be specified before subcommand)
    infra_config_path: PathBuf,

    #[structopt(subcommand)]
    subcommand: SubCommand,
}

#[derive(Debug, StructOpt)]
enum SubCommand {
    Repo(repo::RepoArgs),

    Ami(aws::ami::AmiArgs),
    PublishAmi(aws::publish_ami::PublishArgs),

    Ssm(aws::ssm::SsmArgs),
    PromoteSsm(aws::promote_ssm::PromoteArgs),
}

/// Parses a SemVer, stripping a leading 'v' if present
pub(crate) fn friendly_version(
    mut version_str: &str,
) -> std::result::Result<Version, semver::SemVerError> {
    if version_str.starts_with('v') {
        version_str = &version_str[1..];
    };

    Version::parse(version_str)
}

mod error {
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility = "pub(super)")]
    pub(super) enum Error {
        #[snafu(display("Failed to build AMI: {}", source))]
        Ami { source: crate::aws::ami::Error },

        #[snafu(display("Logger setup error: {}", source))]
        Logger { source: simplelog::TermLogError },

        #[snafu(display("Failed to publish AMI: {}", source))]
        PublishAmi {
            source: crate::aws::publish_ami::Error,
        },

        #[snafu(display("Failed to promote SSM: {}", source))]
        PromoteSsm {
            source: crate::aws::promote_ssm::Error,
        },

        #[snafu(display("Failed to build repo: {}", source))]
        Repo { source: crate::repo::Error },

        #[snafu(display("Failed to create async runtime: {}", source))]
        Runtime { source: std::io::Error },

        #[snafu(display("Failed to update SSM: {}", source))]
        Ssm { source: crate::aws::ssm::Error },
    }
}
type Result<T> = std::result::Result<T, error::Error>;
