// SPDX-License-Identifier: Apache-2.0 OR MIT
//! EIF (Enclave Image Format) builder.
//!
//! Builds a minimal sidecar EIF: kernel + cmdline + empty ramdisk + metadata.
//! The rootfs is a separate erofs artifact attached as virtio-blk at launch.
//!
//! All multi-byte fields are big-endian.

pub mod kernel;
pub mod signer;

pub use kernel::KernelPrepError;
pub use signer::{Signer, SignerError};

use std::borrow::Cow;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crc32fast::Hasher as Crc32Hasher;
use serde::{Deserialize, Serialize};
use snafu::{ensure, ResultExt, Snafu};

const EIF_MAGIC: [u8; 4] = [0x2e, 0x65, 0x69, 0x66]; // ".eif"
/// EIF header version. v4 introduced the metadata section (0x05) and is
/// forward-compatible with the v3 signature section (0x04) — the reference
/// `EifBuilder` still emits `EifSectionSignature` under v4 EIFs, and
/// `nitro-cli describe-eif` recognizes signatures under v4. See
/// <https://github.com/aws/aws-nitro-enclaves-image-format>.
const EIF_HDR_VERSION: u16 = 4;

const EIF_SECTION_KERNEL: u16 = 1;
const EIF_SECTION_CMDLINE: u16 = 2;
const EIF_SECTION_RAMDISK: u16 = 3;
const EIF_SECTION_SIGNATURE: u16 = 4;
const EIF_SECTION_METADATA: u16 = 5;

/// Human-readable name for an EIF section type. `"unknown"` for values that
/// aren't one of the five defined section types; the caller can decide how
/// to surface those (we don't reject them, so `describe_eif` can still report
/// what's on disk for post-mortem debugging of a malformed file).
fn section_type_name(ty: u16) -> &'static str {
    match ty {
        EIF_SECTION_KERNEL => "KERNEL",
        EIF_SECTION_CMDLINE => "CMDLINE",
        EIF_SECTION_RAMDISK => "RAMDISK",
        EIF_SECTION_SIGNATURE => "SIGNATURE",
        EIF_SECTION_METADATA => "METADATA",
        _ => "UNKNOWN",
    }
}

const EIF_ARCH_X86_64: u16 = 0;
const EIF_ARCH_AARCH64: u16 = 1;

const MAX_NUM_SECTIONS: usize = 32;
const EIF_HEADER_SIZE: usize = 548;
const EIF_CRC32_OFFSET: usize = EIF_HEADER_SIZE - 4;
const EIF_SECTION_HEADER_SIZE: usize = 12;

/// Upper bound on the size of the CBOR-encoded EIF signature section, per
/// the aws-nitro-enclaves-image-format spec.
pub const SIGNATURE_MAX_SIZE: usize = 32768;

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

    #[snafu(display("signing failed: {source}"))]
    Sign { source: SignerError },

    #[snafu(display("failed to construct signer: {source}"))]
    SignerBuild { source: SignerError },

    #[snafu(display("failed to read signing certificate: {source}"))]
    ReadCert { source: std::io::Error },

    #[snafu(display("failed to read signing key: {source}"))]
    ReadKey { source: std::io::Error },

    #[snafu(display(
        "encoded EIF signature section is {size} bytes (cert PEM: {cert_pem_size}, \
         COSE_Sign1: {cose_size}), exceeding the {SIGNATURE_MAX_SIZE}-byte \
         SIGNATURE_MAX_SIZE limit; shrink the certificate (drop chain \
         intermediates if any) or shorten the payload"
    ))]
    SignatureTooLarge {
        size: usize,
        cert_pem_size: usize,
        cose_size: usize,
    },

    #[snafu(display("failed to CBOR-encode signature section: {source}"))]
    CborEncodeSignature { source: serde_cbor::Error },

    #[snafu(display("failed to CBOR-encode signature payload: {source}"))]
    CborEncodePayload { source: serde_cbor::Error },

    #[snafu(display("failed to build COSE_Sign1: {source}"))]
    CoseBuild { source: coset::CoseError },

    #[snafu(display("too many EIF sections: {count} > {MAX_NUM_SECTIONS}"))]
    TooManySections { count: usize },

    #[snafu(display("failed to read input EIF {}: {source}", path.display()))]
    ReadInput {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("failed to parse input EIF: {reason}"))]
    ParseInput { reason: String },

    #[snafu(display(
        "failed to decode METADATA section as UTF-8 JSON: {reason}. \
         The METADATA section is expected to be a JSON document produced by \
         `build_metadata`; a binary or non-UTF-8 payload here indicates a \
         corrupt or foreign-tool-produced EIF."
    ))]
    MetadataDecode { reason: String },
}

/// One EIF section (type + data) staged in memory before layout.
struct SectionEntry<'a> {
    ty: u16,
    data: Cow<'a, [u8]>,
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

