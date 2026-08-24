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
/// EIF header version. v4 adds the METADATA section (0x05) and retains the
/// v3 SIGNATURE section (0x04).
const EIF_HDR_VERSION: u16 = 4;

const EIF_SECTION_KERNEL: u16 = 1;
const EIF_SECTION_CMDLINE: u16 = 2;
const EIF_SECTION_RAMDISK: u16 = 3;
const EIF_SECTION_SIGNATURE: u16 = 4;
const EIF_SECTION_METADATA: u16 = 5;

/// Human-readable EIF section type name; `"UNKNOWN"` for unregistered types.
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

/// Upper bound on the CBOR-encoded EIF signature section, per the spec.
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
    CborEncodeSignature {
        source: minicbor_serde::error::EncodeError<std::convert::Infallible>,
    },

    #[snafu(display("failed to CBOR-encode signature payload: {source}"))]
    CborEncodePayload {
        source: minicbor_serde::error::EncodeError<std::convert::Infallible>,
    },

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
/// Fixed-size header carries offset/size tables (up to `MAX_NUM_SECTIONS`
/// entries); sections are laid out in `entries` order; CRC32 is computed
/// over the whole buffer with the CRC field zeroed.
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

    // Precompute offsets and sizes so the header table and section writes stay in lockstep.
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

