mod build;
mod debug;
mod make;

use self::build::BuildCommand;
use crate::cmd::debug::DebugAction;
use crate::cmd::make::Make;
use anyhow::Result;
use clap::Parser;
use env_logger::Builder;
use log::LevelFilter;

const DEFAULT_LEVEL_FILTER: LevelFilter = LevelFilter::Info;

/// A tool for building custom variants of Bottlerocket.
#[derive(Debug, Parser)]
#[clap(about, long_about = None, version)]
pub(crate) struct Args {
    /// Set the logging level. One of [off|error|warn|info|debug|trace]. Defaults to warn. You can
    /// also leave this unset and use the RUST_LOG env variable. See
    /// https://github.com/rust-cli/env_logger/
    #[clap(long = "log-level")]
    pub(crate) log_level: Option<LevelFilter>,

    #[clap(subcommand)]
    pub(crate) subcommand: Subcommand,
}

#[derive(Debug, Parser)]
pub(crate) enum Subcommand {
    /// Build something, such as a Bottlerocket image or a kit of packages.
    #[clap(subcommand)]
    Build(BuildCommand),

    Make(Make),

    /// Commands that are used for checking and troubleshooting Twoliter's internals.
    #[clap(subcommand)]
    Debug(DebugAction),
}

/// Entrypoint for the `twoliter` command line program.
pub(super) async fn run(args: Args) -> Result<()> {
    match args.subcommand {
        Subcommand::Build(build_command) => build_command.run().await,
        Subcommand::Make(make_args) => make_args.run().await,
        Subcommand::Debug(debug_action) => debug_action.run().await,
    }
}

/// use `level` if present, or else use `RUST_LOG` if present, or else use a default.
pub(super) fn init_logger(level: Option<LevelFilter>) {
    match (std::env::var(env_logger::DEFAULT_FILTER_ENV).ok(), level) {
        (Some(_), None) => {
            // RUST_LOG exists and level does not; use the environment variable.
            Builder::from_default_env().init();
        }
        _ => {
            // use provided log level or default for this crate only.
            Builder::new()
                .filter(
                    Some(env!("CARGO_CRATE_NAME")),
                    level.unwrap_or(DEFAULT_LEVEL_FILTER),
                )
                .init();
        }
    }
}