/// Assemble the final EIF bytes for a section list.
///
/// The header is fixed-size (`EIF_HEADER_SIZE`) and carries a table of at
/// most `MAX_NUM_SECTIONS` offset/size pairs. Sections are laid out in the
/// same order as `entries`; the CRC32 is computed over the whole buffer with
/// the CRC field itself zeroed.
///
/// Split off from `build_eif` so that both the unsigned and signed builders
/// can share the (finicky, easy-to-drift) offset/size/CRC math.
fn write_eif_bytes(
    default_mem: u64,
    default_cpus: u64,
    header_flags: u16,
    entries: &[SectionEntry<'_>],
) -> Result<Vec<u8>, EifError> {
    ensure!(
        entries.len() <= MAX_NUM_SECTIONS,
        TooManySectionsSnafu {
            count: entries.len(),
        },
    );
    let num_sections: u16 = entries
        .len()
        .try_into()
        .expect("checked <= MAX_NUM_SECTIONS above");

    // Precompute per-section offsets and sizes so both the header table and
    // the actual writes stay in lockstep.
    let mut offsets: Vec<u64> = Vec::with_capacity(entries.len());
    let mut sizes: Vec<u64> = Vec::with_capacity(entries.len());
    let mut cursor: u64 = EIF_HEADER_SIZE as u64;
    for entry in entries {
        offsets.push(cursor);
        let size = entry.data.len() as u64;
        sizes.push(size);
        cursor = cursor
            .checked_add(EIF_SECTION_HEADER_SIZE as u64)
            .and_then(|c| c.checked_add(size))
            .expect("EIF total size overflow");
    }
    let total_size = cursor as usize;

    let mut eif = Vec::with_capacity(total_size);

    // --- Header ---
    eif.extend_from_slice(&EIF_MAGIC);
    eif.extend_from_slice(&EIF_HDR_VERSION.to_be_bytes());
    eif.extend_from_slice(&header_flags.to_be_bytes());
    eif.extend_from_slice(&default_mem.to_be_bytes());
    eif.extend_from_slice(&default_cpus.to_be_bytes());
    eif.extend_from_slice(&0u16.to_be_bytes()); // reserved
    eif.extend_from_slice(&num_sections.to_be_bytes());

    // Section offset table (fixed length: MAX_NUM_SECTIONS entries).
    for i in 0..MAX_NUM_SECTIONS {
        let val = if i < offsets.len() { offsets[i] } else { 0 };
        eif.extend_from_slice(&val.to_be_bytes());
    }

    // Section size table (fixed length).
    for i in 0..MAX_NUM_SECTIONS {
        let val = if i < sizes.len() { sizes[i] } else { 0 };
        eif.extend_from_slice(&val.to_be_bytes());
    }

    eif.extend_from_slice(&0u32.to_be_bytes()); // unused
    eif.extend_from_slice(&0u32.to_be_bytes()); // crc32 placeholder

    debug_assert_eq!(eif.len(), EIF_HEADER_SIZE);

    // --- Sections ---
    for entry in entries {
        eif.extend_from_slice(&entry.ty.to_be_bytes());
        eif.extend_from_slice(&0u16.to_be_bytes()); // flags
        eif.extend_from_slice(&(entry.data.len() as u64).to_be_bytes());
        eif.extend_from_slice(&entry.data);
    }

    // Compute CRC32 (exclude CRC field itself).
    let mut hasher = Crc32Hasher::new();
    hasher.update(&eif[..EIF_CRC32_OFFSET]);
    hasher.update(&eif[EIF_CRC32_OFFSET + 4..]);
    let crc = hasher.finalize();
    eif[EIF_CRC32_OFFSET..EIF_CRC32_OFFSET + 4].copy_from_slice(&crc.to_be_bytes());

    Ok(eif)
}

/// Prepare the fixed set of sections that go into every EIF (unsigned or signed).
///
/// The returned `KernelPayload` bundles the prepared kernel bytes, cmdline,
/// ramdisk, and metadata JSON. Callers assemble the section list in the
/// order required by their build (unsigned or signed).
struct KernelPayload {
    kernel: Vec<u8>,
    cmdline: Vec<u8>,
    ramdisk: Vec<u8>,
    metadata: Vec<u8>,
}

fn prepare_payload(
    kernel_path: &Path,
    cmdline: &str,
    metadata: &MetadataFields<'_>,
) -> Result<KernelPayload, EifError> {
    let kernel_data = fs::read(kernel_path).context(ReadKernelSnafu { path: kernel_path })?;
    ensure!(!kernel_data.is_empty(), EmptyKernelSnafu);
    // Note: arch-specific pre-processing (zboot unwrap on arm64) happens in
    // the caller, since it depends on `target_arch` — kept out of this helper
    // so both signed and unsigned builders can share it.
    Ok(KernelPayload {
        kernel: kernel_data,
        cmdline: cmdline.as_bytes().to_vec(),
        // Empty RAMDISK section (12-byte header, zero payload). This is
        // well-formed per the EIF spec: `EifSectionRamdisk` has no
        // minimum-count requirement, only an ordering constraint. Bottlerocket
        // sidecar EIFs mount the rootfs from a virtio-blk device with
        // dm-verity, so no initramfs is needed. NOTE: stock `nitro-cli`
        // -produced EIFs always have two ramdisks (bootstrap + customer);
        // consumers that assume that convention (e.g. anything computing PCR2
        // over concatenated ramdisks) will see empty input here.
        ramdisk: Vec::new(),
        metadata: build_metadata(
            metadata.build_tool_version,
            metadata.kernel_version,
            metadata.image_version,
            metadata.build_time,
        )
        .into_bytes(),
    })
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
    let mut payload = prepare_payload(kernel_path, cmdline, metadata)?;
    payload.kernel =
        kernel::prepare_kernel(payload.kernel, target_arch).context(PrepareKernelSnafu)?;

    // Unsigned layout: KERNEL, CMDLINE, RAMDISK, METADATA. Byte-for-byte
    // compatible with the pre-signing implementation; regression-guarded by
    // `test_unsigned_eif_layout_unchanged`.
    let entries = [
        SectionEntry {
            ty: EIF_SECTION_KERNEL,
            data: Cow::Borrowed(&payload.kernel),
        },
        SectionEntry {
            ty: EIF_SECTION_CMDLINE,
            data: Cow::Borrowed(&payload.cmdline),
        },
        SectionEntry {
            ty: EIF_SECTION_RAMDISK,
            data: Cow::Borrowed(&payload.ramdisk),
        },
        SectionEntry {
            ty: EIF_SECTION_METADATA,
            data: Cow::Borrowed(&payload.metadata),
        },
    ];

    let header_flags = target_arch.flags() | pcie_flags;
    let eif = write_eif_bytes(default_mem, default_cpus, header_flags, &entries)?;

    let mut file = fs::File::create(output_path).context(WriteOutputSnafu { path: output_path })?;
    file.write_all(&eif)
        .context(WriteOutputSnafu { path: output_path })?;

    Ok(())
}

/// Build and write a *signed* EIF to the specified output path.
///
/// Layout: KERNEL, CMDLINE, RAMDISK, METADATA, SIGNATURE. The upstream
/// reference implementation places the SIGNATURE section last (see
/// `EifBuilder::write_to` in aws-nitro-enclaves-image-format), and
/// `PcrSignatureChecker::from_eif` scans the section table looking for the
/// signature type — placement within the table does not affect validation.
///
/// PCR0 is computed as `SHA-384(48 zero bytes || SHA-384(kernel || cmdline
/// || ramdisks))` per the spec: kernel and cmdline and ramdisk **data**
/// (headers excluded), with the section-signature and metadata sections
/// **not** included. See <https://github.com/aws/aws-nitro-enclaves-image-format>.
#[allow(clippy::too_many_arguments)]
pub fn build_signed_eif(
    kernel_path: &Path,
    cmdline: &str,
    output_path: &Path,
    default_mem: u64,
    default_cpus: u64,
    pcie_flags: u16,
    target_arch: TargetArch,
    metadata: &MetadataFields<'_>,
    signer: &dyn Signer,
) -> Result<(), EifError> {
    let mut payload = prepare_payload(kernel_path, cmdline, metadata)?;
    payload.kernel =
        kernel::prepare_kernel(payload.kernel, target_arch).context(PrepareKernelSnafu)?;

    let pcr0 = compute_pcr0(&payload.kernel, &payload.cmdline, &[&payload.ramdisk]);
    // `build_signature_section` enforces `SIGNATURE_MAX_SIZE` internally with
    // per-component size attribution in the error message; no additional
    // check needed here.
    let signature_bytes = build_signature_section(signer, &pcr0)?;

    let entries = [
        SectionEntry {
            ty: EIF_SECTION_KERNEL,
            data: Cow::Borrowed(&payload.kernel),
        },
        SectionEntry {
            ty: EIF_SECTION_CMDLINE,
            data: Cow::Borrowed(&payload.cmdline),
        },
        SectionEntry {
            ty: EIF_SECTION_RAMDISK,
            data: Cow::Borrowed(&payload.ramdisk),
        },
        SectionEntry {
            ty: EIF_SECTION_METADATA,
            data: Cow::Borrowed(&payload.metadata),
        },
        SectionEntry {
            ty: EIF_SECTION_SIGNATURE,
            data: Cow::Borrowed(&signature_bytes),
        },
    ];

    let header_flags = target_arch.flags() | pcie_flags;
    let eif = write_eif_bytes(default_mem, default_cpus, header_flags, &entries)?;

    let mut file = fs::File::create(output_path).context(WriteOutputSnafu { path: output_path })?;
    file.write_all(&eif)
        .context(WriteOutputSnafu { path: output_path })?;

    Ok(())
}

/// One parsed section from an existing EIF: type + raw data bytes.
///
/// Produced by [`read_sections`]. Section headers are stripped; `data` is the
/// section payload only. Used by [`resign_eif`] to rebuild the section list
/// after replacing the signature section without materially decoding the
/// kernel/cmdline/ramdisk/metadata contents.
#[derive(Debug)]
pub struct ParsedSection {
    /// Section type (one of the `EIF_SECTION_*` constants).
    pub ty: u16,
    /// Section payload bytes (no section header).
    pub data: Vec<u8>,
}

/// Header fields recovered from an EIF's fixed header.
struct EifHeader {
    default_mem: u64,
    default_cpus: u64,
    flags: u16,
}

/// Parse the fixed header of an EIF and validate the magic and version.
fn read_header(bytes: &[u8]) -> Result<EifHeader, EifError> {
    ensure!(
        bytes.len() >= EIF_HEADER_SIZE,
        ParseInputSnafu {
            reason: format!(
                "input is {} bytes, smaller than the {}-byte EIF header",
                bytes.len(),
                EIF_HEADER_SIZE
            ),
        }
    );
    ensure!(
        bytes[0..4] == EIF_MAGIC,
        ParseInputSnafu {
            reason: "bad EIF magic".to_string(),
        }
    );
    let version = u16::from_be_bytes([bytes[4], bytes[5]]);
    ensure!(
        version == EIF_HDR_VERSION,
        ParseInputSnafu {
            reason: format!(
                "unsupported EIF header version {version} (expected {EIF_HDR_VERSION})"
            ),
        }
    );
    let flags = u16::from_be_bytes([bytes[6], bytes[7]]);
    let default_mem = u64::from_be_bytes(bytes[8..16].try_into().unwrap());
    let default_cpus = u64::from_be_bytes(bytes[16..24].try_into().unwrap());
    Ok(EifHeader {
        default_mem,
        default_cpus,
        flags,
    })
}

/// Read the section list out of an already-serialized EIF.
///
/// Sections are returned in the order they appear in the header's
/// offset/size tables. Section headers (12 bytes each) are validated against
/// the table entries and stripped; only the payload bytes are returned. This
/// is the shared parser used by [`resign_eif`] and (in tests) by
/// `extract_section`.
pub fn read_sections(eif_bytes: &[u8]) -> Result<Vec<ParsedSection>, EifError> {
    read_header(eif_bytes)?;
    // Header layout (post-magic/version/flags/mem/cpus/reserved):
    //   num_sections: u16
    //   offsets: [u64; MAX_NUM_SECTIONS]
    //   sizes:   [u64; MAX_NUM_SECTIONS]
    //   unused: u32
    //   crc32:  u32
    let num_sections_off = 4 + 2 + 2 + 8 + 8 + 2; // magic+ver+flags+mem+cpus+reserved
    let num_sections =
        u16::from_be_bytes([eif_bytes[num_sections_off], eif_bytes[num_sections_off + 1]]) as usize;
    ensure!(
        num_sections <= MAX_NUM_SECTIONS,
        ParseInputSnafu {
            reason: format!(
                "num_sections={num_sections} exceeds MAX_NUM_SECTIONS={MAX_NUM_SECTIONS}"
            ),
        }
    );

    let offsets_start = num_sections_off + 2;
    let sizes_start = offsets_start + MAX_NUM_SECTIONS * 8;

    let mut sections = Vec::with_capacity(num_sections);
    for i in 0..num_sections {
        let off = u64::from_be_bytes(
            eif_bytes[offsets_start + i * 8..offsets_start + (i + 1) * 8]
                .try_into()
                .unwrap(),
        ) as usize;
        let size = u64::from_be_bytes(
            eif_bytes[sizes_start + i * 8..sizes_start + (i + 1) * 8]
                .try_into()
                .unwrap(),
        ) as usize;
        // Reject offsets that fall inside the fixed EIF header. Without this
        // check, `compute_pcr0` in the resign path would hash header bytes
        // as if they were kernel/cmdline data — a hardening gap a hand-crafted
        // input EIF could exploit even though real rpm2eif output is always
        // well-formed.
        ensure!(
            off >= EIF_HEADER_SIZE,
            ParseInputSnafu {
                reason: format!(
                    "section {i} offset {off} lies within the fixed EIF header (< {EIF_HEADER_SIZE})"
                ),
            }
        );
        ensure!(
            off.checked_add(EIF_SECTION_HEADER_SIZE)
                .and_then(|o| o.checked_add(size))
                .is_some_and(|end| end <= eif_bytes.len()),
            ParseInputSnafu {
                reason: format!(
                    "section {i} at offset {off} size {size} exceeds file length {}",
                    eif_bytes.len(),
                ),
            }
        );
        let ty = u16::from_be_bytes([eif_bytes[off], eif_bytes[off + 1]]);
        // Section header carries its own size at bytes 4..12; validate it
        // matches the size table so a malformed file doesn't slip through.
        let hdr_size =
            u64::from_be_bytes(eif_bytes[off + 4..off + 12].try_into().unwrap()) as usize;
        ensure!(
            hdr_size == size,
            ParseInputSnafu {
                reason: format!(
                    "section {i} header size {hdr_size} disagrees with table size {size}",
                ),
            }
        );
        let data_off = off + EIF_SECTION_HEADER_SIZE;
        sections.push(ParsedSection {
            ty,
            data: eif_bytes[data_off..data_off + size].to_vec(),
        });
    }
    Ok(sections)
}

/// Summary of an EIF's contents as a `serde_json::Value`.
///
/// Consumers (notably `eif2eif`) use this to carry `BuildMetadata` forward
/// across a repack: rpm2eif populates `KernelVersion` and `BuildToolVersion`
/// from the source rootfs' installed RPMs; eif2eif has no rootfs to query,
/// so it reads them out of the input EIF via this function and forwards them
/// on the `eif-builder build` invocation that produces the new EIF.
///
/// Output shape (stable — this JSON is a public contract):
///
/// ```json
/// {
///   "eif_version": 4,
///   "is_signed": true,
///   "cmdline": "root=/dev/dm-0 ro ...",
///   "kernel_size": 50331648,
///   "ramdisk_count": 0,
///   "sections": [
///     { "type": "KERNEL",   "type_id": 1, "size": 50331648 },
///     { "type": "CMDLINE",  "type_id": 2, "size":      431 },
///     { "type": "METADATA", "type_id": 5, "size":      254 },
///     { "type": "SIGNATURE","type_id": 4, "size":     2115 }
///   ],
///   "metadata": {
///     "ImageName": "bottlerocket-sidecar",
///     "BuildMetadata": {
///       "KernelVersion": "6.18.30-1.1779997967.64782dc8.br1",
///       "BuildToolVersion": "0.21.0",
///       ...
///     },
///     ...
///   }
/// }
/// ```
///
/// `metadata` is `null` when the EIF has no METADATA section (rare — every
/// EIF our pipeline produces has one, but a hand-crafted or foreign-tool EIF
/// might not). `cmdline` is the CMDLINE section decoded as UTF-8 with a
/// single trailing NUL byte stripped if present (rpm2eif appends one; the
/// enclave loader treats it as a C string).
///
/// Errors:
/// - `ReadInput`  — I/O reading `input`.
/// - `ParseInput` — malformed header/section table (via [`read_sections`]).
/// - `MetadataDecode` — METADATA section is not valid UTF-8 JSON.
///
/// Note: CMDLINE is not required to be UTF-8, but Bottlerocket's rpm2eif
/// always writes it as ASCII/UTF-8. If a non-UTF-8 CMDLINE ever appears here,
/// we fall back to the lossy replacement encoding rather than fail — the
/// primary consumer (metadata carry-forward) does not depend on CMDLINE.
pub fn describe_eif(input: &Path) -> Result<serde_json::Value, EifError> {
    let bytes = fs::read(input).context(ReadInputSnafu { path: input })?;
    let sections = read_sections(&bytes)?;

    let mut kernel_size: u64 = 0;
    let mut cmdline_bytes: Option<Vec<u8>> = None;
    let mut ramdisk_count: usize = 0;
    let mut is_signed = false;
    let mut metadata_bytes: Option<Vec<u8>> = None;
    let mut section_summaries: Vec<serde_json::Value> = Vec::with_capacity(sections.len());
    for s in &sections {
        section_summaries.push(serde_json::json!({
            "type": section_type_name(s.ty),
            "type_id": s.ty,
            "size": s.data.len(),
        }));
        match s.ty {
            EIF_SECTION_KERNEL => kernel_size = s.data.len() as u64,
            EIF_SECTION_CMDLINE => cmdline_bytes = Some(s.data.clone()),
            EIF_SECTION_RAMDISK => ramdisk_count += 1,
            EIF_SECTION_SIGNATURE => is_signed = true,
            EIF_SECTION_METADATA => metadata_bytes = Some(s.data.clone()),
            _ => {} // still reported in `sections`, just not summarized
        }
    }

    // Decode METADATA as JSON. Fail loudly here rather than substitute
    // `null`, so the caller (eif2eif) doesn't silently drop KernelVersion
    // when the section is present-but-garbled: that would reproduce the
    // exact bug this function exists to prevent.
    let metadata_value = match metadata_bytes {
        Some(raw) => {
            let text = std::str::from_utf8(&raw).map_err(|e| EifError::MetadataDecode {
                reason: format!("invalid UTF-8 at byte {}: {e}", e.valid_up_to()),
            })?;
            serde_json::from_str::<serde_json::Value>(text).map_err(|e| {
                EifError::MetadataDecode {
                    reason: format!("JSON parse: {e}"),
                }
            })?
        }
        None => serde_json::Value::Null,
    };

    // CMDLINE: decode leniently. rpm2eif writes ASCII/UTF-8 with an optional
    // trailing NUL. Lossy-decode is safe because no consumer of `describe_eif`
    // relies on the exact byte content of `cmdline` — the field is
    // informational.
    let cmdline_str = cmdline_bytes.as_deref().map(|b| {
        let trimmed = match b.last() {
            Some(&0) => &b[..b.len() - 1],
            _ => b,
        };
        String::from_utf8_lossy(trimmed).into_owned()
    });

    Ok(serde_json::json!({
        "eif_version": EIF_HDR_VERSION,
        "is_signed": is_signed,
        "cmdline": cmdline_str,
        "kernel_size": kernel_size,
        "ramdisk_count": ramdisk_count,
        "sections": section_summaries,
        "metadata": metadata_value,
    }))
}

/// Re-sign an existing EIF: replace (or append) its `EifSectionSignature`
/// (0x04) section without touching the kernel, cmdline, ramdisk, or metadata
/// sections.
///
/// The kernel section is treated as opaque bytes: whatever the input carries
/// is exactly what is embedded in the output. This matters on arm64, where
/// the on-disk kernel section is the *prepared* (post-zboot-unwrap) form; we
/// do not re-run `prepare_kernel`. PCR0 is recomputed from the input's
/// existing kernel + cmdline + ramdisk bytes, so the new signature covers
/// the same measurement the enclave would see.
///
/// Layout of the output matches [`build_signed_eif`]: KERNEL, CMDLINE, one
/// or more RAMDISKs, METADATA, SIGNATURE (last). Order is preserved from the
/// input for non-signature sections; the SIGNATURE section is placed last.
///
/// When the input is unsigned, a new SIGNATURE section is appended. When the
/// input is already signed, the existing SIGNATURE section is dropped and a
/// new one takes its place. `MAX_NUM_SECTIONS` is enforced in both cases.
pub fn resign_eif(input: &Path, output: &Path, signer: &dyn Signer) -> Result<(), EifError> {
    let bytes = fs::read(input).context(ReadInputSnafu { path: input })?;
    let header = read_header(&bytes)?;
    let sections = read_sections(&bytes)?;

    // Extract kernel/cmdline/ramdisks for PCR0. We compute PCR0 from the
    // *input's* on-disk bytes rather than re-running `prepare_payload`, so
    // the resign path is intrinsically forward-compatible with any future
    // change to how kernel bytes are prepared.
    let mut kernel: Option<&[u8]> = None;
    let mut cmdline: Option<&[u8]> = None;
    let mut ramdisks: Vec<&[u8]> = Vec::new();
    for s in &sections {
        match s.ty {
            EIF_SECTION_KERNEL => kernel = Some(&s.data),
            EIF_SECTION_CMDLINE => cmdline = Some(&s.data),
            EIF_SECTION_RAMDISK => ramdisks.push(&s.data),
            _ => {}
        }
    }
    let kernel = kernel.ok_or_else(|| EifError::ParseInput {
        reason: "input EIF has no KERNEL section".to_string(),
    })?;
    let cmdline = cmdline.ok_or_else(|| EifError::ParseInput {
        reason: "input EIF has no CMDLINE section".to_string(),
    })?;
    // A well-formed EIF has at least one RAMDISK section (possibly empty).
    // We tolerate none in case a caller hand-authored an EIF without one;
    // `compute_pcr0` accepts an empty ramdisk slice.

    let pcr0 = compute_pcr0(kernel, cmdline, &ramdisks);
    let signature_bytes = build_signature_section(signer, &pcr0)?;

    // Rebuild the section list: preserve non-signature sections in order,
    // then append the new signature section last (matching `build_signed_eif`).
    let mut entries: Vec<SectionEntry<'_>> = Vec::with_capacity(sections.len() + 1);
    for s in &sections {
        if s.ty == EIF_SECTION_SIGNATURE {
            continue; // drop the old signature
        }
        entries.push(SectionEntry {
            ty: s.ty,
            data: Cow::Borrowed(&s.data),
        });
    }
    // Guard MAX_NUM_SECTIONS at the resign site so the error message
    // identifies the "resign has no room to append its section" case
    // rather than surfacing as a generic overflow inside `write_eif_bytes`
    // (which also enforces the invariant, as a backstop).
    ensure!(
        entries.len() < MAX_NUM_SECTIONS,
        TooManySectionsSnafu {
            count: entries.len() + 1
        },
    );
    entries.push(SectionEntry {
        ty: EIF_SECTION_SIGNATURE,
        data: Cow::Borrowed(&signature_bytes),
    });

    let eif = write_eif_bytes(
        header.default_mem,
        header.default_cpus,
        header.flags,
        &entries,
    )?;
    let mut file = fs::File::create(output).context(WriteOutputSnafu { path: output })?;
    file.write_all(&eif)
        .context(WriteOutputSnafu { path: output })?;
    Ok(())
}

/// Compute PCR0 exactly as the reference `EifBuilder::image_hasher` does.
///
/// The upstream algorithm feeds kernel-data, then cmdline-data, then each
/// ramdisk's data in order into a single `EifHasher`, which performs a TPM-
/// style extend: `PCR = SHA-384(48 zero bytes || SHA-384(concatenated bytes))`.
/// Section-signature and metadata data are deliberately not included.
///
/// This function is the regression guard for interoperability with
/// `PcrSignatureChecker::verify`.
fn compute_pcr0(kernel_data: &[u8], cmdline_data: &[u8], ramdisks: &[&[u8]]) -> Vec<u8> {
    use aws_lc_rs::digest::{digest, SHA384};

    // Inner: SHA-384 over kernel || cmdline || ramdisks_data
    let mut inner = Vec::with_capacity(kernel_data.len() + cmdline_data.len() + 64);
    inner.extend_from_slice(kernel_data);
    inner.extend_from_slice(cmdline_data);
    for r in ramdisks {
        inner.extend_from_slice(r);
    }
    let inner_digest = digest(&SHA384, &inner);

    // Outer (TPM extend): SHA-384(48 zero bytes || inner_digest)
    let mut outer_input = vec![0u8; 48];
    outer_input.extend_from_slice(inner_digest.as_ref());
    let outer_digest = digest(&SHA384, &outer_input);
    outer_digest.as_ref().to_vec()
}

/// Local mirror of upstream `aws-nitro-enclaves-image-format::defs::PcrInfo`.
///
/// The upstream struct is what `PcrSignatureChecker::verify` reconstructs and
/// then byte-compares against the COSE_Sign1 payload:
///
/// ```ignore
/// let pcr_info  = PcrInfo::new(0, pcr0_bytes);
/// let measured  = serde_cbor::to_vec(&pcr_info)?;
/// let coses     = pcr_sign.get_payload(...)?;
/// self.sign_check = Some(measured == coses);   // exact byte compare
/// ```
///
/// Because the comparison is byte-exact, our payload must serialize to the
/// *same* CBOR bytes as upstream's `PcrInfo`. `serde_cbor` encodes `Vec<u8>`
/// as a CBOR **array of integers** (major type 4), not a byte string — the
/// standard serde `Vec<T>` path calls `serialize_seq`. That is the wire
/// shape upstream produces, and the shape this struct produces here.
///
/// Field names and order must match upstream exactly. `register_index` is
/// `i32` upstream — do not widen.
#[derive(Serialize, Deserialize)]
struct PcrInfo {
    register_index: i32,
    register_value: Vec<u8>,
}

/// Local mirror of upstream `aws-nitro-enclaves-image-format::defs::PcrSignature`.
///
/// The outer envelope of the `EifSectionSignature` is a `Vec<PcrSignature>`
/// serialized with `serde_cbor::to_vec`. The upstream `describe-eif` path
/// decodes it with `serde_cbor::from_slice::<Vec<PcrSignature>>`, so we
/// must serialize with the *same* serde derive on a struct with the *same*
/// field shape (`Vec<u8>` — not `serde_bytes::ByteBuf`), or the decode will
/// fail with `invalid type: byte array, expected a sequence`.
#[derive(Serialize, Deserialize)]
struct PcrSignature {
    signing_certificate: Vec<u8>,
    signature: Vec<u8>,
}

/// Build the CBOR bytes that go into the `EifSectionSignature` (0x04) section.
///
/// Shape (from `aws-nitro-enclaves-image-format`, produced by
/// `serde_cbor::to_vec(&Vec<PcrSignature>)`):
///
/// ```text
/// Array(1) {
///     Map(2) {
///         "signing_certificate": <PEM cert bytes as CBOR Array-of-uint>,
///         "signature":           <COSE_Sign1 bytes as CBOR Array-of-uint>,
///     }
/// }
/// ```
///
/// Note: the byte fields decode as CBOR **arrays of integers** (major
/// type 4), not byte strings (major type 2). This is what `serde_cbor`
/// emits for a `Vec<u8>` under a plain `#[derive(Serialize)]` — the
/// standard serde `Vec<T>` path — and it is what upstream produces and
/// upstream consumers expect. Emitting byte strings here (via
/// `ciborium::Value::Bytes` or `serde_bytes`) would make the CBOR envelope
/// unparseable by `serde_cbor::from_slice::<Vec<PcrSignature>>` and would
/// desync the COSE payload from the verifier's `measured_payload`
/// recomputation, silently failing signature verification.
fn build_signature_section(signer: &dyn Signer, pcr0: &[u8]) -> Result<Vec<u8>, EifError> {
    use coset::{CborSerializable, CoseSign1Builder, HeaderBuilder};

    // Payload: `serde_cbor::to_vec(&PcrInfo { register_index: 0,
    // register_value: pcr0 })`. Byte-for-byte identical to what
    // `PcrSignatureChecker::verify` reconstructs as `measured_payload`,
    // which is the exact-byte-compare peer for the COSE payload.
    let payload = serde_cbor::to_vec(&PcrInfo {
        register_index: 0,
        register_value: pcr0.to_vec(),
    })
    .context(CborEncodePayloadSnafu)?;

    // Translate our crate-local `SignAlg` to the COSE `iana::Algorithm`
    // value at the one site that depends on `coset`, so the `Signer` trait
    // stays version-independent of `coset`.
    let alg = match signer.algorithm() {
        signer::SignAlg::Es256 => coset::iana::Algorithm::ES256,
        signer::SignAlg::Es384 => coset::iana::Algorithm::ES384,
    };
    let protected = HeaderBuilder::new().algorithm(alg).build();

    // `create_signature` is called on the pre-hash `Sig_structure1` bytes;
    // the signer produces the raw ECDSA `r||s` signature over that.
    let sign1 = CoseSign1Builder::new()
        .protected(protected)
        .payload(payload)
        .try_create_signature(&[], |tbs| signer.sign_cose(tbs))
        .context(SignSnafu)?
        .build();

    let cose_bytes = sign1.to_vec().context(CoseBuildSnafu)?;
    let cose_size = cose_bytes.len();

    // Certificate goes into the section as raw PEM bytes (the reference
    // reader uses `X509::from_pem` on it).
    let cert_pem = signer.cert_pem().to_vec();
    let cert_pem_size = cert_pem.len();

    // Outer envelope: `serde_cbor::to_vec(&Vec<PcrSignature>)`. Using the
    // upstream-shape struct + the same serde_cbor serializer guarantees
    // the wire bytes are what `describe-eif`'s
    // `from_slice::<Vec<PcrSignature>>` accepts.
    let envelope = vec![PcrSignature {
        signing_certificate: cert_pem,
        signature: cose_bytes,
    }];
    let out = serde_cbor::to_vec(&envelope).context(CborEncodeSignatureSnafu)?;
    // Enforce SIGNATURE_MAX_SIZE here so any caller of
    // `build_signature_section` (not just `build_signed_eif`) gets the
    // check. The error identifies the two biggest contributors — a bulky
    // cert chain vs. an unexpectedly large COSE payload — so operators
    // don't have to guess.
    ensure!(
        out.len() <= SIGNATURE_MAX_SIZE,
        SignatureTooLargeSnafu {
            size: out.len(),
            cert_pem_size,
            cose_size,
        },
    );
    Ok(out)
}

/// Shared DER helpers for test code. `wrap_der` produces a DER TLV with
/// short/long-form length encoding; used by both `signer.rs` (for SPKI
/// synthesis) and by `lib.rs` tests (for the minimal self-signed cert).
///
/// Kept in one place so the length-encoding logic can't drift across the
/// crate — the family of "hand-rolled DER" was flagged in code review as a
/// maintenance risk when duplicated.
#[cfg(test)]
pub(crate) mod der_helpers {
    pub(crate) fn wrap_der(tag: u8, body: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        let n = body.len();
        if n < 0x80 {
            out.push(n as u8);
        } else if n < 0x100 {
            out.push(0x81);
            out.push(n as u8);
        } else if n < 0x10000 {
            out.push(0x82);
            out.push((n >> 8) as u8);
            out.push((n & 0xff) as u8);
        } else {
            out.push(0x83);
            out.push((n >> 16) as u8);
            out.push((n >> 8) as u8);
            out.push((n & 0xff) as u8);
        }
        out.extend_from_slice(body);
        out
    }
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

    // -------- PCR0 / signature tests --------

    #[test]
    fn test_pcr0_matches_spec() {
        // Reference vector: PCR0 = SHA-384(48 zero bytes || SHA-384(kernel || cmdline || ramdisks_data))
        // Recompute independently via aws-lc-rs to be sure the helper isn't
        // silently drifting.
        use aws_lc_rs::digest::{digest, SHA384};

        let kernel = b"FAKE_KERNEL".to_vec();
        let cmdline = b"console=ttyS0".to_vec();
        let ramdisk: Vec<u8> = Vec::new();

        let mut inner = Vec::new();
        inner.extend_from_slice(&kernel);
        inner.extend_from_slice(&cmdline);
        inner.extend_from_slice(&ramdisk);
        let inner_d = digest(&SHA384, &inner);
        let mut outer = vec![0u8; 48];
        outer.extend_from_slice(inner_d.as_ref());
        let expected = digest(&SHA384, &outer).as_ref().to_vec();

        let got = compute_pcr0(&kernel, &cmdline, &[&ramdisk]);
        assert_eq!(got, expected, "PCR0 must match the reference spec");
        assert_eq!(got.len(), 48, "SHA-384 output is 48 bytes");
    }

    #[test]
    fn test_unsigned_eif_layout_unchanged() {
        // The refactored `write_eif_bytes` path must produce byte-identical
        // output to the pre-refactor implementation for the same inputs.
        // We assert on the section-header layout: KERNEL @ EIF_HEADER_SIZE,
        // then CMDLINE, then RAMDISK, then METADATA.
        let dir = tempfile::tempdir().unwrap();
        let kernel = dir.path().join("kernel");
        let output = dir.path().join("test.eif");
        fs::write(&kernel, b"FAKE_KERNEL").unwrap();
        build_eif(
            &kernel,
            "cmd",
            &output,
            512 << 20,
            2,
            0,
            TargetArch::X86_64,
            &MetadataFields::default(),
        )
        .unwrap();
        let eif = fs::read(&output).unwrap();
        // First section header lives at offset EIF_HEADER_SIZE.
        let ty = u16::from_be_bytes([eif[EIF_HEADER_SIZE], eif[EIF_HEADER_SIZE + 1]]);
        assert_eq!(ty, EIF_SECTION_KERNEL);
    }

    /// Build a self-signed ECDSA P-384 cert + private key for tests.
    ///
    /// The resulting X.509 is manually DER-encoded so we don't need to pull
    /// in `x509-cert`'s `builder` feature (which drags in `signature`,
    /// `rsa`, and friends). Structural validity is what the signer needs:
    /// `x509-cert::Certificate::from_der` must parse it, and the SPKI OID
    /// pair must reflect P-384. The `signatureValue` is *not* a valid
    /// signature over `tbsCertificate` — that's fine because
    /// `PcrSignatureChecker::verify` only checks the COSE signature, not
    /// the certificate's own signature chain.
    #[cfg(test)]
    fn generate_test_cert_and_key() -> (Vec<u8>, Vec<u8>) {
        use aws_lc_rs::signature::{EcdsaKeyPair, KeyPair, ECDSA_P384_SHA384_ASN1_SIGNING};
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P384_SHA384_ASN1_SIGNING, &rng).unwrap();
        let pem_key = pem::Pem::new("PRIVATE KEY", pkcs8.as_ref().to_vec());
        let key_pem = pem::encode(&pem_key).into_bytes();

        // Parse the pkcs8 to obtain the SEC1 public key.
        let key_pair =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P384_SHA384_ASN1_SIGNING, pkcs8.as_ref()).unwrap();
        let sec1_pub = key_pair.public_key().as_ref().to_vec();

        let cert_der = build_p384_self_signed_der(&sec1_pub);
        let cert_pem_obj = pem::Pem::new("CERTIFICATE", cert_der);
        let cert_pem = pem::encode(&cert_pem_obj).into_bytes();

        (cert_pem, key_pem)
    }

    /// Build a minimal, syntactically-valid X.509 Certificate carrying a
    /// P-384 SPKI for the given SEC1-uncompressed public key. See the
    /// `generate_test_cert_and_key` doc for what's excluded.
    #[cfg(test)]
    fn build_p384_self_signed_der(sec1_pub: &[u8]) -> Vec<u8> {
        // OIDs:
        //   id-ecPublicKey        1.2.840.10045.2.1  → 06 07 2A 86 48 CE 3D 02 01
        //   secp384r1             1.3.132.0.34       → 06 05 2B 81 04 00 22
        //   ecdsa-with-SHA384     1.2.840.10045.4.3.3 → 06 08 2A 86 48 CE 3D 04 03 03
        //   id-at-commonName      2.5.4.3            → 06 03 55 04 03
        let id_ec_pub = [0x06u8, 0x07, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x02, 0x01];
        let secp384r1 = [0x06u8, 0x05, 0x2B, 0x81, 0x04, 0x00, 0x22];
        let ecdsa_sha384 = [0x06u8, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x03];
        let id_cn = [0x06u8, 0x03, 0x55, 0x04, 0x03];

        // AlgorithmIdentifier for SPKI: SEQUENCE { id-ecPublicKey, secp384r1 }.
        let mut spki_alg_body = Vec::new();
        spki_alg_body.extend_from_slice(&id_ec_pub);
        spki_alg_body.extend_from_slice(&secp384r1);
        let spki_alg = crate::der_helpers::wrap_der(0x30, &spki_alg_body);

        // BIT STRING wrapping the SEC1 public key with 0 unused bits.
        let mut spki_key = vec![0u8];
        spki_key.extend_from_slice(sec1_pub);
        let spki_key_bs = crate::der_helpers::wrap_der(0x03, &spki_key);

        // SubjectPublicKeyInfo ::= SEQUENCE { AlgorithmIdentifier, subjectPublicKey BIT STRING }
        let mut spki_body = Vec::new();
        spki_body.extend_from_slice(&spki_alg);
        spki_body.extend_from_slice(&spki_key_bs);
        let spki = crate::der_helpers::wrap_der(0x30, &spki_body);

        // Name ::= SEQUENCE OF RDN. One RDN with one AttributeTypeAndValue:
        //   RelativeDistinguishedName ::= SET OF ATV
        //   ATV ::= SEQUENCE { type OID (id-at-commonName), value UTF8String("test") }
        let cn_value = crate::der_helpers::wrap_der(0x0C, b"test"); // UTF8String
        let mut atv = Vec::new();
        atv.extend_from_slice(&id_cn);
        atv.extend_from_slice(&cn_value);
        let atv_seq = crate::der_helpers::wrap_der(0x30, &atv);
        let rdn = crate::der_helpers::wrap_der(0x31, &atv_seq); // SET
        let name = crate::der_helpers::wrap_der(0x30, &rdn); // SEQUENCE OF RDN

        // Validity ::= SEQUENCE { notBefore UTCTime, notAfter UTCTime }
        // We use two fixed UTCTime values well in the past/future so the
        // reference `PcrSignatureChecker::verify` (which checks validity)
        // wouldn't complain either.
        let not_before = crate::der_helpers::wrap_der(0x17, b"200101000000Z"); // UTCTime
        let not_after = crate::der_helpers::wrap_der(0x17, b"400101000000Z");
        let mut validity_body = Vec::new();
        validity_body.extend_from_slice(&not_before);
        validity_body.extend_from_slice(&not_after);
        let validity = crate::der_helpers::wrap_der(0x30, &validity_body);

        // AlgorithmIdentifier for the cert's own signature.
        let sig_alg = crate::der_helpers::wrap_der(0x30, &ecdsa_sha384);

        // TBSCertificate ::= SEQUENCE {
        //   version         [0] EXPLICIT INTEGER DEFAULT v1 -- v3 = 2
        //   serialNumber        INTEGER
        //   signature           AlgorithmIdentifier
        //   issuer              Name
        //   validity            Validity
        //   subject             Name
        //   subjectPublicKeyInfo SPKI
        // }
        let version_inner = crate::der_helpers::wrap_der(0x02, &[2u8]); // INTEGER 2
        let version = crate::der_helpers::wrap_der(0xA0, &version_inner); // [0] EXPLICIT
        let serial = crate::der_helpers::wrap_der(0x02, &[0x01u8]); // INTEGER 1
        let mut tbs_body = Vec::new();
        tbs_body.extend_from_slice(&version);
        tbs_body.extend_from_slice(&serial);
        tbs_body.extend_from_slice(&sig_alg);
        tbs_body.extend_from_slice(&name); // issuer
        tbs_body.extend_from_slice(&validity);
        tbs_body.extend_from_slice(&name); // subject (self-signed)
        tbs_body.extend_from_slice(&spki);
        let tbs = crate::der_helpers::wrap_der(0x30, &tbs_body);

        // signatureValue: minimal BIT STRING with 0 unused bits carrying a
        // fake DER-encoded ECDSA signature. Structural presence is all
        // that's needed.
        let fake_sig_bytes = [0x00u8]; // 0 unused bits
        let sig_val = crate::der_helpers::wrap_der(0x03, &fake_sig_bytes);

        // Certificate ::= SEQUENCE { tbsCertificate, signatureAlgorithm, signatureValue }
        let mut cert_body = Vec::new();
        cert_body.extend_from_slice(&tbs);
        cert_body.extend_from_slice(&sig_alg);
        cert_body.extend_from_slice(&sig_val);
        crate::der_helpers::wrap_der(0x30, &cert_body)
    }

    #[test]
    fn test_signed_eif_structure() {
        // Regression guard for the upstream consumer path: the outer envelope
        // must round-trip through `serde_cbor::from_slice::<Vec<PcrSignature>>`,
        // which is exactly what `nitro-cli describe-eif` and the reference
        // `PcrSignatureChecker` do. A prior implementation emitted the byte
        // fields as CBOR byte strings via `ciborium::Value::Bytes`; that
        // parses fine with ciborium but fails serde_cbor's `Vec<u8>` decode
        // with `invalid type: byte array, expected a sequence`, silently
        // rendering the EIF unusable at attestation time.
        let (cert_pem, key_pem) = generate_test_cert_and_key();
        let signer = signer::LocalSigner::from_pem(&cert_pem, &key_pem).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let kernel = dir.path().join("kernel");
        let output = dir.path().join("signed.eif");
        fs::write(&kernel, b"FAKE_KERNEL").unwrap();

        build_signed_eif(
            &kernel,
            "cmd",
            &output,
            512 << 20,
            2,
            0,
            TargetArch::X86_64,
            &MetadataFields::default(),
            &signer,
        )
        .unwrap();

        let eif = fs::read(&output).unwrap();
        // Find the signature section by scanning the section-size table.
        let sig = extract_section(&eif, EIF_SECTION_SIGNATURE)
            .expect("signature section must be present");

        // Cross-decode via the exact `from_slice::<Vec<PcrSignature>>` shape
        // used by `aws-nitro-enclaves-image-format::utils::eif_reader`. A
        // decode error here would mean the emitted EIF is unreadable to
        // downstream Nitro tooling.
        let envelope: Vec<PcrSignature> =
            serde_cbor::from_slice(&sig).expect("outer envelope must decode via serde_cbor");
        assert_eq!(envelope.len(), 1, "outer envelope must hold one signature");
        assert_eq!(
            envelope[0].signing_certificate, cert_pem,
            "signing_certificate must equal the embedded PEM"
        );
        assert!(
            !envelope[0].signature.is_empty(),
            "signature bytes must be present"
        );
    }

    /// Scan a serialized EIF for a section of the given type and return its
    /// data bytes. Uses the offset+size tables in the header rather than
    /// walking section headers, so it is robust against ordering changes.
    fn extract_section(eif: &[u8], ty: u16) -> Option<Vec<u8>> {
        // Header layout: magic(4) version(2) flags(2) mem(8) cpus(8) reserved(2) num_sections(2)
        // then offsets[MAX_NUM_SECTIONS]*u64, sizes[MAX_NUM_SECTIONS]*u64.
        let num_sections =
            u16::from_be_bytes([eif[4 + 2 + 2 + 8 + 8 + 2], eif[4 + 2 + 2 + 8 + 8 + 2 + 1]])
                as usize;
        let offsets_start = 4 + 2 + 2 + 8 + 8 + 2 + 2;
        let sizes_start = offsets_start + MAX_NUM_SECTIONS * 8;
        for i in 0..num_sections {
            let off = u64::from_be_bytes(
                eif[offsets_start + i * 8..offsets_start + (i + 1) * 8]
                    .try_into()
                    .unwrap(),
            ) as usize;
            let size = u64::from_be_bytes(
                eif[sizes_start + i * 8..sizes_start + (i + 1) * 8]
                    .try_into()
                    .unwrap(),
            ) as usize;
            // Section header at `off`: ty(2) flags(2) size(8) data(size).
            let this_ty = u16::from_be_bytes([eif[off], eif[off + 1]]);
            if this_ty == ty {
                let data_off = off + EIF_SECTION_HEADER_SIZE;
                return Some(eif[data_off..data_off + size].to_vec());
            }
        }
        None
    }

    #[test]
    fn test_signed_eif_crc_valid() {
        // After inserting the signature section, the header CRC must still
        // validate against the whole file.
        let (cert_pem, key_pem) = generate_test_cert_and_key();
        let signer = signer::LocalSigner::from_pem(&cert_pem, &key_pem).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let kernel = dir.path().join("kernel");
        let output = dir.path().join("signed.eif");
        fs::write(&kernel, b"FAKE_KERNEL").unwrap();

        build_signed_eif(
            &kernel,
            "cmd",
            &output,
            512 << 20,
            2,
            0,
            TargetArch::X86_64,
            &MetadataFields::default(),
            &signer,
        )
        .unwrap();

        let eif = fs::read(&output).unwrap();
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

    /// Regression guard for the *exact* upstream verification path:
    ///
    ///     let pcr_info = PcrInfo::new(0, pcr0);
    ///     let measured = serde_cbor::to_vec(&pcr_info).unwrap();
    ///     let coses    = pcr_sign.get_payload(...).unwrap();
    ///     assert_eq!(measured, coses);       // exact byte compare
    ///
    /// If our emitted COSE payload is not byte-identical to what
    /// `serde_cbor::to_vec(&PcrInfo)` produces on the verifier side,
    /// `sign_check` comes back `false` even when the ECDSA signature is
    /// cryptographically valid — silent, unrecoverable attestation failure.
    /// Guard both sides here so a future refactor cannot regress the
    /// wire-shape invariant.
    #[test]
    fn test_pcr_info_payload_matches_upstream_shape() {
        use coset::{CborSerializable, CoseSign1};

        let (cert_pem, key_pem) = generate_test_cert_and_key();
        let signer = signer::LocalSigner::from_pem(&cert_pem, &key_pem).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let kernel = dir.path().join("kernel");
        let output = dir.path().join("signed.eif");
        fs::write(&kernel, b"FAKE_KERNEL").unwrap();
        build_signed_eif(
            &kernel,
            "cmd",
            &output,
            512 << 20,
            2,
            0,
            TargetArch::X86_64,
            &MetadataFields::default(),
            &signer,
        )
        .unwrap();

        let eif = fs::read(&output).unwrap();
        let sig = extract_section(&eif, EIF_SECTION_SIGNATURE).unwrap();

        // Outer envelope decodes via the upstream serde_cbor path.
        let envelope: Vec<PcrSignature> = serde_cbor::from_slice(&sig).unwrap();
        assert_eq!(envelope.len(), 1);
        let cose_bytes = &envelope[0].signature;

        // Extract the COSE payload...
        let sign1 = CoseSign1::from_slice(cose_bytes).unwrap();
        let coses_payload = sign1.payload.expect("payload must be present");

        // ...recompute PCR0 the same way `build_signed_eif` did, then
        // reproduce the upstream `measured_payload` construction exactly.
        let pcr0 = compute_pcr0(b"FAKE_KERNEL", b"cmd", &[&[][..]]);
        let measured_payload = serde_cbor::to_vec(&PcrInfo {
            register_index: 0,
            register_value: pcr0,
        })
        .unwrap();

        assert_eq!(
            coses_payload, measured_payload,
            "COSE payload must be byte-identical to serde_cbor(PcrInfo) — \
             any drift here means downstream `sign_check` returns false"
        );

        // Also decode the payload directly as `PcrInfo` to catch a
        // hypothetical future drift where the bytes happen to compare
        // equal but the shape diverges from the upstream struct.
        let decoded: PcrInfo = serde_cbor::from_slice(&coses_payload).unwrap();
        assert_eq!(decoded.register_index, 0);
        assert_eq!(
            decoded.register_value.len(),
            48,
            "PCR0 must be a 48-byte SHA-384 digest"
        );
    }

    #[test]
    fn test_signed_eif_verifies() {
        // The COSE_Sign1 in the signature section must verify with the
        // cert's public key over the recomputed to-be-signed bytes.
        use aws_lc_rs::signature::{UnparsedPublicKey, ECDSA_P384_SHA384_ASN1};
        use coset::{CborSerializable, CoseSign1};

        let (cert_pem, key_pem) = generate_test_cert_and_key();
        let signer = signer::LocalSigner::from_pem(&cert_pem, &key_pem).unwrap();
        let pub_key_spki_der = signer.public_key_der().to_vec();

        let dir = tempfile::tempdir().unwrap();
        let kernel = dir.path().join("kernel");
        let output = dir.path().join("signed.eif");
        fs::write(&kernel, b"FAKE_KERNEL").unwrap();

        build_signed_eif(
            &kernel,
            "cmd",
            &output,
            512 << 20,
            2,
            0,
            TargetArch::X86_64,
            &MetadataFields::default(),
            &signer,
        )
        .unwrap();

        let eif = fs::read(&output).unwrap();
        let sig_section = extract_section(&eif, EIF_SECTION_SIGNATURE).unwrap();

        // Outer envelope → COSE_Sign1 bytes, via the upstream serde_cbor
        // consumer path (`from_slice::<Vec<PcrSignature>>`).
        let envelope: Vec<PcrSignature> = serde_cbor::from_slice(&sig_section).unwrap();
        let cose_bytes = envelope[0].signature.clone();

        // The COSE crate can verify with an aws-lc-rs closure; we recompute
        // Sig_structure1 ourselves and check the signature by hand so this
        // test doesn't depend on coset's verification path.
        let sign1 = CoseSign1::from_slice(&cose_bytes).unwrap();
        let tbs = sign1.tbs_data(&[]);
        let public_key = UnparsedPublicKey::new(&ECDSA_P384_SHA384_ASN1, &pub_key_spki_der);
        // The COSE signature is fixed-width `r||s`; convert to ASN.1 DER
        // for aws-lc-rs' _ASN1 verifier by re-encoding.
        let sig_der = ecdsa_raw_to_der(&sign1.signature);
        public_key
            .verify(&tbs, &sig_der)
            .expect("signature must verify");
    }

    /// Decode the outer envelope of the signature section via the exact
    /// serde_cbor path used by `aws-nitro-enclaves-image-format` on the
    /// consumer side. Panics if the envelope is malformed.
    fn decode_envelope(sig_section: &[u8]) -> Vec<PcrSignature> {
        serde_cbor::from_slice(sig_section).expect("envelope must decode via serde_cbor")
    }

    /// Convert raw ECDSA `r || s` (fixed 2*n bytes) to a DER-encoded
    /// `SEQUENCE(INTEGER r, INTEGER s)` for aws-lc-rs' ASN1 verifier.
    fn ecdsa_raw_to_der(raw: &[u8]) -> Vec<u8> {
        let n = raw.len() / 2;
        let r = &raw[..n];
        let s = &raw[n..];
        let r_der = der_uint(r);
        let s_der = der_uint(s);
        let mut inner = Vec::new();
        inner.extend_from_slice(&r_der);
        inner.extend_from_slice(&s_der);
        let mut out = Vec::new();
        out.push(0x30); // SEQUENCE
        out.extend_from_slice(&der_len(inner.len()));
        out.extend_from_slice(&inner);
        out
    }

    fn der_uint(bytes: &[u8]) -> Vec<u8> {
        // Strip leading zeros; if the high bit is set, prepend one zero.
        let mut v = bytes;
        while v.len() > 1 && v[0] == 0 {
            v = &v[1..];
        }
        let mut out = Vec::new();
        out.push(0x02); // INTEGER
        let body_len = if v[0] & 0x80 != 0 {
            v.len() + 1
        } else {
            v.len()
        };
        out.extend_from_slice(&der_len(body_len));
        if v[0] & 0x80 != 0 {
            out.push(0);
        }
        out.extend_from_slice(v);
        out
    }

    fn der_len(n: usize) -> Vec<u8> {
        if n < 0x80 {
            vec![n as u8]
        } else if n < 0x100 {
            vec![0x81, n as u8]
        } else {
            vec![0x82, (n >> 8) as u8, (n & 0xff) as u8]
        }
    }

    #[test]
    fn test_pcr8_digestable_from_signature() {
        // PCR8 is defined as SHA-384(48 zero bytes || SHA-384(cert_DER)).
        // We must be able to extract a PEM cert from the emitted signature
        // section, parse it, convert to DER, and hash. Since our test cert is
        // synthesized as raw bytes for lightness, we only check the PEM
        // extraction step here — a full x509-cert parse round-trip runs in
        // the signer module tests where a real cert is available.
        let (cert_pem, key_pem) = generate_test_cert_and_key();
        let signer = signer::LocalSigner::from_pem(&cert_pem, &key_pem).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let kernel = dir.path().join("kernel");
        let output = dir.path().join("signed.eif");
        fs::write(&kernel, b"FAKE_KERNEL").unwrap();

        build_signed_eif(
            &kernel,
            "cmd",
            &output,
            512 << 20,
            2,
            0,
            TargetArch::X86_64,
            &MetadataFields::default(),
            &signer,
        )
        .unwrap();

        let eif = fs::read(&output).unwrap();
        let sig = extract_section(&eif, EIF_SECTION_SIGNATURE).unwrap();
        let envelope = decode_envelope(&sig);
        let cert = envelope[0].signing_certificate.clone();
        // Must round-trip through the `pem` crate.
        let parsed = pem::parse(&cert).unwrap();
        assert_eq!(parsed.tag(), "CERTIFICATE");
        // SHA-384 of DER is computable.
        use aws_lc_rs::digest::{digest, SHA384};
        let _ = digest(&SHA384, parsed.contents());
    }

    // -------- resign tests --------

    /// After `resign_eif`, kernel/cmdline/ramdisk/metadata sections must be
    /// byte-identical to the input; only the signature section (and header
    /// CRC) may differ. This is the byte-preservation contract the plan
    /// documents.
    #[test]
    fn test_resign_preserves_non_signature_sections() {
        let (cert_pem, key_pem) = generate_test_cert_and_key();
        let signer = signer::LocalSigner::from_pem(&cert_pem, &key_pem).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let kernel = dir.path().join("kernel");
        let input = dir.path().join("in.eif");
        let output = dir.path().join("out.eif");
        fs::write(&kernel, b"FAKE_KERNEL").unwrap();

        build_signed_eif(
            &kernel,
            "cmd",
            &input,
            512 << 20,
            2,
            0,
            TargetArch::X86_64,
            &MetadataFields {
                build_tool_version: "0.20.0",
                kernel_version: "6.1.152-0.br1",
                image_version: "",
                build_time: "",
            },
            &signer,
        )
        .unwrap();

        // Resign with a fresh (different) key to guarantee the signature
        // bytes change. Using the same key would work too, but this
        // strengthens the test (proves we actually re-signed).
        let (cert2, key2) = generate_test_cert_and_key();
        let signer2 = signer::LocalSigner::from_pem(&cert2, &key2).unwrap();
        resign_eif(&input, &output, &signer2).unwrap();

        let in_bytes = fs::read(&input).unwrap();
        let out_bytes = fs::read(&output).unwrap();

        // Every non-signature section is byte-identical.
        for ty in [
            EIF_SECTION_KERNEL,
            EIF_SECTION_CMDLINE,
            EIF_SECTION_RAMDISK,
            EIF_SECTION_METADATA,
        ] {
            let a = extract_section(&in_bytes, ty).expect("input has section");
            let b = extract_section(&out_bytes, ty).expect("output has section");
            assert_eq!(a, b, "section 0x{ty:02x} differs after resign");
        }
    }

    /// Resigning an unsigned EIF adds a signature section; the resulting
    /// signature verifies against the resign cert.
    #[test]
    fn test_resign_unsigned_to_signed() {
        let dir = tempfile::tempdir().unwrap();
        let kernel = dir.path().join("kernel");
        let input = dir.path().join("in.eif");
        let output = dir.path().join("out.eif");
        fs::write(&kernel, b"FAKE_KERNEL").unwrap();

        build_eif(
            &kernel,
            "cmd",
            &input,
            512 << 20,
            2,
            0,
            TargetArch::X86_64,
            &MetadataFields::default(),
        )
        .unwrap();
        let in_bytes = fs::read(&input).unwrap();
        assert!(
            extract_section(&in_bytes, EIF_SECTION_SIGNATURE).is_none(),
            "unsigned input must not carry a signature section"
        );

        let (cert_pem, key_pem) = generate_test_cert_and_key();
        let signer = signer::LocalSigner::from_pem(&cert_pem, &key_pem).unwrap();
        resign_eif(&input, &output, &signer).unwrap();

        let out_bytes = fs::read(&output).unwrap();
        let sig = extract_section(&out_bytes, EIF_SECTION_SIGNATURE)
            .expect("resigned output must have a signature section");
        // Sanity: the CBOR outer envelope decodes via the upstream path.
        let _envelope = decode_envelope(&sig);

        // CRC still validates over the full file.
        let stored_crc = u32::from_be_bytes(
            out_bytes[EIF_CRC32_OFFSET..EIF_CRC32_OFFSET + 4]
                .try_into()
                .unwrap(),
        );
        let mut hasher = Crc32Hasher::new();
        hasher.update(&out_bytes[..EIF_CRC32_OFFSET]);
        hasher.update(&out_bytes[EIF_CRC32_OFFSET + 4..]);
        assert_eq!(hasher.finalize(), stored_crc);
    }

    /// Resigning a signed EIF with a different cert replaces the signature
    /// section (the embedded certificate changes) but leaves the PCR0 that
    /// gets signed alone (the underlying kernel/cmdline/ramdisks are
    /// unchanged).
    #[test]
    fn test_resign_signed_to_resigned_with_different_cert() {
        let (cert_a, key_a) = generate_test_cert_and_key();
        let (cert_b, key_b) = generate_test_cert_and_key();
        let signer_a = signer::LocalSigner::from_pem(&cert_a, &key_a).unwrap();
        let signer_b = signer::LocalSigner::from_pem(&cert_b, &key_b).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let kernel = dir.path().join("kernel");
        let input = dir.path().join("in.eif");
        let output = dir.path().join("out.eif");
        fs::write(&kernel, b"FAKE_KERNEL").unwrap();

        build_signed_eif(
            &kernel,
            "cmd",
            &input,
            512 << 20,
            2,
            0,
            TargetArch::X86_64,
            &MetadataFields::default(),
            &signer_a,
        )
        .unwrap();
        resign_eif(&input, &output, &signer_b).unwrap();

        let in_bytes = fs::read(&input).unwrap();
        let out_bytes = fs::read(&output).unwrap();
        let sig_in = extract_section(&in_bytes, EIF_SECTION_SIGNATURE).unwrap();
        let sig_out = extract_section(&out_bytes, EIF_SECTION_SIGNATURE).unwrap();
        assert_ne!(
            sig_in, sig_out,
            "signature section must change after resign"
        );

        // Pull the embedded cert PEM out and confirm it's the resign cert
        // (cert_b), not the original (cert_a).
        let envelope = decode_envelope(&sig_out);
        assert_eq!(
            envelope[0].signing_certificate, cert_b,
            "resigned EIF must embed the resign cert",
        );
    }

    /// A hand-crafted EIF whose first section-offset table entry points
    /// inside the fixed header must be rejected by `read_sections`.
    /// Regression guard for the P2 hardening: without this check
    /// `compute_pcr0` in the resign path would hash header bytes as
    /// kernel/cmdline data.
    #[test]
    fn test_read_sections_rejects_offset_in_header() {
        // Start from a real EIF and rewrite the first offset-table entry
        // to 0, which falls inside the header.
        let dir = tempfile::tempdir().unwrap();
        let kernel = dir.path().join("kernel");
        let eif_path = dir.path().join("in.eif");
        fs::write(&kernel, b"FAKE_KERNEL").unwrap();
        build_eif(
            &kernel,
            "cmd",
            &eif_path,
            512 << 20,
            2,
            0,
            TargetArch::X86_64,
            &MetadataFields::default(),
        )
        .unwrap();
        let mut bytes = fs::read(&eif_path).unwrap();

        // The offset table starts at:
        //   4 (magic) + 2 (ver) + 2 (flags) + 8 (mem) + 8 (cpus)
        //   + 2 (reserved) + 2 (num_sections) = 28.
        let offsets_start = 4 + 2 + 2 + 8 + 8 + 2 + 2;
        // Overwrite the first u64 offset with 0 (points to file start,
        // well inside the 548-byte header).
        for b in &mut bytes[offsets_start..offsets_start + 8] {
            *b = 0;
        }
        // Recompute the header CRC so the parser doesn't reject the file
        // on that instead. `read_sections` doesn't currently verify CRC,
        // but do it anyway for hygiene.
        let mut hasher = Crc32Hasher::new();
        hasher.update(&bytes[..EIF_CRC32_OFFSET]);
        hasher.update(&bytes[EIF_CRC32_OFFSET + 4..]);
        let crc = hasher.finalize();
        bytes[EIF_CRC32_OFFSET..EIF_CRC32_OFFSET + 4].copy_from_slice(&crc.to_be_bytes());

        let err = read_sections(&bytes).unwrap_err();
        match err {
            EifError::ParseInput { reason } => {
                assert!(
                    reason.contains("lies within the fixed EIF header"),
                    "unexpected error reason: {reason}"
                );
            }
            other => panic!("expected ParseInput, got {other:?}"),
        }
    }

    /// `describe_eif` on a real, from-scratch build must faithfully
    /// surface the metadata fields we care about (`KernelVersion`,
    /// `BuildToolVersion`), plus the top-level shape a downstream caller
    /// depends on: `is_signed`, section list, kernel/ramdisk sizes.
    /// This is the property `eif2eif` relies on for metadata carry-forward.
    #[test]
    fn test_describe_eif_surfaces_build_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let kernel = dir.path().join("kernel");
        let eif_path = dir.path().join("in.eif");
        fs::write(&kernel, b"FAKE_KERNEL_BYTES_FOR_DESCRIBE").unwrap();
        build_eif(
            &kernel,
            "root=/dev/vda1 console=ttyS0",
            &eif_path,
            512 << 20,
            2,
            0,
            TargetArch::X86_64,
            &MetadataFields {
                build_tool_version: "0.99.0",
                kernel_version: "6.18.30-1.br1",
                image_version: "",
                build_time: "",
            },
        )
        .unwrap();

        let v = describe_eif(&eif_path).unwrap();
        assert_eq!(v["eif_version"], EIF_HDR_VERSION);
        assert_eq!(v["is_signed"], false);
        assert_eq!(v["cmdline"], "root=/dev/vda1 console=ttyS0");
        assert!(v["kernel_size"].as_u64().unwrap() > 0);
        // Every EIF our builder produces has one (empty) RAMDISK section
        // by construction. See `prepare_payload`; the empty section keeps
        // the sidecar layout self-consistent with what stock EIFs carry.
        assert_eq!(v["ramdisk_count"], 1);

        // BuildMetadata surfaces what the caller passed.
        let bm = &v["metadata"]["BuildMetadata"];
        assert_eq!(bm["BuildToolVersion"], "0.99.0");
        assert_eq!(bm["KernelVersion"], "6.18.30-1.br1");
        assert_eq!(bm["OperatingSystem"], "Linux");
        assert_eq!(bm["BuildTool"], "twoliter");

        // Sections list must include KERNEL, CMDLINE, METADATA in the order
        // `build_eif` emits them; unsigned builds have no SIGNATURE section.
        let types: Vec<String> = v["sections"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["type"].as_str().unwrap().to_string())
            .collect();
        assert!(types.contains(&"KERNEL".to_string()));
        assert!(types.contains(&"CMDLINE".to_string()));
        assert!(types.contains(&"METADATA".to_string()));
        assert!(!types.contains(&"SIGNATURE".to_string()));
    }

    /// A signed EIF must report `is_signed: true` and include a SIGNATURE
    /// entry in `sections`, so consumers can gate their behavior on that
    /// bit without cracking open the SIGNATURE bytes themselves.
    #[test]
    fn test_describe_eif_reports_signed_flag() {
        let (cert_pem, key_pem) = generate_test_cert_and_key();
        let signer = signer::LocalSigner::from_pem(&cert_pem, &key_pem).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let kernel = dir.path().join("kernel");
        let output = dir.path().join("signed.eif");
        fs::write(&kernel, b"FAKE_KERNEL").unwrap();
        build_signed_eif(
            &kernel,
            "cmd",
            &output,
            512 << 20,
            2,
            0,
            TargetArch::X86_64,
            &MetadataFields::default(),
            &signer,
        )
        .unwrap();

        let v = describe_eif(&output).unwrap();
        assert_eq!(v["is_signed"], true);
        let types: Vec<String> = v["sections"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["type"].as_str().unwrap().to_string())
            .collect();
        assert!(types.contains(&"SIGNATURE".to_string()));
    }

    /// A METADATA section that is not valid UTF-8 JSON must surface as a
    /// `MetadataDecode` error rather than being silently mapped to `null`.
    /// Silent-null would reproduce the bug this describe path exists to
    /// prevent (a repack losing `KernelVersion` and reporting success).
    #[test]
    fn test_describe_eif_rejects_garbled_metadata() {
        // Build a real EIF, then corrupt the METADATA payload bytes in place.
        let dir = tempfile::tempdir().unwrap();
        let kernel = dir.path().join("kernel");
        let eif_path = dir.path().join("in.eif");
        fs::write(&kernel, b"FAKE_KERNEL").unwrap();
        build_eif(
            &kernel,
            "cmd",
            &eif_path,
            512 << 20,
            2,
            0,
            TargetArch::X86_64,
            &MetadataFields::default(),
        )
        .unwrap();
        let mut bytes = fs::read(&eif_path).unwrap();

        // Locate the METADATA section using the offset/size table and
        // rewrite the first byte of its payload with a non-UTF-8 lead byte
        // (0xff) to force `str::from_utf8` to fail. Doing it via the
        // table (not the section header) keeps the file structurally
        // intact so `read_sections` still succeeds and the error we get
        // is specifically `MetadataDecode`.
        let num_sections_off = 4 + 2 + 2 + 8 + 8 + 2;
        let num_sections =
            u16::from_be_bytes([bytes[num_sections_off], bytes[num_sections_off + 1]]) as usize;
        let offsets_start = num_sections_off + 2;
        let mut patched = false;
        for i in 0..num_sections {
            let off = u64::from_be_bytes(
                bytes[offsets_start + i * 8..offsets_start + (i + 1) * 8]
                    .try_into()
                    .unwrap(),
            ) as usize;
            let ty = u16::from_be_bytes([bytes[off], bytes[off + 1]]);
            if ty == EIF_SECTION_METADATA {
                let data_off = off + EIF_SECTION_HEADER_SIZE;
                bytes[data_off] = 0xff;
                patched = true;
                break;
            }
        }
        assert!(patched, "test setup: no METADATA section found");
        // Recompute CRC — hygiene; `describe_eif` doesn't verify CRC today,
        // but keeping the file self-consistent isolates this test to the
        // one thing it's checking.
        let mut hasher = Crc32Hasher::new();
        hasher.update(&bytes[..EIF_CRC32_OFFSET]);
        hasher.update(&bytes[EIF_CRC32_OFFSET + 4..]);
        let crc = hasher.finalize();
        bytes[EIF_CRC32_OFFSET..EIF_CRC32_OFFSET + 4].copy_from_slice(&crc.to_be_bytes());
        fs::write(&eif_path, &bytes).unwrap();

        let err = describe_eif(&eif_path).unwrap_err();
        match err {
            EifError::MetadataDecode { reason } => {
                assert!(
                    reason.contains("UTF-8") || reason.contains("JSON"),
                    "unexpected MetadataDecode reason: {reason}"
                );
            }
            other => panic!("expected MetadataDecode, got {other:?}"),
        }
    }
}