/// Bundle of prepared section data (kernel/cmdline/ramdisk/metadata) shared
/// by the unsigned and signed builders.
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
    Ok(KernelPayload {
        kernel: kernel_data,
        cmdline: cmdline.as_bytes().to_vec(),
        // Empty RAMDISK section; sidecar mounts rootfs via virtio-blk + dm-verity.
        // NOTE: stock nitro-cli produces two ramdisks, so PCR2 consumers see empty input here.
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
/// `target_arch` selects the header architecture flag and must match the
/// kernel image. `metadata` populates the METADATA section; missing fields
/// default to empty strings (all keys are always emitted).
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

    // Layout: KERNEL, CMDLINE, RAMDISK, METADATA.
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

/// Build and write a signed EIF.
///
/// Layout: KERNEL, CMDLINE, RAMDISK, METADATA, SIGNATURE (last).
/// PCR0 = `SHA-384(48 zero bytes || SHA-384(kernel || cmdline || ramdisk_data))`;
/// metadata and signature sections are excluded from the hash.
#[allow(clippy::too_many_arguments)]
pub async fn build_signed_eif(
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
    let signature_bytes = build_signature_section(signer, &pcr0).await?;

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

/// One parsed EIF section: type + payload (section header stripped).
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

/// Parse the section list from a serialized EIF.
///
/// Sections come back in header-table order; the 12-byte section headers are
/// validated against the table and stripped.
pub fn read_sections(eif_bytes: &[u8]) -> Result<Vec<ParsedSection>, EifError> {
    read_header(eif_bytes)?;
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
        // Reject offsets inside the header so `compute_pcr0` can't be tricked
        // into hashing header bytes as section data.
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
        // Cross-check the size in the section header against the size table.
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

/// Summarize an EIF's contents as JSON.
///
/// `metadata` is `null` when no METADATA section is present. `cmdline` is
/// lossy-decoded UTF-8 with an optional trailing NUL stripped.
/// Fails with `MetadataDecode` when METADATA is present but not UTF-8 JSON.
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

    // METADATA must be valid UTF-8 JSON when present; fail loudly rather than substitute null.
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

    // Strip optional trailing NUL, lossy-decode UTF-8.
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

/// Re-sign an EIF: replace (or append) its SIGNATURE section, byte-preserving
/// all other sections. Kernel bytes are treated as opaque (no `prepare_kernel`
/// re-run) so PCR0 covers exactly what's on disk. Any existing SIGNATURE is
/// dropped; a new one is appended last.
pub async fn resign_eif(input: &Path, output: &Path, signer: &dyn Signer) -> Result<(), EifError> {
    let bytes = fs::read(input).context(ReadInputSnafu { path: input })?;
    let header = read_header(&bytes)?;
    let sections = read_sections(&bytes)?;

    // PCR0 is computed from the input's on-disk bytes so we don't need to know
    // how they were prepared.
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
    // RAMDISK is not required; `compute_pcr0` accepts an empty slice.
    let pcr0 = compute_pcr0(kernel, cmdline, &ramdisks);
    let signature_bytes = build_signature_section(signer, &pcr0).await?;

    // Preserve non-signature sections in order; append the new signature last.
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
    // Check MAX_NUM_SECTIONS here so the error attributes overflow to resign.
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

/// Compute PCR0 = `SHA-384(48 zero bytes || SHA-384(kernel || cmdline || ramdisks))`,
/// matching the reference `EifBuilder::image_hasher`.
fn compute_pcr0(kernel_data: &[u8], cmdline_data: &[u8], ramdisks: &[&[u8]]) -> Vec<u8> {
    use aws_lc_rs::digest::{digest, SHA384};

    let mut inner = Vec::with_capacity(kernel_data.len() + cmdline_data.len() + 64);
    inner.extend_from_slice(kernel_data);
    inner.extend_from_slice(cmdline_data);
    for r in ramdisks {
        inner.extend_from_slice(r);
    }
    let inner_digest = digest(&SHA384, &inner);

    let mut outer_input = vec![0u8; 48];
    outer_input.extend_from_slice(inner_digest.as_ref());
    let outer_digest = digest(&SHA384, &outer_input);
    outer_digest.as_ref().to_vec()
}

/// Wire-compatible mirror of upstream `PcrInfo`. Serde encodes `Vec<u8>`
/// as a CBOR array-of-integers, matching what the verifier reconstructs
/// and byte-compares. Field names, order, and types must match upstream
/// exactly.
#[derive(Serialize, Deserialize)]
struct PcrInfo {
    register_index: i32, // do not widen: upstream uses i32
    register_value: Vec<u8>,
}

/// Wire-compatible mirror of upstream `PcrSignature`. Must use `Vec<u8>`
/// (not `serde_bytes::ByteBuf`) so serde-based CBOR decodes match upstream.
#[derive(Serialize, Deserialize)]
struct PcrSignature {
    signing_certificate: Vec<u8>,
    signature: Vec<u8>,
}

/// Build CBOR bytes for the `EifSectionSignature` section:
/// `Array(1) { Map { "signing_certificate": PEM, "signature": COSE_Sign1 } }`.
/// Byte fields are CBOR array-of-integers (serde's default `Vec<u8>` shape),
/// matching what the upstream consumer expects.
async fn build_signature_section(signer: &dyn Signer, pcr0: &[u8]) -> Result<Vec<u8>, EifError> {
    use coset::{CborSerializable, CoseSign1Builder, HeaderBuilder};

    let payload = minicbor_serde::to_vec(&PcrInfo {
        register_index: 0,
        register_value: pcr0.to_vec(),
    })
    .context(CborEncodePayloadSnafu)?;

    let alg = match signer.algorithm() {
        signer::SignAlg::Es256 => coset::iana::Algorithm::ES256,
        signer::SignAlg::Es384 => coset::iana::Algorithm::ES384,
    };
    let protected = HeaderBuilder::new().algorithm(alg).build();

    // `CoseSign1Builder::{create,try_create}_signature` take a sync closure,
    // so we build the shell first, compute the `Sig_structure1` TBS bytes,
    // await the async signer, then attach the signature. Semantically
    // identical to `try_create_signature(&[], sign)`.
    let mut sign1 = CoseSign1Builder::new()
        .protected(protected)
        .payload(payload)
        .build();
    let tbs = sign1.tbs_data(&[]);
    sign1.signature = signer.sign_cose(&tbs).await.context(SignSnafu)?;

    let cose_bytes = sign1.to_vec().context(CoseBuildSnafu)?;
    let cose_size = cose_bytes.len();

    let cert_pem = signer.cert_pem().to_vec();
    let cert_pem_size = cert_pem.len();

    let envelope = vec![PcrSignature {
        signing_certificate: cert_pem,
        signature: cose_bytes,
    }];
    let out = minicbor_serde::to_vec(&envelope).context(CborEncodeSignatureSnafu)?;
    // Enforce SIGNATURE_MAX_SIZE here; the error attributes bytes to cert vs. COSE.
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

/// Shared DER TLV helper for test code (short + long-form length encoding).
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
        // Keys must be present with empty values, not omitted.
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
        // Header arch must reflect the target, not the build host.
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("test.eif");

        // x86_64 pass-through: any non-empty bytes work.
        let x86_kernel = dir.path().join("x86-kernel");
        fs::write(&x86_kernel, b"FAKE_KERNEL").unwrap();

        // Minimal arm64 PE Image shape: MZ header + ARM\x64 at offset 56.
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
            // flags is u16 at offset 6; PCIE bits are 0 so full value equals arch bits.
            let flags = u16::from_be_bytes([eif[6], eif[7]]);
            assert_eq!(flags, expected_low_bits, "arch={arch:?}");
        }
    }

    #[test]
    fn test_arm64_non_pe_kernel_is_rejected() {
        // Non-PE arm64 kernels must be rejected upfront, not at launch time.
        let dir = tempfile::tempdir().unwrap();
        let kernel = dir.path().join("vmlinuz.gz");
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
        // Recompute PCR0 independently to guard against drift in `compute_pcr0`.
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
        // First section header (KERNEL) lives at EIF_HEADER_SIZE.
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
        let ty = u16::from_be_bytes([eif[EIF_HEADER_SIZE], eif[EIF_HEADER_SIZE + 1]]);
        assert_eq!(ty, EIF_SECTION_KERNEL);
    }

    /// Build a self-signed ECDSA P-384 cert + private key for tests. The DER
    /// is hand-rolled to avoid pulling in `x509-cert`'s `builder` feature.
    /// The cert's own `signatureValue` is not a valid signature; only the
    /// COSE signature is verified downstream.
    #[cfg(test)]
    fn generate_test_cert_and_key() -> (Vec<u8>, Vec<u8>) {
        use aws_lc_rs::signature::{EcdsaKeyPair, KeyPair, ECDSA_P384_SHA384_ASN1_SIGNING};
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P384_SHA384_ASN1_SIGNING, &rng).unwrap();
        let pem_key = pem::Pem::new("PRIVATE KEY", pkcs8.as_ref().to_vec());
        let key_pem = pem::encode(&pem_key).into_bytes();

        let key_pair =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P384_SHA384_ASN1_SIGNING, pkcs8.as_ref()).unwrap();
        let sec1_pub = key_pair.public_key().as_ref().to_vec();

        let cert_der = build_p384_self_signed_der(&sec1_pub);
        let cert_pem_obj = pem::Pem::new("CERTIFICATE", cert_der);
        let cert_pem = pem::encode(&cert_pem_obj).into_bytes();

        (cert_pem, key_pem)
    }

    /// Minimal syntactically-valid X.509 cert with a P-384 SPKI over the
    /// given SEC1 public key.
    #[cfg(test)]
    fn build_p384_self_signed_der(sec1_pub: &[u8]) -> Vec<u8> {
        // id-ecPublicKey 1.2.840.10045.2.1, secp384r1 1.3.132.0.34,
        // ecdsa-with-SHA384 1.2.840.10045.4.3.3, id-at-commonName 2.5.4.3.
        let id_ec_pub = [0x06u8, 0x07, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x02, 0x01];
        let secp384r1 = [0x06u8, 0x05, 0x2B, 0x81, 0x04, 0x00, 0x22];
        let ecdsa_sha384 = [0x06u8, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x03];
        let id_cn = [0x06u8, 0x03, 0x55, 0x04, 0x03];

        let mut spki_alg_body = Vec::new();
        spki_alg_body.extend_from_slice(&id_ec_pub);
        spki_alg_body.extend_from_slice(&secp384r1);
        let spki_alg = crate::der_helpers::wrap_der(0x30, &spki_alg_body);

        let mut spki_key = vec![0u8];
        spki_key.extend_from_slice(sec1_pub);
        let spki_key_bs = crate::der_helpers::wrap_der(0x03, &spki_key);

        let mut spki_body = Vec::new();
        spki_body.extend_from_slice(&spki_alg);
        spki_body.extend_from_slice(&spki_key_bs);
        let spki = crate::der_helpers::wrap_der(0x30, &spki_body);

        // Name = SEQUENCE OF RDN(SET OF ATV(SEQUENCE OID + UTF8String("test"))).
        let cn_value = crate::der_helpers::wrap_der(0x0C, b"test"); // UTF8String
        let mut atv = Vec::new();
        atv.extend_from_slice(&id_cn);
        atv.extend_from_slice(&cn_value);
        let atv_seq = crate::der_helpers::wrap_der(0x30, &atv);
        let rdn = crate::der_helpers::wrap_der(0x31, &atv_seq); // SET
        let name = crate::der_helpers::wrap_der(0x30, &rdn); // SEQUENCE OF RDN

        // Fixed UTCTime range well in the past/future to satisfy validity checks.
        let not_before = crate::der_helpers::wrap_der(0x17, b"200101000000Z"); // UTCTime
        let not_after = crate::der_helpers::wrap_der(0x17, b"400101000000Z");
        let mut validity_body = Vec::new();
        validity_body.extend_from_slice(&not_before);
        validity_body.extend_from_slice(&not_after);
        let validity = crate::der_helpers::wrap_der(0x30, &validity_body);

        let sig_alg = crate::der_helpers::wrap_der(0x30, &ecdsa_sha384);

        // TBSCertificate: version[0] serial sigAlg issuer validity subject SPKI.
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

        // Fake signatureValue: minimal BIT STRING (0 unused bits) for structural presence.
        let fake_sig_bytes = [0x00u8];
        let sig_val = crate::der_helpers::wrap_der(0x03, &fake_sig_bytes);

        let mut cert_body = Vec::new();
        cert_body.extend_from_slice(&tbs);
        cert_body.extend_from_slice(&sig_alg);
        cert_body.extend_from_slice(&sig_val);
        crate::der_helpers::wrap_der(0x30, &cert_body)
    }

    #[tokio::test]
    async fn test_signed_eif_structure() {
        // Outer envelope must decode via `minicbor_serde::from_slice::<Vec<PcrSignature>>`,
        // the exact shape the upstream consumer path uses.
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
        .await
        .unwrap();

        let eif = fs::read(&output).unwrap();
        let sig = extract_section(&eif, EIF_SECTION_SIGNATURE)
            .expect("signature section must be present");

        let envelope: Vec<PcrSignature> = minicbor_serde::from_slice(&sig)
            .expect("outer envelope must decode via minicbor_serde");
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

    /// Scan an EIF for a section of the given type and return its payload,
    /// using the header offset+size tables.
    fn extract_section(eif: &[u8], ty: u16) -> Option<Vec<u8>> {
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
            let this_ty = u16::from_be_bytes([eif[off], eif[off + 1]]);
            if this_ty == ty {
                let data_off = off + EIF_SECTION_HEADER_SIZE;
                return Some(eif[data_off..data_off + size].to_vec());
            }
        }
        None
    }

    #[tokio::test]
    async fn test_signed_eif_crc_valid() {
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
        .await
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

    /// The COSE payload we emit must be byte-identical to
    /// serde-driven `to_vec(&PcrInfo)` on the verifier side, or `sign_check`
    /// silently returns false.
    #[tokio::test]
    async fn test_pcr_info_payload_matches_upstream_shape() {
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
        .await
        .unwrap();

        let eif = fs::read(&output).unwrap();
        let sig = extract_section(&eif, EIF_SECTION_SIGNATURE).unwrap();

        let envelope: Vec<PcrSignature> = minicbor_serde::from_slice(&sig).unwrap();
        assert_eq!(envelope.len(), 1);
        let cose_bytes = &envelope[0].signature;

        let sign1 = CoseSign1::from_slice(cose_bytes).unwrap();
        let coses_payload = sign1.payload.expect("payload must be present");

        // Recompute PCR0 and the upstream `measured_payload` for exact byte compare.
        let pcr0 = compute_pcr0(b"FAKE_KERNEL", b"cmd", &[&[][..]]);
        let measured_payload = minicbor_serde::to_vec(&PcrInfo {
            register_index: 0,
            register_value: pcr0,
        })
        .unwrap();

        assert_eq!(
            coses_payload, measured_payload,
            "COSE payload must be byte-identical to serde-CBOR(PcrInfo) — \
             any drift here means downstream `sign_check` returns false"
        );

        // Also decode as `PcrInfo` to catch shape drift that happens to byte-match.
        let decoded: PcrInfo = minicbor_serde::from_slice(&coses_payload).unwrap();
        assert_eq!(decoded.register_index, 0);
        assert_eq!(
            decoded.register_value.len(),
            48,
            "PCR0 must be a 48-byte SHA-384 digest"
        );
    }

    #[tokio::test]
    async fn test_signed_eif_verifies() {
        // COSE_Sign1 must verify with the cert's public key over recomputed TBS.
        use aws_lc_rs::signature::{UnparsedPublicKey, ECDSA_P384_SHA384_ASN1};
        use coset::{CborSerializable, CoseSign1};

        let (cert_pem, key_pem) = generate_test_cert_and_key();
        let signer = signer::LocalSigner::from_pem(&cert_pem, &key_pem).unwrap();
        let pub_key_spki_der = signer.public_key_der();

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
        .await
        .unwrap();

        let eif = fs::read(&output).unwrap();
        let sig_section = extract_section(&eif, EIF_SECTION_SIGNATURE).unwrap();

        let envelope: Vec<PcrSignature> = minicbor_serde::from_slice(&sig_section).unwrap();
        let cose_bytes = envelope[0].signature.clone();

        // Recompute Sig_structure1 and verify directly (no dependence on coset).
        let sign1 = CoseSign1::from_slice(&cose_bytes).unwrap();
        let tbs = sign1.tbs_data(&[]);
        let public_key = UnparsedPublicKey::new(&ECDSA_P384_SHA384_ASN1, &pub_key_spki_der);
        // aws-lc-rs' _ASN1 verifier needs DER; re-encode from raw `r||s`.
        let sig_der = ecdsa_raw_to_der(&sign1.signature);
        public_key
            .verify(&tbs, &sig_der)
            .expect("signature must verify");
    }

    /// Decode the signature section outer envelope via the upstream serde CBOR path.
    fn decode_envelope(sig_section: &[u8]) -> Vec<PcrSignature> {
        minicbor_serde::from_slice(sig_section).expect("envelope must decode via minicbor_serde")
    }

    /// Convert raw ECDSA `r || s` to DER `SEQUENCE(INTEGER r, INTEGER s)`.
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

    #[tokio::test]
    async fn test_pcr8_digestable_from_signature() {
        // PCR8 = SHA-384(48 zero bytes || SHA-384(cert_DER)); verify PEM
        // extraction + digest steps are reachable from the signature section.
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
        .await
        .unwrap();

        let eif = fs::read(&output).unwrap();
        let sig = extract_section(&eif, EIF_SECTION_SIGNATURE).unwrap();
        let envelope = decode_envelope(&sig);
        let cert = envelope[0].signing_certificate.clone();
        let parsed = pem::parse(&cert).unwrap();
        assert_eq!(parsed.tag(), "CERTIFICATE");
        use aws_lc_rs::digest::{digest, SHA384};
        let _ = digest(&SHA384, parsed.contents());
    }

    // -------- resign tests --------

    /// Non-signature sections must be byte-identical after `resign_eif`.
    #[tokio::test]
    async fn test_resign_preserves_non_signature_sections() {
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
        .await
        .unwrap();

        // Resign with a fresh key so the signature bytes must change.
        let (cert2, key2) = generate_test_cert_and_key();
        let signer2 = signer::LocalSigner::from_pem(&cert2, &key2).unwrap();
        resign_eif(&input, &output, &signer2).await.unwrap();

        let in_bytes = fs::read(&input).unwrap();
        let out_bytes = fs::read(&output).unwrap();

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

    /// Resigning an unsigned EIF appends a SIGNATURE section and keeps CRC valid.
    #[tokio::test]
    async fn test_resign_unsigned_to_signed() {
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
        resign_eif(&input, &output, &signer).await.unwrap();

        let out_bytes = fs::read(&output).unwrap();
        let sig = extract_section(&out_bytes, EIF_SECTION_SIGNATURE)
            .expect("resigned output must have a signature section");
        let _envelope = decode_envelope(&sig);

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

    /// Resigning a signed EIF with a different cert swaps the SIGNATURE section
    /// but preserves PCR0.
    #[tokio::test]
    async fn test_resign_signed_to_resigned_with_different_cert() {
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
        .await
        .unwrap();
        resign_eif(&input, &output, &signer_b).await.unwrap();

        let in_bytes = fs::read(&input).unwrap();
        let out_bytes = fs::read(&output).unwrap();
        let sig_in = extract_section(&in_bytes, EIF_SECTION_SIGNATURE).unwrap();
        let sig_out = extract_section(&out_bytes, EIF_SECTION_SIGNATURE).unwrap();
        assert_ne!(
            sig_in, sig_out,
            "signature section must change after resign"
        );

        let envelope = decode_envelope(&sig_out);
        assert_eq!(
            envelope[0].signing_certificate, cert_b,
            "resigned EIF must embed the resign cert",
        );
    }

    /// Section offsets inside the fixed header must be rejected.
    #[test]
    fn test_read_sections_rejects_offset_in_header() {
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

        // Overwrite the first offset-table entry with 0 (inside the header).
        let offsets_start = 4 + 2 + 2 + 8 + 8 + 2 + 2;
        for b in &mut bytes[offsets_start..offsets_start + 8] {
            *b = 0;
        }
        // Recompute CRC for hygiene; keeps the failure attributable to the offset check.
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

    /// `describe_eif` surfaces BuildMetadata and the top-level shape that
    /// downstream carry-forward relies on.
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
        // Every builder-produced EIF has exactly one (empty) RAMDISK section.
        assert_eq!(v["ramdisk_count"], 1);

        let bm = &v["metadata"]["BuildMetadata"];
        assert_eq!(bm["BuildToolVersion"], "0.99.0");
        assert_eq!(bm["KernelVersion"], "6.18.30-1.br1");
        assert_eq!(bm["OperatingSystem"], "Linux");
        assert_eq!(bm["BuildTool"], "twoliter");

        // Unsigned builds emit KERNEL, CMDLINE, METADATA and no SIGNATURE.
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

    /// Signed EIFs report `is_signed: true` and list SIGNATURE in `sections`.
    #[tokio::test]
    async fn test_describe_eif_reports_signed_flag() {
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
        .await
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

    /// Garbled METADATA must surface as `MetadataDecode`, never silent `null`.
    #[test]
    fn test_describe_eif_rejects_garbled_metadata() {
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

        // Corrupt the first METADATA payload byte to 0xff so UTF-8 decode fails
        // while `read_sections` still succeeds.
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
        // Recompute CRC for hygiene.
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
