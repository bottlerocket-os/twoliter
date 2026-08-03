// SPDX-License-Identifier: Apache-2.0 OR MIT
//! EIF (Enclave Image Format) builder.
//!
//! Builds a minimal sidecar EIF: kernel + cmdline + empty ramdisk + metadata.
//! The rootfs is a separate erofs artifact attached as virtio-blk at launch.
//!
//! All multi-byte fields are big-endian.

pub mod kernel;

pub use kernel::KernelPrepError;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crc32fast::Hasher as Crc32Hasher;
use snafu::{ensure, ResultExt, Snafu};

const EIF_MAGIC: [u8; 4] = [0x2e, 0x65, 0x69, 0x66]; // ".eif"
const EIF_HDR_VERSION: u16 = 4;

const EIF_SECTION_KERNEL: u16 = 1;
const EIF_SECTION_CMDLINE: u16 = 2;
const EIF_SECTION_RAMDISK: u16 = 3;
const EIF_SECTION_METADATA: u16 = 5;

const EIF_ARCH_X86_64: u16 = 0;
const EIF_ARCH_AARCH64: u16 = 1;

const MAX_NUM_SECTIONS: usize = 32;
const EIF_HEADER_SIZE: usize = 548;
const EIF_CRC32_OFFSET: usize = EIF_HEADER_SIZE - 4;
const EIF_SECTION_HEADER_SIZE: usize = 12;

/// PCIE flag constants.
pub const EIF_HDR_FLAG_PCIE: u16 = 1 << 6;
pub const EIF_HDR_FLAG_PCIE_VIRTIO: u16 = 1 << 9;

/// Default PCIE flags for sidecar mode.
pub const DEFAULT_PCIE_FLAGS: u16 = EIF_HDR_FLAG_PCIE | EIF_HDR_FLAG_PCIE_VIRTIO;

/// Default kernel command line for block device boot.
pub const DEFAULT_CMDLINE: &str = "initcall_blacklist=i8042_init console=ttyS0 root=/dev/vda rw";

#[derive(Debug, Clone, Copy)]
pub enum TargetArch {
    X86_64,
    Aarch64,
}

impl TargetArch {
    /// Returns the EIF header architecture flag for this target.
    fn flags(self) -> u16 {
        match self {
            Self::X86_64 => EIF_ARCH_X86_64,
            Self::Aarch64 => EIF_ARCH_AARCH64,
        }
    }
}

