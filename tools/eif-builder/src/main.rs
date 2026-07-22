// SPDX-License-Identifier: Apache-2.0 OR MIT

use clap::{Parser, Subcommand};
use eif_builder::kernel::prepare_kernel;
use eif_builder::signer::{KmsSigner, LocalSigner};
use eif_builder::{
    build_eif, build_signed_eif, describe_eif, resign_eif, EifError, MetadataFields,
    PrepareKernelSnafu, ReadCertSnafu, ReadKernelSnafu, ReadKeySnafu, SignerBuildSnafu, TargetArch,
    WritePreparedKernelSnafu, DEFAULT_CMDLINE, DEFAULT_PCIE_FLAGS,
};
use snafu::{report, ResultExt};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "eif-builder", about = "Build a minimal sidecar EIF")]
struct Args {
    /// Subcommand. When omitted, the top-level flags are used to run the
    /// implicit "build" command; this preserves backward compatibility with
    /// existing callers (rpm2eif, eif2eif) that pass flags at the top level.
    #[command(subcommand)]
    command: Option<Command>,

    // ---------------- top-level "build" flags ----------------
    /// Path to the kernel image to embed. The accepted format is
    /// arch-dependent; see `README.md` for details. Required when no
    /// subcommand is given (implicit `build`).
    #[arg(long)]
    kernel: Option<PathBuf>,

    /// Kernel command line
    #[arg(long, default_value = DEFAULT_CMDLINE)]
    cmdline: String,

    /// Output EIF path. Required when no subcommand is given.
    #[arg(long)]
    output: Option<PathBuf>,

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
    /// from the build host. Required when no subcommand is given. See
    /// `README.md`.
    #[arg(long)]
    arch: Option<TargetArch>,

    /// Build tool version to embed in EIF metadata (populates
    /// `BuildMetadata.BuildToolVersion`). Optional; empty by default.
    #[arg(long, default_value = "")]
    build_tool_version: String,

    /// Kernel version to embed in EIF metadata (populates
    /// `BuildMetadata.KernelVersion`). Optional; empty by default.
    #[arg(long, default_value = "")]
    kernel_version: String,

    /// User-facing image version to embed in EIF metadata (populates the
    /// top-level `ImageVersion` field). rpm2eif passes `VERSION_ID`
    /// (Bottlerocket release, e.g. `1.63.0`); eif2eif carries forward the
    /// value from the input EIF. Optional; empty by default.
    #[arg(long, default_value = "")]
    image_version: String,

    /// Build time to embed in EIF metadata as RFC 3339 UTC
    /// (populates `BuildMetadata.BuildTime`). Optional; empty by default.
    /// Callers that want reproducible EIFs should pass a fixed timestamp
    /// derived from a stable input (rpm2eif uses `BUILD_ID_TIMESTAMP`, the
    /// git commit Unix seconds, which is the same timestamp already baked
    /// into `KernelVersion`). Upstream nitro-cli uses `Utc::now()` here,
    /// but we do not default to that on purpose — determinism is a
    /// buildsys-wide convention.
    #[arg(long, default_value = "")]
    build_time: String,

    /// Optional: also write the *prepared* kernel bytes (the exact byte
    /// stream embedded in the EIF's kernel section) to this path. Intended
    /// for local dev smoke-tests; the EIF itself is written unchanged. See
    /// `README.md`.
    #[arg(long)]
    out_prepared_kernel: Option<PathBuf>,

    /// PEM-encoded X.509 certificate used to sign PCR0. Required for a
    /// signed EIF. When both `--signing-cert` and `--signing-key` are
    /// present, the local-key signing path is used. When
    /// `--signing-cert` is present alongside `--kms-key-id`, the KMS
    /// path is used and this cert is embedded into the CBOR signature
    /// section verbatim (the private key never leaves KMS). Passing
    /// `--signing-cert` alone (without a signing backend) is an error;
    /// absence of all three flags produces an unsigned EIF.
    #[arg(long, requires = "signing_backend")]
    signing_cert: Option<PathBuf>,

    /// PEM-encoded ECDSA private key (P-256 or P-384) used to sign PCR0.
    /// Mutually exclusive with `--kms-key-id`; requires `--signing-cert`.
    #[arg(long, requires = "signing_cert", group = "signing_backend")]
    signing_key: Option<PathBuf>,

