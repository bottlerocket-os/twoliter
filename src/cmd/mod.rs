mod args;

pub(crate) use self::args::{Args, Subcommand};
use anyhow::Result;
use env_logger::Builder;
use log::LevelFilter;

const DEFAULT_LEVEL_FILTER: LevelFilter = LevelFilter::Warn;

/// Entrypoint for the `twoliter` command line program.
pub(super) async fn run(args: Args) -> Result<()> {
    match args.subcommand {
        Subcommand::Build(build_command) => build_command.run().await,
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