impl std::str::FromStr for TargetArch {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Accept the canonical Linux `uname -m` spellings so callers can pass
        // `${ARCH}` directly from the buildsys env without translation.
        match s {
            "x86_64" | "amd64" => Ok(Self::X86_64),
            "aarch64" | "arm64" => Ok(Self::Aarch64),
            other => Err(format!(
                "unsupported target arch {other:?}: expected one of x86_64, aarch64"
            )),
        }
    }
}

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum EifError {
    #[snafu(display("failed to read kernel {}: {source}", path.display()))]
    ReadKernel {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("failed to write EIF {}: {source}", path.display()))]
    WriteOutput {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("kernel image is empty"))]
    EmptyKernel,

    #[snafu(display("failed to prepare kernel image: {source}"))]
    PrepareKernel { source: KernelPrepError },

    #[snafu(display("failed to write prepared kernel {}: {source}", path.display()))]
    WritePreparedKernel {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Build a minimal metadata JSON matching the schema expected by the NE hypervisor.
///
/// The schema (top-level keys and `BuildMetadata` sub-keys) matches
/// `aws-nitro-enclaves-image-format`. Values are left as empty strings where
/// we do not populate them; the keys themselves must be present.
///
/// NOTE: This intentionally omits the PCR/measurement fields produced by
/// the upstream AWS Nitro Enclaves CLI. Attestation of the resulting EIF
/// still works normally: the hypervisor measures the kernel, cmdline, and
/// (dm-verity-anchored) rootfs handoff into PCRs at launch. See
/// `README.md` for details.
fn build_metadata(
    build_tool_version: &str,
    kernel_version: &str,
    image_version: &str,
    build_time: &str,
) -> String {
    // `serde_json` handles escaping so we can safely embed values that may
    // contain characters requiring escaping (e.g. dashes, colons, quotes).
    //
    // Empty-string values are preserved (not omitted). The upstream
    // `EifIdentityInfo` / `EifBuildInfo` structs use `String`, not
    // `Option<String>`, so consumers expect the keys to always exist.
    serde_json::json!({
        "ImageName": "bottlerocket-sidecar",
        "ImageVersion": image_version,
        "BuildMetadata": {
            "BuildTime": build_time,
            "BuildTool": "twoliter",
            "BuildToolVersion": build_tool_version,
            "OperatingSystem": "Linux",
            "KernelVersion": kernel_version,
        },
        "DockerInfo": {},
        "CustomMetadata": serde_json::Value::Null,
    })
    .to_string()
}

/// Bundle of caller-supplied metadata fields for the EIF METADATA section.
///
/// Every field is optional (empty string = unknown); the JSON keys
/// themselves are always emitted so the NE hypervisor's `EifIdentityInfo`
/// / `EifBuildInfo` deserializers (which use `String`, not
/// `Option<String>`) accept the output.
#[derive(Debug, Default, Clone)]
pub struct MetadataFields<'a> {
    /// `BuildMetadata.BuildToolVersion` — semver of the tool producing the
    /// EIF (twoliter). rpm2eif forwards `TWOLITER_VERSION` here.
    pub build_tool_version: &'a str,
    /// `BuildMetadata.KernelVersion` — RPM `VERSION-RELEASE` of the kernel
    /// package that owns the vmlinuz. rpm2eif derives it from
    /// `rpm --whatprovides`.
    pub kernel_version: &'a str,
    /// `ImageVersion` (top-level, not nested under BuildMetadata) — the
    /// user-facing release version. Bottlerocket sets this to `VERSION_ID`
    /// (`1.63.0` etc.), matching upstream nitro-cli's `--image-version` flag.
    pub image_version: &'a str,
    /// `BuildMetadata.BuildTime` — RFC 3339 UTC timestamp string. Upstream
    /// nitro-cli uses `Utc::now()` here (non-reproducible). Our pipeline
    /// prefers determinism: rpm2eif passes the git commit timestamp
    /// (`BUILD_ID_TIMESTAMP`) formatted as RFC 3339, which is the same
    /// timestamp already baked into `KernelVersion`. Empty = "unknown".
    pub build_time: &'a str,
}

fn write_section(eif: &mut Vec<u8>, section_type: u16, data: &[u8]) {
    eif.extend_from_slice(&section_type.to_be_bytes());
    eif.extend_from_slice(&0u16.to_be_bytes()); // flags
    eif.extend_from_slice(&(data.len() as u64).to_be_bytes());
    eif.extend_from_slice(data);
}