    /// AWS KMS key ID or ARN used to sign PCR0. Mutually exclusive with
    /// `--signing-key`; requires `--signing-cert` (the CBOR signature
    /// section carries an X.509 that KMS itself does not manage).
    /// Credentials are picked up from the ambient AWS environment;
    /// region falls back to the ambient chain when `--region` is unset
    /// but that chain is empty inside the buildkit sandbox, so callers
    /// building off-EC2 must pass `--region` explicitly (buildsys
    /// forwards `Infra.toml [eif].signing_key.kms.region` for this).
    #[arg(long, requires = "signing_cert", group = "signing_backend")]
    kms_key_id: Option<String>,

    /// AWS region for the KMS `Sign` call. Required for KMS-signed builds
    /// inside the buildkit sandbox (no ambient region source exists
    /// there); ignored for the local-key path and for unsigned builds.
    #[arg(long, requires = "kms_key_id")]
    region: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Re-sign an existing EIF in place: rewrite only the SIGNATURE section
    /// (and header CRC). Kernel, cmdline, ramdisk, and metadata sections
    /// are byte-preserved. PCR0 is recomputed from the input's bytes, so
    /// the new signature covers the same measurement the enclave sees.
    Resign(ResignArgs),

    /// Print a JSON summary of an EIF's contents on stdout.
    ///
    /// The METADATA section is decoded (JSON), so downstream tooling can
    /// pull out fields with `jq .metadata.BuildMetadata.KernelVersion`.
    /// The primary consumer is `eif2eif`, which needs to carry
    /// `KernelVersion` and `BuildToolVersion` forward across a repack (rpm2eif
    /// reads them from the source rootfs' RPM DB; eif2eif has no rootfs to
    /// query, so it reads them out of the input EIF instead).
    Describe(DescribeArgs),
}

#[derive(clap::Args)]
struct ResignArgs {
    /// Path to the input EIF.
    #[arg(long)]
    input: PathBuf,

    /// Path to write the resigned EIF to. May equal `--input` (the file
    /// is read fully before being rewritten).
    #[arg(long)]
    output: PathBuf,

    /// PEM-encoded X.509 certificate to embed in the new SIGNATURE
    /// section. Required; `resign` never produces an unsigned EIF.
    #[arg(long, requires = "signing_backend")]
    signing_cert: PathBuf,

    /// PEM-encoded ECDSA private key. Mutually exclusive with
    /// `--kms-key-id`.
    #[arg(long, group = "signing_backend")]
    signing_key: Option<PathBuf>,

    /// AWS KMS key ID or ARN. Mutually exclusive with `--signing-key`.
    #[arg(long, group = "signing_backend")]
    kms_key_id: Option<String>,

    /// AWS region for the KMS `Sign` call. See the top-level `--region`
    /// doc for when this is required.
    #[arg(long, requires = "kms_key_id")]
    region: Option<String>,
}

#[derive(clap::Args)]
struct DescribeArgs {
    /// Path to the input EIF.
    #[arg(long)]
    input: PathBuf,

    /// Emit the JSON compactly (single line, no indentation). Useful for
    /// piping into `jq`; the default is pretty-printed for human reading.
    #[arg(long)]
    compact: bool,
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

    match args.command {
        Some(Command::Resign(r)) => return run_resign(r),
        Some(Command::Describe(d)) => return run_describe(d),
        None => {} // fall through to the implicit "build" path.
    }

    // Implicit `build` path — the top-level flags. Validate required flags
    // here since we cannot express "required unless subcommand" via clap
    // attributes when the subcommand is itself optional (Option<Command>).
    let (kernel, output, arch) = match (args.kernel, args.output, args.arch) {
        (Some(k), Some(o), Some(a)) => (k, o, a),
        _ => {
            eprintln!(
                "error: --kernel, --output, and --arch are required when no subcommand is given"
            );
            std::process::exit(2);
        }
    };

    let metadata = MetadataFields {
        build_tool_version: &args.build_tool_version,
        kernel_version: &args.kernel_version,
        image_version: &args.image_version,
        build_time: &args.build_time,
    };

