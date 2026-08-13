//! ukisys: derives the unsigned systemd-stub PE a Bottlerocket UKI was
//! built from, by stripping its Authenticode signature and truncating the
//! trailing payload sections. Those sections are `.osrel`, `.cmdline`,
//! `.uname`, and `.linux`.

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
    about = "PE section removal for Bottlerocket Unified Kernel Images"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

/// Subcommands for UKI repack section removal.
#[derive(Subcommand)]
enum Command {
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
