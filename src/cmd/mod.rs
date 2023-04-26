use anyhow::Result;
use clap::Parser;

pub(super) fn run(_: Args) -> Result<()> {
    println!("This program is a placeholder for a Bottlerocket build tool.");
    Ok(())
}

/// A tool for building custom variants of Bottlerocket!
///
/// This tool is under construction and not ready for use. Please check back with the project later!
#[derive(Parser, Debug)]
#[clap(about, long_about = None)]
pub(super) struct Args {}