    // Dispatch: three states — unsigned, local-signed, KMS-signed. Clap has
    // already enforced the mutual-exclusion constraints, so we only need to
    // build the right signer here.
    match (
        args.signing_cert.as_deref(),
        args.signing_key.as_deref(),
        args.kms_key_id.as_deref(),
    ) {
        (None, None, None) => {
            build_eif(
                &kernel,
                &args.cmdline,
                &output,
                args.mem,
                args.cpus,
                args.pcie_flags,
                arch,
                &metadata,
            )?;
        }
        (Some(cert_path), Some(key_path), None) => {
            let cert_pem = std::fs::read(cert_path).context(ReadCertSnafu)?;
            let key_pem = std::fs::read(key_path).context(ReadKeySnafu)?;
            let signer = LocalSigner::from_pem(&cert_pem, &key_pem).context(SignerBuildSnafu)?;
            build_signed_eif(
                &kernel,
                &args.cmdline,
                &output,
                args.mem,
                args.cpus,
                args.pcie_flags,
                arch,
                &metadata,
                &signer,
            )?;
        }
        (Some(cert_path), None, Some(key_id)) => {
            let cert_pem = std::fs::read(cert_path).context(ReadCertSnafu)?;
            let signer = KmsSigner::from_key_id(key_id.to_string(), cert_pem, args.region.clone())
                .context(SignerBuildSnafu)?;
            build_signed_eif(
                &kernel,
                &args.cmdline,
                &output,
                args.mem,
                args.cpus,
                args.pcie_flags,
                arch,
                &metadata,
                &signer,
            )?;
        }
        // Every other tuple is impossible: `--signing-cert` requires the
        // `signing_backend` ArgGroup (so a `Some, None, None` cert-only
        // input errors at parse), and `--signing-key`/`--kms-key-id` each
        // require `--signing-cert` (so a `None, Some, ...` errors at parse).
        // The group's `multiple(false)` default also rules out
        // `Some, Some, Some`. Guarded here anyway so a future clap-attr
        // regression fails at build time via clippy rather than at runtime.
        _ => unreachable!("clap ArgGroup enforces the (cert, {{key|kms}}) invariant"),
    }
    println!("Created {}", output.display());

    // Optional: dump the prepared kernel bytes for consumers that need a
    // flat, PE-loadable image (see `README.md`). We re-read + re-prepare
    // from the input kernel rather than plumbing the intermediate buffer
    // out of `build_eif`; it is cheap (megabytes), avoids widening the API
    // for a dev-only knob, and guarantees we write exactly the bytes the
    // EIF embedded.
    if let Some(out_path) = args.out_prepared_kernel.as_deref() {
        write_prepared_kernel(&kernel, out_path, arch)?;
        println!("Wrote prepared kernel to {}", out_path.display());
    }

    Ok(())
}

/// Dispatch the `resign` subcommand. Builds the appropriate signer from the
/// arg group and calls [`resign_eif`].
fn run_resign(args: ResignArgs) -> Result<(), EifError> {
    let cert_pem = std::fs::read(&args.signing_cert).context(ReadCertSnafu)?;
    // clap's `signing_backend` group has `required = true` via the
    // `requires = "signing_backend"` attribute on `--signing-cert`, so one
    // of --signing-key or --kms-key-id is always present here.
    match (args.signing_key.as_deref(), args.kms_key_id.as_deref()) {
        (Some(key_path), None) => {
            let key_pem = std::fs::read(key_path).context(ReadKeySnafu)?;
            let signer = LocalSigner::from_pem(&cert_pem, &key_pem).context(SignerBuildSnafu)?;
            resign_eif(&args.input, &args.output, &signer)?;
        }
        (None, Some(key_id)) => {
            let signer = KmsSigner::from_key_id(key_id.to_string(), cert_pem, args.region.clone())
                .context(SignerBuildSnafu)?;
            resign_eif(&args.input, &args.output, &signer)?;
        }
        _ => unreachable!("clap ArgGroup enforces exactly one of --signing-key/--kms-key-id"),
    }
    println!(
        "Resigned {} -> {}",
        args.input.display(),
        args.output.display()
    );
    Ok(())
}

/// Dispatch the `describe` subcommand. Prints a JSON summary of `--input` to
/// stdout. Writes nothing to stderr on success so the output can be piped
/// directly into `jq`.
fn run_describe(args: DescribeArgs) -> Result<(), EifError> {
    let value = describe_eif(&args.input)?;
    let out = if args.compact {
        serde_json::to_string(&value)
    } else {
        serde_json::to_string_pretty(&value)
    }
    .expect("serde_json::to_string on a Value cannot fail");
    println!("{out}");
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
