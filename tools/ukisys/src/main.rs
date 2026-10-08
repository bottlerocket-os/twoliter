//! UKI command-line construction and recovery of an unsigned systemd-stub
//! from a finished Bottlerocket UKI.

mod error;
mod pe;

use clap::{Parser, Subcommand};
use pe::PeImage;
use snafu::ResultExt;
use std::path::{Path, PathBuf};

use crate::error::Result;

/// All four sections must be removed, not just the three payload sections:
/// `ukify build` overwrites any same-named section already in the stub in
/// place instead of appending fresh content, rather than skipping it.
const TRAILING_SECTIONS_TO_REMOVE: &[&str] = &[".osrel", ".cmdline", ".uname", ".linux"];

/// Command-line arguments for ukisys.
#[derive(Parser)]
#[command(
    version,
    about = "Command-line construction and PE section removal for Bottlerocket UKIs"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

/// Subcommands for UKI construction and repacking.
#[derive(Subcommand)]
enum Command {
    /// Construct a UKI command-line file from generated binary bootconfig.
    Cmdline {
        #[arg(long)]
        bootconfig: PathBuf,
        /// Base kernel arguments, with literal kernel quoting.
        #[arg(long, allow_hyphen_values = true)]
        kernel_args: String,
        /// Base init arguments (after the separator).
        #[arg(long, allow_hyphen_values = true)]
        init_args: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Derive an unsigned systemd-stub PE from a finished, signed UKI.
    DeriveStub {
        /// Path to the finished, signed UKI to strip.
        uki: PathBuf,

        /// Path to write the derived, unsigned stub to.
        stub: PathBuf,
    },
}

#[snafu::report]
fn main() -> Result<()> {
    let args = Args::parse();
    match &args.command {
        Command::DeriveStub { uki, stub } => derive_stub(uki, stub),
        Command::Cmdline {
            bootconfig,
            kernel_args,
            init_args,
            output,
        } => {
            let data = std::fs::read(bootconfig)
                .with_whatever_context(|_| format!("Failed to read '{}'", bootconfig.display()))?;
            let config = bootconfig::parse(&data)?;
            let cmdline = config.uki_cmdline(kernel_args, init_args)?;
            std::fs::write(output, cmdline)
                .with_whatever_context(|_| format!("Failed to write '{}'", output.display()))
        }
    }
}

fn derive_stub(uki_path: &Path, stub_path: &Path) -> Result<()> {
    let mut image = PeImage::load(uki_path)
        .with_whatever_context(|_| format!("Failed to parse UKI '{}'", uki_path.display()))?;

    image
        .remove_signature()
        .with_whatever_context(|_| "Failed to remove signature".to_string())?;

    image
        .derive_stub_by_truncating_trailing_sections(TRAILING_SECTIONS_TO_REMOVE)
        .with_whatever_context(|_| "Failed to derive stub".to_string())?;

    image
        .write_to(stub_path)
        .with_whatever_context(|_| format!("Failed to write stub '{}'", stub_path.display()))?;

    eprintln!(
        "ukisys: stripped signature and trailing sections {:?} from '{}', wrote {} bytes to '{}'",
        TRAILING_SECTIONS_TO_REMOVE,
        uki_path.display(),
        image.bytes.len(),
        stub_path.display(),
    );

    Ok(())
}
