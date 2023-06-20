use crate::cmd::{init_logger, Args};
use anyhow::Result;
use clap::Parser;

mod cmd;
mod common;
mod docker;

/// `anyhow` prints a nicely formatted error message with `Debug`, so we can return a result from
/// the `main` function.
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    init_logger(args.log_level);
    cmd::run(args).await
}
