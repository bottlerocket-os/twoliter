//! ukisys: merges bootconfig drop-ins and constructs the kernel command line
//! for UKIs, and derives the unsigned systemd-stub PE a Bottlerocket UKI was
//! built from, by stripping its Authenticode signature and truncating the
//! trailing payload sections. Those sections are `.osrel`, `.cmdline`,
//! `.uname`, and `.linux`.

mod error;
mod pe;

use bootconfig::{Bootconfig, UkiCmdline};
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
    about = "Command-line construction and PE section removal for Bottlerocket Unified Kernel Images"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

/// Subcommands for UKI repack section removal and command line construction.
#[derive(Subcommand)]
enum Command {
    /// Keep the last assignment of each key in the drop-ins before generating binary bootconfig.
    MergeBootconfig {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
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
        Command::MergeBootconfig { input, output } => merge_bootconfig(input, output),
        Command::Cmdline {
            bootconfig,
            kernel_args,
            init_args,
            output,
        } => cmdline(bootconfig, kernel_args, init_args, output),
        Command::DeriveStub { uki, stub } => derive_stub(uki, stub),
    }
}

/// Merge the drop-in contributions in the bootconfig input text.
fn merge_bootconfig(input: &Path, output: &Path) -> Result<()> {
    let text = std::fs::read_to_string(input).with_whatever_context(|_| {
        format!(
            "Failed to read collected bootconfig drop-ins '{}'",
            input.display()
        )
    })?;
    let merged = bootconfig::merge_parameters(&text).with_whatever_context(|_| {
        format!("Failed to merge bootconfig drop-ins '{}'", input.display())
    })?;
    std::fs::write(output, merged)
        .with_whatever_context(|_| format!("Couldn't write file '{}'", output.display()))
}

fn cmdline(bootconfig: &Path, kernel_args: &str, init_args: &str, output: &Path) -> Result<()> {
    let data = std::fs::read(bootconfig).with_whatever_context(|_| {
        format!("Failed to read bootconfig '{}'", bootconfig.display())
    })?;
    let bootconfig_parameters = Bootconfig::decode(&data).with_whatever_context(|_| {
        format!("Failed to decode bootconfig '{}'", bootconfig.display())
    })?;
    let uki_cmdline = UkiCmdline::compose(&bootconfig_parameters, kernel_args, init_args)
        .whatever_context("Failed to construct UKI command line")?;
    std::fs::write(output, uki_cmdline.to_string())
        .with_whatever_context(|_| format!("Couldn't write file '{}'", output.display()))
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

#[cfg(test)]
mod tests {
    use super::*;

    // Encoded bootconfig generated as output of `bootconfig -a`
    fn encoded_bootconfig_data() -> Vec<u8> {
        let config = "kernel.fips = 1\n";
        let mut data = config.as_bytes().to_vec();
        data.push(0);
        data.resize(data.len().next_multiple_of(4), 0);
        let size = u32::try_from(data.len()).expect("bootconfig fits in u32");
        let checksum = data.iter().map(|&byte| u32::from(byte)).sum::<u32>();
        data.extend(size.to_le_bytes());
        data.extend(checksum.to_le_bytes());
        data.extend(b"#BOOTCONFIG\n");
        data
    }

    #[test]
    fn cmdline_joins_bootconfig_with_image_builder_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("bootconfig.data");
        let output = dir.path().join("cmdline");
        std::fs::write(&input, encoded_bootconfig_data()).unwrap();
        cmdline(&input, "root=/dev/dm-0", "quiet", &output).unwrap();
        assert_eq!(
            std::fs::read_to_string(output).unwrap(),
            "fips=1 root=/dev/dm-0 -- quiet"
        );
    }

    #[test]
    fn invalid_input_does_not_publish_cmdline_output() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("bad.data");
        let output = dir.path().join("cmdline");
        std::fs::write(&input, b"not a bootconfig").unwrap();
        assert!(cmdline(&input, "", "", &output).is_err());
        assert!(!output.exists());
    }

    #[test]
    fn merge_bootconfig_rewrites_dropins_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("bootconfig.in");
        std::fs::write(&input, "kernel.fips = 1\nkernel.fips = 0\n").unwrap();
        merge_bootconfig(&input, &input).unwrap();
        assert_eq!(
            std::fs::read_to_string(&input).unwrap(),
            "kernel.fips = 0\n"
        );
    }
}
