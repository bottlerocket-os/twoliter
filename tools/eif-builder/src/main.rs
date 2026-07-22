// SPDX-License-Identifier: Apache-2.0 OR MIT

use clap::Parser;
use eif_builder::kernel::prepare_kernel;
use eif_builder::{
    build_eif, EifError, PrepareKernelSnafu, ReadKernelSnafu, TargetArch, WritePreparedKernelSnafu,
    DEFAULT_CMDLINE, DEFAULT_PCIE_FLAGS,
};
use snafu::{report, ResultExt};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "eif-builder", about = "Build a minimal sidecar EIF")]
struct Args {
    /// Path to the kernel image to embed. The accepted format is
    /// arch-dependent; see `README.md` for details.
    #[arg(long)]
    kernel: PathBuf,

    /// Kernel command line
    #[arg(long, default_value = DEFAULT_CMDLINE)]
    cmdline: String,

    /// Output EIF path
    #[arg(long)]
    output: PathBuf,

    /// Default memory in bytes
    #[arg(long, default_value_t = 512 * 1024 * 1024)]
    mem: u64,

    /// Default vCPU count
    #[arg(long, default_value_t = 2)]
    cpus: u64,

    /// PCIE flags (hex; `0x` / `0X` prefix optional)
    #[arg(long, default_value_t = DEFAULT_PCIE_FLAGS, value_parser = parse_hex_u16)]
    pcie_flags: u16,

    /// Target architecture of the kernel image being packaged (x86_64|amd64
    /// or aarch64|arm64). Must match the kernel image; it is never inferred
    /// from the build host. See `README.md`.
    #[arg(long)]
    arch: TargetArch,

    /// Build tool version to embed in EIF metadata (populates
    /// `BuildMetadata.BuildToolVersion`). Optional; empty by default.
    #[arg(long, default_value = "")]
    build_tool_version: String,

    /// Kernel version to embed in EIF metadata (populates
    /// `BuildMetadata.KernelVersion`). Optional; empty by default.
    #[arg(long, default_value = "")]
    kernel_version: String,

    /// Optional: also write the *prepared* kernel bytes (the exact byte
    /// stream embedded in the EIF's kernel section) to this path. Intended
    /// for local dev smoke-tests; the EIF itself is written unchanged. See
    /// `README.md`.
    #[arg(long)]
    out_prepared_kernel: Option<PathBuf>,
}

fn parse_hex_u16(s: &str) -> Result<u16, String> {
    let s = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    u16::from_str_radix(s, 16).map_err(|e| e.to_string())
}

/// Re-read the input kernel, run `prepare_kernel`, and write the flat bytes
/// out. Used only for the `--out-prepared-kernel` dev-only knob; see the
/// clap arg doc for rationale.
fn write_prepared_kernel(
    kernel_path: &Path,
    out_path: &Path,
    arch: TargetArch,
) -> Result<(), EifError> {
    let kernel_data = std::fs::read(kernel_path).context(ReadKernelSnafu { path: kernel_path })?;
    let prepared = prepare_kernel(kernel_data, arch).context(PrepareKernelSnafu)?;
    std::fs::write(out_path, &prepared).context(WritePreparedKernelSnafu { path: out_path })?;
    Ok(())
}

#[report]
fn main() -> Result<(), EifError> {
    let args = Args::parse();
    build_eif(
        &args.kernel,
        &args.cmdline,
        &args.output,
        args.mem,
        args.cpus,
        args.pcie_flags,
        args.arch,
        &args.build_tool_version,
        &args.kernel_version,
    )?;
    println!("Created {}", args.output.display());

    // Optional: dump the prepared kernel bytes for consumers that need a
    // flat, PE-loadable image (see `README.md`). We re-read + re-prepare
    // from the input kernel rather than plumbing the intermediate buffer
    // out of `build_eif`; it is cheap (megabytes), avoids widening the API
    // for a dev-only knob, and guarantees we write exactly the bytes the
    // EIF embedded.
    if let Some(out_path) = args.out_prepared_kernel.as_deref() {
        write_prepared_kernel(&args.kernel, out_path, args.arch)?;
        println!("Wrote prepared kernel to {}", out_path.display());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_variants() {
        assert_eq!(parse_hex_u16("0240").unwrap(), 0x0240);
        assert_eq!(parse_hex_u16("0x0240").unwrap(), 0x0240);
        assert_eq!(parse_hex_u16("0X0240").unwrap(), 0x0240);
        assert_eq!(parse_hex_u16("FFFF").unwrap(), 0xFFFF);
        assert_eq!(parse_hex_u16("0").unwrap(), 0);
    }

    #[test]
    fn parse_hex_rejects_overflow_and_empty() {
        assert!(parse_hex_u16("10000").is_err());
        assert!(parse_hex_u16("").is_err());
        assert!(parse_hex_u16("zz").is_err());
    }
}
