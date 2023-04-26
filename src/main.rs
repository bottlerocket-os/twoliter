use crate::cmd::Args;
use anyhow::Result;
use clap::Parser;

mod cmd;

/// `anyhow` prints a nicely formatted error message with `Debug`, so we can return a result from
/// the `main` function.
fn main() -> Result<()> {
    let args = Args::parse();
    cmd::run(args)
}