/// Build and write an EIF to the specified output path.
///
/// `target_arch` selects the architecture flag written into the EIF header;
/// it must match the kernel image at `kernel_path` (cross-arch builds are
/// legitimate and expected in cross-compile pipelines).
///
/// `metadata` populates the METADATA section. Every field is optional
/// (empty string = unknown); the JSON keys themselves are always emitted so
/// the NE hypervisor's `EifIdentityInfo` / `EifBuildInfo` deserializers
/// (which use `String`, not `Option<String>`) accept the output.
#[allow(clippy::too_many_arguments)]
pub fn build_eif(
    kernel_path: &Path,
    cmdline: &str,
    output_path: &Path,
    default_mem: u64,
    default_cpus: u64,
    pcie_flags: u16,
    target_arch: TargetArch,
    metadata: &MetadataFields<'_>,
) -> Result<(), EifError> {
    let kernel_data = fs::read(kernel_path).context(ReadKernelSnafu { path: kernel_path })?;
    ensure!(!kernel_data.is_empty(), EmptyKernelSnafu);
    // Arch-specific pre-processing. On arm64 this unwraps EFI zboot images
    // (recent Bottlerocket kernel-kit ships `vmlinuz` as a zboot-wrapped
    // zstd-compressed `Image`) so Firecracker's PE loader sees the flat
    // arm64 `Image` it expects. On x86_64 this is a no-op.
    let kernel_data =
        kernel::prepare_kernel(kernel_data, target_arch).context(PrepareKernelSnafu)?;

    let cmdline_data = cmdline.as_bytes();
    // Empty RAMDISK section (12-byte header, zero payload). This is well-formed
    // per the EIF spec: `EifSectionRamdisk` has no minimum-count requirement,
    // only an ordering constraint (must follow the kernel section). Bottlerocket
    // sidecar EIFs mount the rootfs from a virtio-blk device with dm-verity, so
    // no initramfs is needed. NOTE: stock `nitro-cli`-produced EIFs always have
    // two ramdisks (bootstrap + customer); consumers that assume that convention
    // (e.g. anything computing PCR2 over concatenated ramdisks) will see empty
    // input here. Our custom launcher handles this correctly.
    let ramdisk_data: &[u8] = &[];
    let metadata_json = build_metadata(
        metadata.build_tool_version,
        metadata.kernel_version,
        metadata.image_version,
        metadata.build_time,
    );
    let metadata_data = metadata_json.as_bytes();

    let num_sections: u16 = 4;
    debug_assert!(
        (num_sections as usize) <= MAX_NUM_SECTIONS,
        "num_sections must fit within the EIF v4 header's section table"
    );

    // Calculate total size
    let sections_size = (EIF_SECTION_HEADER_SIZE * num_sections as usize)
        + kernel_data.len()
        + cmdline_data.len()
        + ramdisk_data.len()
        + metadata_data.len();
    let total_size = EIF_HEADER_SIZE + sections_size;

    let mut eif = Vec::with_capacity(total_size);

    // --- Header ---
    eif.extend_from_slice(&EIF_MAGIC);
    eif.extend_from_slice(&EIF_HDR_VERSION.to_be_bytes());
    eif.extend_from_slice(&(target_arch.flags() | pcie_flags).to_be_bytes());
    eif.extend_from_slice(&default_mem.to_be_bytes());
    eif.extend_from_slice(&default_cpus.to_be_bytes());
    eif.extend_from_slice(&0u16.to_be_bytes()); // reserved
    eif.extend_from_slice(&num_sections.to_be_bytes());

    // Section offsets
    let kernel_offset = EIF_HEADER_SIZE as u64;
    let cmdline_offset = kernel_offset + EIF_SECTION_HEADER_SIZE as u64 + kernel_data.len() as u64;
    let ramdisk_offset =
        cmdline_offset + EIF_SECTION_HEADER_SIZE as u64 + cmdline_data.len() as u64;
    let metadata_offset =
        ramdisk_offset + EIF_SECTION_HEADER_SIZE as u64 + ramdisk_data.len() as u64;

    let offsets = [
        kernel_offset,
        cmdline_offset,
        ramdisk_offset,
        metadata_offset,
    ];
    for i in 0..MAX_NUM_SECTIONS {
        let val = if i < offsets.len() { offsets[i] } else { 0 };
        eif.extend_from_slice(&val.to_be_bytes());
    }

    // Section sizes
    let sizes = [
        kernel_data.len() as u64,
        cmdline_data.len() as u64,
        ramdisk_data.len() as u64,
        metadata_data.len() as u64,
    ];
    for i in 0..MAX_NUM_SECTIONS {
        let val = if i < sizes.len() { sizes[i] } else { 0 };
        eif.extend_from_slice(&val.to_be_bytes());
    }

    eif.extend_from_slice(&0u32.to_be_bytes()); // unused
    eif.extend_from_slice(&0u32.to_be_bytes()); // crc32 placeholder

    debug_assert_eq!(eif.len(), EIF_HEADER_SIZE);

    // --- Sections ---
    write_section(&mut eif, EIF_SECTION_KERNEL, &kernel_data);
    write_section(&mut eif, EIF_SECTION_CMDLINE, cmdline_data);
    write_section(&mut eif, EIF_SECTION_RAMDISK, ramdisk_data);
    write_section(&mut eif, EIF_SECTION_METADATA, metadata_data);

    // Compute CRC32 (exclude CRC field itself)
    let mut hasher = Crc32Hasher::new();
    hasher.update(&eif[..EIF_CRC32_OFFSET]);
    hasher.update(&eif[EIF_CRC32_OFFSET + 4..]);
    let crc = hasher.finalize();
    eif[EIF_CRC32_OFFSET..EIF_CRC32_OFFSET + 4].copy_from_slice(&crc.to_be_bytes());

    // Write to file
    let mut file = fs::File::create(output_path).context(WriteOutputSnafu { path: output_path })?;
    file.write_all(&eif)
        .context(WriteOutputSnafu { path: output_path })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_build_eif() {
        let dir = tempfile::tempdir().unwrap();
        let kernel = dir.path().join("kernel");
        let output = dir.path().join("test.eif");

        fs::write(&kernel, b"FAKE_KERNEL").unwrap();

        build_eif(
            &kernel,
            "console=ttyS0",
            &output,
            512 << 20,
            2,
            DEFAULT_PCIE_FLAGS,
            TargetArch::X86_64,
            &MetadataFields {
                build_tool_version: "0.20.0",
                kernel_version: "6.1.152-0.br1",
                image_version: "",
                build_time: "",
            },
        )
        .unwrap();

        let eif = fs::read(&output).unwrap();
        assert_eq!(&eif[0..4], &EIF_MAGIC);
        assert_eq!(u16::from_be_bytes([eif[4], eif[5]]), EIF_HDR_VERSION);

        // Verify CRC
        let stored_crc = u32::from_be_bytes(
            eif[EIF_CRC32_OFFSET..EIF_CRC32_OFFSET + 4]
                .try_into()
                .unwrap(),
        );
        let mut hasher = Crc32Hasher::new();
        hasher.update(&eif[..EIF_CRC32_OFFSET]);
        hasher.update(&eif[EIF_CRC32_OFFSET + 4..]);
        assert_eq!(hasher.finalize(), stored_crc);
    }

    #[test]
    fn test_empty_kernel_error() {
        let dir = tempfile::tempdir().unwrap();
        let kernel = dir.path().join("kernel");
        let output = dir.path().join("test.eif");

        fs::write(&kernel, b"").unwrap();

        let result = build_eif(
            &kernel,
            "",
            &output,
            512 << 20,
            2,
            0,
            TargetArch::X86_64,
            &MetadataFields::default(),
        );
        assert!(matches!(result, Err(EifError::EmptyKernel)));
    }

    #[test]
    fn test_metadata_schema() {
        // Verify the metadata JSON has all keys the NE hypervisor expects,
        // including nested BuildMetadata sub-keys, and that caller-supplied
        // values are embedded verbatim.
        let json = build_metadata("0.20.0", "6.1.152-0.br1", "1.63.0", "2026-07-28T22:20:54Z");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v.get("ImageName").is_some());
        assert_eq!(v.get("ImageVersion").unwrap(), "1.63.0");
        assert!(v.get("DockerInfo").is_some());
        assert!(v.get("CustomMetadata").is_some());
        let bm = v.get("BuildMetadata").unwrap();
        assert_eq!(bm.get("BuildTool").unwrap(), "twoliter");
        assert_eq!(bm.get("BuildToolVersion").unwrap(), "0.20.0");
        assert_eq!(bm.get("KernelVersion").unwrap(), "6.1.152-0.br1");
        assert_eq!(bm.get("OperatingSystem").unwrap(), "Linux");
        assert_eq!(bm.get("BuildTime").unwrap(), "2026-07-28T22:20:54Z");
    }

    #[test]
    fn test_metadata_empty_values() {
        // Empty strings must still round-trip through the schema (keys present,
        // values empty) rather than being omitted.
        let json = build_metadata("", "", "", "");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v.get("ImageVersion").unwrap(), "");
        let bm = v.get("BuildMetadata").unwrap();
        assert_eq!(bm.get("BuildToolVersion").unwrap(), "");
        assert_eq!(bm.get("KernelVersion").unwrap(), "");
        assert_eq!(bm.get("BuildTime").unwrap(), "");
    }

    #[test]
    fn test_target_arch_from_str() {
        use std::str::FromStr;
        assert!(matches!(
            TargetArch::from_str("x86_64").unwrap(),
            TargetArch::X86_64
        ));
        assert!(matches!(
            TargetArch::from_str("amd64").unwrap(),
            TargetArch::X86_64
        ));
        assert!(matches!(
            TargetArch::from_str("aarch64").unwrap(),
            TargetArch::Aarch64
        ));
        assert!(matches!(
            TargetArch::from_str("arm64").unwrap(),
            TargetArch::Aarch64
        ));
        assert!(TargetArch::from_str("mips").is_err());
        assert!(TargetArch::from_str("").is_err());
    }

    #[test]
    fn test_header_arch_flag_is_target_not_host() {
        // Building for a target arch different from cfg!(target_arch=...) must
        // still yield the *target* arch flag in the EIF header. This guards
        // against regressions where the arch is derived from the build host.
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("test.eif");

        // x86_64 pass-through: any non-empty bytes work.
        let x86_kernel = dir.path().join("x86-kernel");
        fs::write(&x86_kernel, b"FAKE_KERNEL").unwrap();

        // aarch64 shape-check: must look like an arm64 PE Image (MZ + ARM\x64
        // at offset 56). Build the smallest plausible one.
        let arm_kernel = dir.path().join("arm-kernel");
        let mut arm_bytes = vec![0u8; 60];
        arm_bytes[0..2].copy_from_slice(b"MZ");
        arm_bytes[56..60].copy_from_slice(b"ARM\x64");
        fs::write(&arm_kernel, &arm_bytes).unwrap();

        for (arch, expected_low_bits, kernel_path) in [
            (TargetArch::X86_64, EIF_ARCH_X86_64, &x86_kernel),
            (TargetArch::Aarch64, EIF_ARCH_AARCH64, &arm_kernel),
        ] {
            build_eif(
                kernel_path,
                "",
                &output,
                512 << 20,
                2,
                0,
                arch,
                &MetadataFields::default(),
            )
            .unwrap();
            let eif = fs::read(&output).unwrap();
            // flags is u16 at offset 6; PCIE bits are 0 here so the full value
            // equals just the arch bits.
            let flags = u16::from_be_bytes([eif[6], eif[7]]);
            assert_eq!(flags, expected_low_bits, "arch={arch:?}");
        }
    }

    #[test]
    fn test_arm64_non_pe_kernel_is_rejected() {
        // A verbatim gzip-compressed vmlinuz (or anything else that isn't a
        // flat PE-wrapped arm64 Image) must be rejected up-front with a
        // KernelPrepError, rather than producing an EIF that Firecracker's
        // PE loader will reject at launch. This is the regression the
        // KernelPrep pass was introduced to prevent.
        let dir = tempfile::tempdir().unwrap();
        let kernel = dir.path().join("vmlinuz.gz");
        // Gzip magic + a bit of body — decidedly not an MZ/ARM\x64 header.
        fs::write(&kernel, [0x1f, 0x8b, 0x08, 0x00, b'j', b'u', b'n', b'k']).unwrap();
        let output = dir.path().join("test.eif");

        let err = build_eif(
            &kernel,
            "",
            &output,
            512 << 20,
            2,
            0,
            TargetArch::Aarch64,
            &MetadataFields::default(),
        )
        .unwrap_err();
        assert!(matches!(err, EifError::PrepareKernel { .. }), "err={err:?}");
    }
}
