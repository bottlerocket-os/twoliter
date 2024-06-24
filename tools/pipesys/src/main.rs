//! Pipesys provides a command-line tool and library for passing file descriptors over an abstract
//! Unix domain socket. It allows processes in the same network namespace but disjount mount
//! namespaces to efficiently share access to files or directories.

use crate::cmd::{init_logger, Args};
use anyhow::Result;
use clap::Parser;

mod cmd;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    init_logger(args.log_level);
    cmd::run(args).await
}
