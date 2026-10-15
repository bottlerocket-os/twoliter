//! File-level comparison of partitions whose bytes are expected to differ.

use crate::error::Result;
use crate::eventlog;
use crate::kmod;
use crate::verity;
use crc::{Algorithm, Crc};
use ext4_view::Ext4;
use fatfs::{FileSystem, FsOptions};
use hmac::{Hmac, KeyInit, Mac};
use serde::Serialize;
use serde_json::{json, Value};
use sha2_0_11::{Digest, Sha256, Sha512};
use snafu::{ensure_whatever, OptionExt, ResultExt};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{Cursor, Read};

/// Signatures and signing certificates, which may differ if both images have them.
const SIGNING_ARTIFACT_SUFFIXES: &[&str] = &[".sig", ".crt", ".cer", ".der", ".auth", ".esl"];

/// FIPS HMACs of boot files, written by `openssl sha512 -hmac`.
const HMAC_SUFFIX: &[u8] = b".hmac";

/// The well-known key of the FIPS boot file HMACs.
const HMAC_KEY: &[u8] = b"FIPS-FTW-RHT2009";

/// The GRUB configuration in BOOT-A, which holds the kernel command line.
pub const GRUB_CFG: &[u8] = b"/grub/grub.cfg";

/// Where the firmware finds the UKI on the EFI system partition of a UKI image.
pub const UKI_PATHS: &[&str] = &["/EFI/BOOT/BOOTX64.EFI", "/EFI/BOOT/BOOTAA64.EFI"];

/// Whether `path` is where the firmware finds a UKI. FAT names are case-insensitive.
pub fn is_uki_path(path: &[u8]) -> bool {
    UKI_PATHS
        .iter()
        .any(|u| path.eq_ignore_ascii_case(u.as_bytes()))
}

/// The TUF root role, which rebuilds cannot reproduce without the release keys.
const TUF_ROOT_ROLE: &[u8] = b"/usr/share/updog/root.json";

/// The SBOMs in ROOT-A, which sbomtool cannot reproduce yet.
const SBOMS: &[&[u8]] = &[
    b"/usr/share/bottlerocket/spdx-sbom.json",
    b"/usr/share/bottlerocket/cyclonedx-sbom.json",
];

/// PE sections that only hold embedded keys.
const KEY_SECTIONS: &[&str] = &[".vendor_cert", ".pubkey"];

/// The PE section of a UKI that holds the kernel command line.
pub const CMDLINE_SECTION: &str = ".cmdline";

/// Largest file read into memory for a content-aware comparison.
const MAX_BUFFERED_SIZE: u64 = 256 << 20;

/// Which differences are expected in a filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rules {
    /// EFI-A and BOOT-A: signing-dependent content.
    Boot,
    /// ROOT-A: module signatures, the TUF root role and the SBOMs.
    Root,
    /// PRIVATE: none; compared only to explain a difference.
    Exact,
}

/// An absolute path within a filesystem. File names are bytes and need not be UTF-8.
pub type FsPath = Vec<u8>;

pub type Files = BTreeMap<FsPath, Vec<u8>>;

/// Extended attributes, by full name (such as `security.selinux`).
pub type Xattrs = BTreeMap<Vec<u8>, Vec<u8>>;

/// Show a path or name in reports; invalid UTF-8 is replaced.
pub fn display(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Insert `value` at `key`, failing if it is already present.
pub fn insert_unique<V>(map: &mut BTreeMap<Vec<u8>, V>, key: Vec<u8>, value: V) -> Result<()> {
    ensure_whatever!(!map.contains_key(&key), "duplicate entry {}", display(&key));
    map.insert(key, value);
    Ok(())
}

/// The keys of either map, in order.
pub fn union_keys<'a, K: Ord, V, W>(
    a: &'a BTreeMap<K, V>,
    b: &'a BTreeMap<K, W>,
) -> BTreeSet<&'a K> {
    a.keys().chain(b.keys()).collect()
}

/// The first `n` of `items`, and how many more there are, or "none".
pub fn first_n(items: &[String], n: usize) -> String {
    match items.len() {
        0 => "none".to_string(),
        len if len > n => format!("{} and {} more", items[..n].join(", "), len - n),
        _ => items.join(", "),
    }
}

/// Bounds the file data read from a partition to the partition's size.
struct ReadBudget {
    limit: u64,
    remaining: u64,
}

impl ReadBudget {
    fn new(partition: &[u8]) -> Self {
        let limit = partition.len() as u64;
        Self {
            limit,
            remaining: limit,
        }
    }

    /// Account for reading `len` bytes of `path`.
    fn take(&mut self, path: &str, len: u64) -> Result<()> {
        ensure_whatever!(
            len <= self.remaining,
            "reading {path} ({len} bytes) would read more file data than the {}-byte partition holds",
            self.limit
        );
        self.remaining -= len;
        Ok(())
    }
}

/// Read every regular file from a FAT filesystem image.
pub fn fat_files(partition: Vec<u8>) -> Result<Files> {
    let mut budget = ReadBudget::new(&partition);
    let fs = FileSystem::new(Cursor::new(partition), FsOptions::new())
        .whatever_context("failed to mount FAT filesystem")?;
    let mut files = Files::new();
    let mut stack = vec![(String::new(), fs.root_dir())];
    while let Some((prefix, dir)) = stack.pop() {
        for entry in dir.iter() {
            let entry = entry.whatever_context("failed to read FAT directory")?;
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let path = format!("{prefix}/{name}");
            if entry.is_dir() {
                stack.push((path, entry.to_dir()));
            } else {
                budget.take(&path, entry.len())?;
                let mut contents = Vec::new();
                entry
                    .to_file()
                    .take(entry.len())
                    .read_to_end(&mut contents)
                    .whatever_context(format!("failed to read {path}"))?;
                insert_unique(&mut files, path.into_bytes(), contents)?;
            }
        }
    }
    Ok(files)
}

/// Offset of the primary ext4 superblock, and the size of every copy.
const EXT4_SUPERBLOCK: usize = 1024;

/// `s_flags` bits recording the directory hash signedness: EXT2_FLAGS_SIGNED_HASH and
/// EXT2_FLAGS_UNSIGNED_HASH.
const EXT4_DIR_HASH_FLAGS: u32 = 0x3;

/// Offset of `s_flags` in an ext4 superblock.
const EXT4_S_FLAGS: usize = 0x160;

/// Offset of `s_checksum` in an ext4 superblock, which covers the bytes before it.
const EXT4_S_CHECKSUM: usize = 0x3fc;

fn le_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().expect("4 bytes"))
}

/// The ext4 superblock at `offset` in `partition`.
fn ext4_superblock(partition: &[u8], offset: usize) -> Result<&[u8]> {
    let sb = partition
        .get(offset..offset + EXT4_SUPERBLOCK)
        .whatever_context("ext4 filesystem is truncated")?;
    ensure_whatever!(
        sb[0x38..0x3a] == [0x53, 0xef],
        "no ext4 superblock at offset {offset}"
    );
    Ok(sb)
}

/// The offsets of the primary superblock and of each backup.
fn ext4_superblock_offsets(partition: &[u8]) -> Result<Vec<usize>> {
    let sb = ext4_superblock(partition, EXT4_SUPERBLOCK)?;
    let (compat, incompat, ro_compat) = (le_u32(sb, 0x5c), le_u32(sb, 0x60), le_u32(sb, 0x64));
    let log_block_size = le_u32(sb, 0x18);
    ensure_whatever!(log_block_size <= 6, "invalid ext4 block size");
    let block_size = 1024u64 << log_block_size;
    let mut blocks = u64::from(le_u32(sb, 0x4));
    // INCOMPAT_64BIT
    if incompat & 0x80 != 0 {
        blocks |= u64::from(le_u32(sb, 0x150)) << 32;
    }
    let first_data_block = u64::from(le_u32(sb, 0x14));
    let blocks_per_group = u64::from(le_u32(sb, 0x20));
    ensure_whatever!(blocks_per_group > 0, "invalid ext4 blocks per group");
    let groups = blocks
        .saturating_sub(first_data_block)
        .div_ceil(blocks_per_group);
    let is_power = |mut g: u64, base: u64| {
        while g.is_multiple_of(base) {
            g /= base;
        }
        g == 1
    };
    let has_backup = |g: u64| {
        // COMPAT_SPARSE_SUPER2 keeps at most two backups, at the groups it names.
        if compat & 0x200 != 0 {
            g == u64::from(le_u32(sb, 0x24c)) || g == u64::from(le_u32(sb, 0x250))
        // RO_COMPAT_SPARSE_SUPER keeps them in group 1 and powers of 3, 5 and 7.
        } else if ro_compat & 0x1 != 0 {
            g == 1 || is_power(g, 3) || is_power(g, 5) || is_power(g, 7)
        } else {
            true
        }
    };
    let mut offsets = vec![EXT4_SUPERBLOCK];
    for g in (1..groups).filter(|g| has_backup(*g)) {
        let offset = g
            .checked_mul(blocks_per_group)
            .and_then(|b| b.checked_add(first_data_block))
            .and_then(|b| b.checked_mul(block_size))
            .and_then(|o| usize::try_from(o).ok())
            .whatever_context("invalid ext4 backup superblock offset")?;
        ext4_superblock(partition, offset)?;
        offsets.push(offset);
    }
    Ok(offsets)
}

/// CRC32C without the final inversion, as ext4 checksums use it.
const EXT4_CRC32C: Crc<u32> = Crc::<u32>::new(&Algorithm {
    width: 32,
    poly: 0x1edc_6f41,
    init: !0,
    refin: true,
    refout: true,
    xorout: 0,
    check: 0x1cf9_6d7c,
    residue: 0,
});

fn ext4_superblock_checksum(sb: &[u8]) -> u32 {
    EXT4_CRC32C.checksum(&sb[..EXT4_S_CHECKSUM])
}

/// The directory hash flags of the primary ext4 superblock.
pub fn ext4_dir_hash_flags(partition: &[u8]) -> Result<u32> {
    let sb = ext4_superblock(partition, EXT4_SUPERBLOCK)?;
    Ok(le_u32(sb, EXT4_S_FLAGS) & EXT4_DIR_HASH_FLAGS)
}

/// How directory hash flags read in reports.
pub fn dir_hash_signedness(flags: u32) -> Option<&'static str> {
    match flags & EXT4_DIR_HASH_FLAGS {
        0x1 => Some("signed"),
        0x2 => Some("unsigned"),
        0x3 => Some("flagged both signed and unsigned"),
        _ => None,
    }
}

/// Copy `like`'s directory-hash flags into every superblock of `partition` (fixing valid
/// checksums), as a build host with `like`'s `char` signedness would write it.
pub fn ext4_copy_dir_hash_flags(partition: &mut [u8], like: &[u8]) -> Result<()> {
    let sb = ext4_superblock(partition, EXT4_SUPERBLOCK)?;
    // RO_COMPAT_METADATA_CSUM
    let checksummed = le_u32(sb, 0x64) & 0x400 != 0;
    for offset in ext4_superblock_offsets(partition)? {
        // Without a superblock there, `like` differs anyway.
        let Ok(like_sb) = ext4_superblock(like, offset) else {
            continue;
        };
        let flags = le_u32(like_sb, EXT4_S_FLAGS) & EXT4_DIR_HASH_FLAGS;
        let sb = &mut partition[offset..offset + EXT4_SUPERBLOCK];
        let valid_checksum =
            checksummed && ext4_superblock_checksum(sb) == le_u32(sb, EXT4_S_CHECKSUM);
        let s_flags = (le_u32(sb, EXT4_S_FLAGS) & !EXT4_DIR_HASH_FLAGS) | flags;
        sb[EXT4_S_FLAGS..EXT4_S_FLAGS + 4].copy_from_slice(&s_flags.to_le_bytes());
        if valid_checksum {
            let checksum = ext4_superblock_checksum(sb);
            sb[EXT4_S_CHECKSUM..].copy_from_slice(&checksum.to_le_bytes());
        }
    }
    Ok(())
}

/// Read every regular file from an ext4 filesystem image.
pub fn ext4_files(partition: Vec<u8>) -> Result<Files> {
    let mut budget = ReadBudget::new(&partition);
    let fs = Ext4::load(Box::new(partition)).whatever_context("failed to mount ext4 filesystem")?;
    let mut files = Files::new();
    let mut stack = vec![ext4_view::PathBuf::new("/")];
    while let Some(dir) = stack.pop() {
        let entries = fs
            .read_dir(&dir)
            .whatever_context(format!("failed to read directory {}", dir.display()))?;
        for entry in entries {
            let entry =
                entry.whatever_context(format!("failed to read directory {}", dir.display()))?;
            let name = entry.file_name();
            let name = name.as_ref();
            if name == b"." || name == b".." || name == b"lost+found" {
                continue;
            }
            let path = entry.path();
            let metadata = entry
                .metadata()
                .whatever_context(format!("failed to stat {}", path.display()))?;
            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.file_type().is_regular_file() {
                // Sparse files and hardlinks can claim more than the partition holds.
                budget.take(&path.display().to_string(), metadata.len())?;
                let contents = fs
                    .read(&path)
                    .whatever_context(format!("failed to read {}", path.display()))?;
                insert_unique(&mut files, path.as_ref().to_vec(), contents)?;
            }
        }
    }
    Ok(files)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Match,
    ExpectedDifference,
    Mismatch,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileReport {
    pub path: String,
    pub status: Status,
    pub detail: String,
}

fn is_pe(data: &[u8]) -> bool {
    data.starts_with(b"MZ")
}

/// Compare a PE binary. Only signatures, key sections and, if `cmdline_checked`,
/// the verity digests and data size in `.cmdline` may differ.
fn compare_pe(expected: &[u8], actual: &[u8], cmdline_checked: bool) -> (Status, String) {
    let (e, a) = match (
        pcrsys::pe::get_authenticode_hash(expected),
        pcrsys::pe::get_authenticode_hash(actual),
    ) {
        (Ok(e), Ok(a)) if e == a => {
            return (
                Status::ExpectedDifference,
                "signatures differ; Authenticode digests match".to_string(),
            )
        }
        (Ok(e), Ok(a)) => (hex::encode(e), hex::encode(a)),
        _ => {
            return (
                Status::Mismatch,
                "failed to compute Authenticode digest".to_string(),
            )
        }
    };
    let sections = differing_sections(expected, actual, cmdline_checked);
    let (mut status, mut detail) = describe_pe_difference(&sections);
    if status == Status::ExpectedDifference {
        let allowed: BTreeSet<_> = sections
            .iter()
            .flatten()
            .map(|s| (s.name.clone(), s.occurrence))
            .collect();
        let same_beyond_sections =
            normalize_pe(expected, &allowed).and_then(|e| Ok(e == normalize_pe(actual, &allowed)?));
        match same_beyond_sections {
            Ok(true) => {}
            Ok(false) => {
                status = Status::Mismatch;
                detail = format!("{detail}, but its headers or layout differ too");
            }
            Err(e) => {
                status = Status::Mismatch;
                detail = format!("{detail}, but its headers could not be compared: {e}");
            }
        }
    }
    (
        status,
        format!("Authenticode digests differ ({e} vs {a}); {detail}"),
    )
}

/// A PE section whose contents differ, with why that is expected, if it is.
#[derive(Debug)]
struct SectionDifference {
    name: String,
    /// Which section of that name this is, counting from 1.
    occurrence: usize,
    expected: Option<&'static str>,
}

impl SectionDifference {
    fn new(
        (name, occurrence): &(String, usize),
        expected: Option<&[u8]>,
        actual: Option<&[u8]>,
        cmdline_checked: bool,
    ) -> Self {
        let expected = match (expected, actual) {
            (Some(_), Some(_)) if KEY_SECTIONS.contains(&name.as_str()) => {
                Some("sections holding embedded keys")
            }
            // BOOT-CONFIG checks the rest of the command line.
            (Some(e), Some(a))
                if cmdline_checked
                    && name == CMDLINE_SECTION
                    && verity::normalize(e) == verity::normalize(a) =>
            {
                Some("the verity root hash, salt and data size of the kernel command line")
            }
            _ => None,
        };
        Self {
            name: name.clone(),
            occurrence: *occurrence,
            expected,
        }
    }
}

/// Classify the section differences of PE files with different Authenticode digests.
fn describe_pe_difference(sections: &Result<Vec<SectionDifference>>) -> (Status, String) {
    let names = |sections: &[&SectionDifference]| {
        sections
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match sections {
        Ok(sections) if sections.is_empty() => (
            Status::Mismatch,
            "no section differs, so the headers do".to_string(),
        ),
        Ok(sections) if sections.iter().all(|s| s.expected.is_some()) => {
            let mut reasons: Vec<(&str, Vec<&SectionDifference>)> = Vec::new();
            for s in sections {
                let reason = s.expected.expect("checked above");
                match reasons.iter_mut().find(|(r, _)| *r == reason) {
                    Some((_, list)) => list.push(s),
                    None => reasons.push((reason, vec![s])),
                }
            }
            let reasons: Vec<String> = reasons
                .iter()
                .map(|(reason, sections)| format!("{reason} ({})", names(sections)))
                .collect();
            (
                Status::ExpectedDifference,
                format!("differs only in {}", reasons.join(" and ")),
            )
        }
        Ok(sections) => (
            Status::Mismatch,
            format!(
                "differs in sections {}; only signatures and the sections holding embedded keys \
                 may differ",
                names(&sections.iter().collect::<Vec<_>>())
            ),
        ),
        Err(e) => (
            Status::Mismatch,
            format!("sections could not be compared: {e}"),
        ),
    }
}

/// Key PE sections by name and occurrence, so same-named sections stay apart.
fn by_occurrence(sections: Vec<(String, &[u8])>) -> Vec<((String, usize), &[u8])> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    sections
        .into_iter()
        .map(|(name, data)| {
            let n = seen.entry(name.clone()).or_default();
            *n += 1;
            ((name, *n), data)
        })
        .collect()
}

/// The PE sections that differ, including those in only one binary.
fn differing_sections(
    expected: &[u8],
    actual: &[u8],
    cmdline_checked: bool,
) -> Result<Vec<SectionDifference>> {
    let sections = |data, side: &str| -> Result<BTreeMap<_, _>> {
        let sections = pcrsys::pe::sections(data)
            .with_whatever_context(|_| format!("failed to read {side} PE sections"))?;
        Ok(by_occurrence(sections).into_iter().collect())
    };
    let expected = sections(expected, "rebuilt")?;
    let actual = sections(actual, "published")?;
    Ok(union_keys(&expected, &actual)
        .into_iter()
        .filter(|k| expected.get(*k) != actual.get(*k))
        .map(|k| {
            SectionDifference::new(
                k,
                expected.get(k).copied(),
                actual.get(k).copied(),
                cmdline_checked,
            )
        })
        .collect())
}

/// `data` without its checksum, signatures, or the `allowed` sections' contents
/// and virtual sizes.
fn normalize_pe(data: &[u8], allowed: &BTreeSet<(String, usize)>) -> Result<Vec<u8>> {
    let sections = pcrsys::pe::sections(data).whatever_context("failed to read PE sections")?;
    let cleared: Vec<usize> = by_occurrence(sections)
        .iter()
        .enumerate()
        .filter(|(_, (key, _))| allowed.contains(key))
        .map(|(i, _)| i)
        .collect();
    pcrsys::pe::normalize(data, &cleared).whatever_context("failed to read PE headers")
}

/// Compare a kernel module, ignoring its appended signature.
fn compare_module(path: &[u8], expected: &[u8], actual: &[u8]) -> (Status, String) {
    let unsigned = |data: &[u8]| -> Result<Option<Vec<u8>>> {
        let module = kmod::decompress(path, data)?;
        Ok(kmod::split_signature(&module)?.map(|(body, _)| body.to_vec()))
    };
    match (unsigned(expected), unsigned(actual)) {
        (Ok(Some(e)), Ok(Some(a))) if e == a => (
            Status::ExpectedDifference,
            "differs only in the appended module signature".to_string(),
        ),
        (Ok(Some(e)), Ok(Some(a))) => (
            Status::Mismatch,
            format!(
                "module differs beyond its signature (unsigned: {} vs {})",
                ContentDigest::of_slice(&e),
                ContentDigest::of_slice(&a)
            ),
        ),
        (Ok(None), Ok(None)) => (
            Status::Mismatch,
            format!(
                "unsigned module contents differ ({} vs {})",
                ContentDigest::of_slice(expected),
                ContentDigest::of_slice(actual)
            ),
        ),
        (Ok(e), Ok(a)) => {
            let side = |m: &Option<Vec<u8>>| if m.is_some() { "signed" } else { "unsigned" };
            (
                Status::Mismatch,
                format!(
                    "module is {} in the rebuilt image but {} in the published image",
                    side(&e),
                    side(&a)
                ),
            )
        }
        (Err(e), _) | (_, Err(e)) => (Status::Mismatch, format!("failed to read module: {e}")),
    }
}

/// Describe the published TUF root role so it can be checked by hand.
fn describe_root_role(actual: &[u8]) -> (Status, String) {
    let summary = serde_json::from_slice::<serde_json::Value>(actual)
        .ok()
        .and_then(|root| {
            let signed = root.get("signed")?;
            if signed.get("_type")?.as_str()? != "root" {
                return None;
            }
            let keys: Vec<&str> = signed
                .get("keys")?
                .as_object()?
                .keys()
                .map(String::as_str)
                .collect();
            Some(format!(
                "version {}, expires {}, keys {}",
                signed.get("version")?,
                signed.get("expires")?.as_str()?,
                keys.join(", ")
            ))
        });
    match summary {
        Some(summary) => (
            Status::ExpectedDifference,
            format!(
                "TUF root role; rebuilds generate their own because the release root role keys \
                 are not available. Check the published root role against the one your \
                 repository publishes: sha512 {}, {summary}",
                hex::encode(Sha512::digest(actual))
            ),
        ),
        None => (
            Status::Mismatch,
            format!(
                "published file is not a TUF root role ({})",
                ContentDigest::of_slice(actual)
            ),
        ),
    }
}

/// The packages an SBOM lists, by name and version, with how often each is listed.
type SbomPackages = BTreeMap<(String, String), usize>;

fn is_sbom(path: &[u8]) -> bool {
    SBOMS.iter().any(|s| path.ends_with(s))
}

/// The format and packages of an SPDX or CycloneDX SBOM.
fn sbom_packages(data: &[u8]) -> Option<(&'static str, SbomPackages)> {
    fn field(v: &Value, name: &str) -> String {
        v.get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
    fn components(list: &[Value], out: &mut SbomPackages) -> Option<()> {
        for c in list {
            *out.entry((field(c, "name"), field(c, "version")))
                .or_default() += 1;
            if let Some(nested) = c.get("components") {
                components(nested.as_array()?, out)?;
            }
        }
        Some(())
    }
    let doc = serde_json::from_slice::<Value>(data).ok()?;
    let mut packages = SbomPackages::new();
    if doc.get("spdxVersion").is_some() {
        for p in doc.get("packages")?.as_array()? {
            *packages
                .entry((field(p, "name"), field(p, "versionInfo")))
                .or_default() += 1;
        }
        Some(("SPDX", packages))
    } else if doc.get("bomFormat")?.as_str()? == "CycloneDX" {
        components(doc.get("components")?.as_array()?, &mut packages)?;
        Some(("CycloneDX", packages))
    } else {
        None
    }
}

/// Compare SBOMs, which may differ if they list the same packages.
fn compare_sbom(expected: &[u8], actual: &[u8]) -> (Status, String) {
    let (Some((e_format, e_packages)), Some((a_format, a_packages))) =
        (sbom_packages(expected), sbom_packages(actual))
    else {
        return (
            Status::Mismatch,
            format!(
                "not both SPDX or CycloneDX SBOMs ({} vs {})",
                ContentDigest::of_slice(expected),
                ContentDigest::of_slice(actual)
            ),
        );
    };
    if e_format != a_format {
        return (
            Status::Mismatch,
            format!("{e_format} SBOM rebuilt but {a_format} SBOM published"),
        );
    }
    let count = |packages: &SbomPackages| packages.values().sum::<usize>();
    if e_packages != a_packages {
        // Packages listed more often in `a` than in `b`, up to a few.
        let more = |a: &SbomPackages, b: &SbomPackages| {
            let names: Vec<_> = a
                .iter()
                .filter(|(k, n)| b.get(*k).is_none_or(|m| m < *n))
                .map(|((name, version), _)| format!("{name} {version}"))
                .collect();
            first_n(&names, 5)
        };
        return (
            Status::Mismatch,
            format!(
                "{e_format} SBOMs list different packages ({} rebuilt, {} published); only or \
                 more often in the rebuilt SBOM: {}; in the published SBOM: {}",
                count(&e_packages),
                count(&a_packages),
                more(&e_packages, &a_packages),
                more(&a_packages, &e_packages)
            ),
        );
    }
    let differences = sbom_differences(expected, actual);
    if !differences.is_empty() {
        return (
            Status::Mismatch,
            format!(
                "{e_format} SBOMs list the same {} packages, but differ beyond the time sbomtool \
                 ran, its random document IDs and the order of CPEs: {}",
                count(&a_packages),
                first_n(&differences, MAX_SHOWN_SBOM_DIFFERENCES)
            ),
        );
    }
    (
        Status::ExpectedDifference,
        format!(
            "{e_format} SBOM listing the same {} packages, with the same fields, such as purls, \
             hashes, licenses and CPEs; sbomtool records the time it ran, random document IDs \
             and CPEs in a random order, so those and the references between IDs are not compared",
            count(&a_packages)
        ),
    )
}

/// Differences shown per SBOM.
const MAX_SHOWN_SBOM_DIFFERENCES: usize = 5;

/// Fields of SBOM objects that hold random IDs.
const SBOM_ID_FIELDS: &[&str] = &["bom-ref", "SPDXID"];

/// Top-level SBOM fields that record when sbomtool ran or refer to random IDs:
/// CycloneDX's serial number and dependency graph, SPDX's creation info,
/// document namespace and relationships.
const SBOM_VOLATILE_FIELDS: &[&str] = &[
    "serialNumber",
    "dependencies",
    "creationInfo",
    "documentNamespace",
    "documentDescribes",
    "relationships",
];

/// Fields of SBOM objects that refer to random IDs.
const SBOM_REFERENCE_FIELDS: &[&str] = &["hasFiles"];

/// Arrays whose order sbomtool does not keep: CPEs, which SPDX lists among
/// external references and CycloneDX among properties.
const SBOM_UNORDERED_FIELDS: &[&str] = &["externalRefs", "properties"];

/// Remove the fields of `value` that hold random IDs or refer to them, and sort
/// the arrays sbomtool writes in a random order.
fn normalize_sbom_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for field in SBOM_ID_FIELDS.iter().chain(SBOM_REFERENCE_FIELDS) {
                map.remove(*field);
            }
            map.values_mut().for_each(normalize_sbom_value);
            for field in SBOM_UNORDERED_FIELDS {
                if let Some(Value::Array(items)) = map.get_mut(*field) {
                    items.sort_by_cached_key(Value::to_string);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(normalize_sbom_value),
        _ => {}
    }
}

/// An SBOM's packages, by name and version.
type SbomPackageFields = BTreeMap<(String, String), Vec<Value>>;

/// An SBOM's packages and the rest of the document, without what sbomtool
/// records differently on each run.
fn normalized_sbom(data: &[u8]) -> Option<(SbomPackageFields, Value)> {
    fn flatten(list: Vec<Value>, out: &mut Vec<Value>) {
        for mut c in list {
            if let Some(Value::Array(nested)) =
                c.as_object_mut().and_then(|c| c.remove("components"))
            {
                flatten(nested, out);
            }
            out.push(c);
        }
    }
    let mut doc = serde_json::from_slice::<Value>(data).ok()?;
    let map = doc.as_object_mut()?;
    for field in SBOM_VOLATILE_FIELDS {
        map.remove(*field);
    }
    if let Some(Value::Object(metadata)) = map.get_mut("metadata") {
        metadata.remove("timestamp");
    }
    let (list, version_field) = match (map.remove("packages"), map.remove("components")) {
        (Some(Value::Array(packages)), None) => (packages, "versionInfo"),
        (None, Some(Value::Array(components))) => {
            let mut flat = Vec::new();
            flatten(components, &mut flat);
            (flat, "version")
        }
        _ => return None,
    };
    normalize_sbom_value(&mut doc);
    let mut packages = SbomPackageFields::new();
    for mut p in list {
        normalize_sbom_value(&mut p);
        let field = |name: &str| {
            p.get(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let key = (field("name"), field(version_field));
        packages.entry(key).or_default().push(p);
    }
    for list in packages.values_mut() {
        list.sort_by_cached_key(Value::to_string);
    }
    Some((packages, doc))
}

/// The fields of two JSON objects whose values differ, or `value` if they are not both objects.
fn differing_fields(expected: &Value, actual: &Value) -> Vec<String> {
    match (expected.as_object(), actual.as_object()) {
        (Some(e), Some(a)) => {
            let keys: BTreeSet<&String> = e.keys().chain(a.keys()).collect();
            keys.into_iter()
                .filter(|k| e.get(*k) != a.get(*k))
                .cloned()
                .collect()
        }
        _ => vec!["value".to_string()],
    }
}

/// How two SBOMs that list the same packages differ, package by package and
/// in the rest of the document, beyond what sbomtool records differently on
/// each run.
fn sbom_differences(expected: &[u8], actual: &[u8]) -> Vec<String> {
    let (Some((e_packages, e_doc)), Some((a_packages, a_doc))) =
        (normalized_sbom(expected), normalized_sbom(actual))
    else {
        return vec!["their structure could not be compared".to_string()];
    };
    let mut out = Vec::new();
    if e_doc != a_doc {
        out.push(format!(
            "document fields {}",
            differing_fields(&e_doc, &a_doc).join(", ")
        ));
    }
    for (key @ (name, version), e_list) in &e_packages {
        let a_list = a_packages.get(key).map_or(&[][..], Vec::as_slice);
        if e_list.as_slice() == a_list {
            continue;
        }
        let fields: BTreeSet<String> = e_list
            .iter()
            .zip(a_list)
            .flat_map(|(e, a)| differing_fields(e, a))
            .collect();
        let fields: Vec<String> = fields.into_iter().collect();
        out.push(format!("{name} {version} ({})", fields.join(", ")));
    }
    out
}

fn report(path: &[u8], status: Status, detail: impl Into<String>) -> FileReport {
    FileReport {
        path: display(path),
        status,
        detail: detail.into(),
    }
}

/// Compare one file between the rebuilt and the published image.
fn compare_file(
    path: &[u8],
    expected: Option<&[u8]>,
    actual: Option<&[u8]>,
    rules: Rules,
) -> FileReport {
    let (expected, actual) = match (expected, actual) {
        (Some(e), Some(a)) => (e, a),
        (None, _) => return report(path, Status::Mismatch, "missing from rebuilt image"),
        (_, None) => return report(path, Status::Mismatch, "missing from published image"),
    };

    if expected == actual {
        return report(path, Status::Match, "identical");
    }
    match rules {
        Rules::Boot => {
            let signing_artifact = SIGNING_ARTIFACT_SUFFIXES
                .iter()
                .any(|s| path.to_ascii_lowercase().ends_with(s.as_bytes()));
            if signing_artifact {
                return report(path, Status::ExpectedDifference, "signing artifact");
            }
            if is_pe(expected) && is_pe(actual) {
                let (status, detail) = compare_pe(expected, actual, is_uki_path(path));
                return report(path, status, detail);
            }
            // BOOT-CONFIG checks the rest of grub.cfg.
            if path == GRUB_CFG && verity::normalize(expected) == verity::normalize(actual) {
                return report(
                    path,
                    Status::ExpectedDifference,
                    "differs only in the verity root hash, salt and data size",
                );
            }
        }
        Rules::Root => {
            if kmod::is_module(path) {
                let (status, detail) = compare_module(path, expected, actual);
                return report(path, status, detail);
            }
            if path.ends_with(TUF_ROOT_ROLE) {
                let (status, detail) = describe_root_role(actual);
                return report(path, status, detail);
            }
            if is_sbom(path) {
                let (status, detail) = compare_sbom(expected, actual);
                return report(path, status, detail);
            }
        }
        Rules::Exact => {}
    }
    report(
        path,
        Status::Mismatch,
        format!(
            "contents differ ({} vs {})",
            ContentDigest::of_slice(expected),
            ContentDigest::of_slice(actual)
        ),
    )
}

/// HMAC-SHA512 of `data` with `key`, as `openssl sha512 -hmac` computes it.
fn hmac_sha512(key: &[u8], data: &[u8]) -> [u8; 64] {
    let mut mac = Hmac::<Sha512>::new_from_slice(key).expect("HMAC takes keys of any length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// Check that a FIPS HMAC, whose key is public, is the HMAC of the published
/// file it names.
fn compare_hmac(path: &[u8], expected: &[u8], actual: &[u8], published: &Files) -> FileReport {
    // `<hex digest>  <path from the partition root>`
    let parse = |data: &[u8]| -> Option<(String, String)> {
        let text = std::str::from_utf8(data).ok()?.trim_end_matches('\n');
        let (digest, target) = text.split_once("  ")?;
        Some((digest.to_string(), target.to_string()))
    };
    let (Some((_, rebuilt_target)), Some((digest, target))) = (parse(expected), parse(actual))
    else {
        return report(
            path,
            Status::Mismatch,
            format!(
                "not an HMAC of the form `<digest>  <path>` ({} vs {})",
                ContentDigest::of_slice(expected),
                ContentDigest::of_slice(actual)
            ),
        );
    };
    if target != rebuilt_target {
        return report(
            path,
            Status::Mismatch,
            format!("covers {target} instead of {rebuilt_target}"),
        );
    }
    let target_path = format!("/{target}");
    let targets: Vec<_> = published
        .iter()
        .filter(|(p, _)| p.eq_ignore_ascii_case(target_path.as_bytes()))
        .collect();
    let [(_, data)] = targets.as_slice() else {
        return report(
            path,
            Status::Mismatch,
            format!("the published image has no single {target_path} to check it against"),
        );
    };
    let computed = hex::encode(hmac_sha512(HMAC_KEY, data));
    if digest.eq_ignore_ascii_case(&computed) && expected == actual {
        report(path, Status::Match, "identical")
    } else if digest.eq_ignore_ascii_case(&computed) {
        report(
            path,
            Status::ExpectedDifference,
            format!("HMAC of the published {target_path}, which differs from the rebuilt one"),
        )
    } else {
        report(
            path,
            Status::Mismatch,
            format!(
                "is not the HMAC of the published {target_path} ({digest} vs computed {computed})"
            ),
        )
    }
}

/// Compare all files; identical ones are logged but not returned.
pub fn compare_files(expected: &Files, actual: &Files, rules: Rules) -> Vec<FileReport> {
    union_keys(expected, actual)
        .into_iter()
        .map(|p| {
            let (e, a) = (
                expected.get(p).map(Vec::as_slice),
                actual.get(p).map(Vec::as_slice),
            );
            let report = match (e, a) {
                (Some(e), Some(a))
                    if rules == Rules::Boot && p.to_ascii_lowercase().ends_with(HMAC_SUFFIX) =>
                {
                    compare_hmac(p, e, a, actual)
                }
                _ => compare_file(p, e, a, rules),
            };
            if eventlog::enabled() {
                eventlog::event(
                    "file",
                    json!({
                        "path": report.path,
                        "status": report.status,
                        "detail": report.detail,
                        "rebuilt": e.map(ContentDigest::of_slice),
                        "published": a.map(ContentDigest::of_slice),
                    }),
                );
            }
            report
        })
        .filter(|r| r.status != Status::Match)
        .collect()
}

/// The type of a filesystem entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Directory,
    Symlink,
    CharDevice,
    BlockDevice,
    Fifo,
    Socket,
}

/// The file type bits of an inode's mode.
pub const S_IFMT: u16 = 0xf000;
pub const S_IFCHR: u16 = 0x2000;
pub const S_IFBLK: u16 = 0x6000;

impl Kind {
    /// The kind of entry an inode's mode describes, as in erofs and ext4.
    pub fn from_mode(mode: u16) -> Option<Self> {
        Some(match mode & S_IFMT {
            0x8000 => Kind::File,
            0x4000 => Kind::Directory,
            0xa000 => Kind::Symlink,
            S_IFCHR => Kind::CharDevice,
            S_IFBLK => Kind::BlockDevice,
            0x1000 => Kind::Fifo,
            0xc000 => Kind::Socket,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::File => "file",
            Kind::Directory => "directory",
            Kind::Symlink => "symlink",
            Kind::CharDevice => "character device",
            Kind::BlockDevice => "block device",
            Kind::Fifo => "fifo",
            Kind::Socket => "socket",
        }
    }
}

impl Serialize for Kind {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

/// The attributes of a filesystem entry, as logged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub kind: Kind,
    /// Permission bits, including setuid, setgid and sticky.
    #[serde(serialize_with = "octal_mode")]
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    #[serde(serialize_with = "lossy_text")]
    pub link: Option<Vec<u8>>,
    #[serde(serialize_with = "xattrs_json")]
    pub xattrs: Xattrs,
    /// Size of a regular file's contents, which are compared separately.
    pub size: u64,
    /// Major and minor number of a character or block device.
    #[serde(serialize_with = "device_number")]
    pub device: Option<(u32, u32)>,
}

fn octal_mode<S: serde::Serializer>(mode: &u32, s: S) -> std::result::Result<S::Ok, S::Error> {
    s.serialize_str(&format!("{mode:04o}"))
}

fn lossy_text<S: serde::Serializer>(
    text: &Option<Vec<u8>>,
    s: S,
) -> std::result::Result<S::Ok, S::Error> {
    text.as_deref().map(display).serialize(s)
}

fn device_number<S: serde::Serializer>(
    device: &Option<(u32, u32)>,
    s: S,
) -> std::result::Result<S::Ok, S::Error> {
    device
        .map(|(major, minor)| format!("{major}:{minor}"))
        .serialize(s)
}

/// Xattrs by name, with values as text or hex.
fn xattrs_json<S: serde::Serializer>(
    xattrs: &Xattrs,
    s: S,
) -> std::result::Result<S::Ok, S::Error> {
    s.collect_map(xattrs.iter().map(|(name, value)| {
        let text = value.strip_suffix(b"\0").unwrap_or(value);
        let value = match std::str::from_utf8(text) {
            Ok(text) if text.chars().all(|c| c.is_ascii_graphic() || c == ' ') => json!(text),
            _ => json!({ "hex": hex::encode(value) }),
        };
        (display(name), value)
    }))
}

/// Split a `new_encode_dev` device number into its major and minor number.
pub fn decode_device(dev: u32) -> (u32, u32) {
    (
        (dev & 0xf_ff00) >> 8,
        (dev & 0xff) | ((dev >> 12) & 0xf_ff00),
    )
}

/// Values longer than this are shown by size and digest.
const MAX_SHOWN_XATTR: usize = 128;

/// Show an xattr value as text, hex, or by size and digest if it is long.
fn describe_xattr(value: Option<&Vec<u8>>) -> String {
    let Some(value) = value else {
        return "<none>".to_string();
    };
    let text = value.strip_suffix(b"\0").unwrap_or(value);
    if value.len() > MAX_SHOWN_XATTR {
        ContentDigest::of_slice(value).to_string()
    } else if !text.is_empty() && text.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        format!("\"{}\"", display(text))
    } else {
        format!("0x{}", hex::encode(value))
    }
}

impl Entry {
    /// How `self` (rebuilt) differs from `other` (published), apart from contents.
    fn differences(&self, other: &Self) -> Vec<String> {
        let mut out = Vec::new();
        if self.kind != other.kind {
            out.push(format!(
                "type {} vs {}",
                self.kind.name(),
                other.kind.name()
            ));
        }
        if self.mode != other.mode {
            out.push(format!("mode {:04o} vs {:04o}", self.mode, other.mode));
        }
        if self.uid != other.uid || self.gid != other.gid {
            out.push(format!(
                "owner {}:{} vs {}:{}",
                self.uid, self.gid, other.uid, other.gid
            ));
        }
        if self.link != other.link {
            let target = |l: &Option<Vec<u8>>| l.as_deref().map_or("<none>".to_string(), display);
            out.push(format!(
                "symlink target {} vs {}",
                target(&self.link),
                target(&other.link)
            ));
        }
        if self.device != other.device {
            let device = |d: Option<(u32, u32)>| {
                d.map_or("<none>".to_string(), |(major, minor)| {
                    format!("{major}:{minor}")
                })
            };
            out.push(format!(
                "device {} vs {}",
                device(self.device),
                device(other.device)
            ));
        }
        for name in union_keys(&self.xattrs, &other.xattrs) {
            let (e, a) = (self.xattrs.get(name), other.xattrs.get(name));
            if e != a {
                out.push(format!(
                    "xattr {} {} vs {}",
                    display(name),
                    describe_xattr(e),
                    describe_xattr(a)
                ));
            }
        }
        out
    }
}

/// A filesystem whose entries can be listed and whose files can be read.
pub trait FileTree {
    /// What `open` needs to find a file again.
    type Handle;

    /// Every entry, including the root directory, keyed by absolute path.
    fn entries(&self) -> Result<BTreeMap<FsPath, (Self::Handle, Entry)>>;

    /// Open a regular file to read its contents.
    fn open(&self, handle: &Self::Handle) -> Result<Box<dyn Read + '_>>;
}

/// The size and sha256 of a file's contents.
#[derive(PartialEq, Eq, Serialize)]
pub struct ContentDigest {
    size: u64,
    #[serde(serialize_with = "hex_bytes")]
    sha256: [u8; 32],
}

/// Serialize bytes as a hex string.
pub fn hex_bytes<S: serde::Serializer>(
    bytes: impl AsRef<[u8]>,
    s: S,
) -> std::result::Result<S::Ok, S::Error> {
    s.serialize_str(&hex::encode(bytes))
}

impl ContentDigest {
    /// Hash up to `size` bytes from `reader`, without holding them in memory.
    fn of(reader: impl Read, size: u64) -> Result<Self> {
        let mut reader = reader.take(size);
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        let mut size = 0u64;
        loop {
            let n = reader
                .read(&mut buf)
                .whatever_context("failed to read file")?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            size += n as u64;
        }
        Ok(Self {
            size,
            sha256: hasher.finalize().into(),
        })
    }

    pub fn of_slice(data: &[u8]) -> Self {
        Self {
            size: data.len() as u64,
            sha256: Sha256::digest(data).into(),
        }
    }
}

impl fmt::Display for ContentDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} bytes, sha256 {}",
            self.size,
            hex::encode(self.sha256)
        )
    }
}

fn read_contents(reader: impl Read, size: u64) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    reader
        .take(size)
        .read_to_end(&mut out)
        .whatever_context("failed to read file")?;
    Ok(out)
}

fn log_contents(path: &[u8], method: &str, expected: &ContentDigest, actual: &ContentDigest) {
    eventlog::event(
        "contents",
        json!({
            "path": display(path),
            "method": method,
            "rebuilt": expected,
            "published": actual,
        }),
    );
}

/// A regular file in a tree: the tree, the file's handle and its size.
type TreeFile<'a, T> = (&'a T, &'a <T as FileTree>::Handle, u64);

/// Compare a regular file in both trees. Published files larger than both
/// `size_limit` and the rebuilt file are not read.
fn compare_contents<E: FileTree, A: FileTree>(
    path: &[u8],
    (e_tree, e_handle, e_size): TreeFile<'_, E>,
    (a_tree, a_handle, a_size): TreeFile<'_, A>,
    rules: Rules,
    size_limit: u64,
) -> Result<FileReport> {
    if a_size > size_limit.max(e_size) {
        return Ok(report(
            path,
            Status::Mismatch,
            format!(
                "published file is {a_size} bytes, more than the rebuilt file ({e_size} bytes) \
                 and its partition ({size_limit} bytes); not read"
            ),
        ));
    }
    let content_aware = rules == Rules::Root
        && (kmod::is_module(path) || path.ends_with(TUF_ROOT_ROLE) || is_sbom(path));
    if content_aware {
        if e_size.max(a_size) > MAX_BUFFERED_SIZE {
            return Ok(report(
                path,
                Status::Mismatch,
                format!(
                    "too large to compare ({e_size} vs {a_size} bytes, limit {MAX_BUFFERED_SIZE})"
                ),
            ));
        }
        let expected = read_contents(e_tree.open(e_handle)?, e_size)?;
        let actual = read_contents(a_tree.open(a_handle)?, a_size)?;
        if eventlog::enabled() {
            log_contents(
                path,
                "content-aware",
                &ContentDigest::of_slice(&expected),
                &ContentDigest::of_slice(&actual),
            );
        }
        return Ok(compare_file(path, Some(&expected), Some(&actual), rules));
    }
    let expected = ContentDigest::of(e_tree.open(e_handle)?, e_size)?;
    let actual = ContentDigest::of(a_tree.open(a_handle)?, a_size)?;
    log_contents(path, "sha256", &expected, &actual);
    Ok(if expected == actual {
        report(path, Status::Match, "identical")
    } else {
        report(
            path,
            Status::Mismatch,
            format!("contents differ ({expected} vs {actual})"),
        )
    })
}

/// The xattr holding an entry's SELinux label.
pub const SELINUX_XATTR: &[u8] = b"security.selinux";

/// Log the comparison of an entry.
fn log_entry(file: &FileReport, expected: Option<&Entry>, actual: Option<&Entry>) {
    if eventlog::enabled() {
        eventlog::event(
            "entry",
            json!({
                "path": file.path,
                "status": file.status,
                "detail": file.detail,
                "rebuilt": expected,
                "published": actual,
            }),
        );
    }
}

/// Compare the metadata of two filesystems' entries, but not their contents.
/// Entries that differ only in the `expected_xattr` are expected differences;
/// identical entries are logged but not returned.
pub fn compare_entries(
    expected: &BTreeMap<FsPath, Entry>,
    actual: &BTreeMap<FsPath, Entry>,
    expected_xattr: &[u8],
) -> Vec<FileReport> {
    let without_xattr = |entry: &Entry| {
        let mut entry = entry.clone();
        entry.xattrs.remove(expected_xattr);
        entry
    };
    let mut reports = Vec::new();
    for path in union_keys(expected, actual) {
        let (e, a) = (expected.get(path), actual.get(path));
        let file = match (e, a) {
            (Some(e), Some(a)) => {
                let differences = e.differences(a);
                if differences.is_empty() {
                    report(path, Status::Match, "identical")
                } else if without_xattr(e).differences(&without_xattr(a)).is_empty() {
                    report(path, Status::ExpectedDifference, differences.join("; "))
                } else {
                    report(path, Status::Mismatch, differences.join("; "))
                }
            }
            (e, _) => {
                let side = if e.is_none() { "rebuilt" } else { "published" };
                report(path, Status::Mismatch, format!("missing from {side} image"))
            }
        };
        log_entry(&file, e, a);
        if file.status != Status::Match {
            reports.push(file);
        }
    }
    reports
}

/// Compare two filesystems entry by entry, apart from timestamps. Identical
/// entries are omitted.
pub fn compare_trees<E: FileTree, A: FileTree>(
    expected: &E,
    actual: &A,
    rules: Rules,
    size_limit: u64,
) -> Result<Vec<FileReport>> {
    let expected_entries = expected.entries()?;
    let actual_entries = actual.entries()?;
    let mut reports = Vec::new();
    for path in union_keys(&expected_entries, &actual_entries) {
        let (e, a) = (expected_entries.get(path), actual_entries.get(path));
        let file = match (e, a) {
            (Some((e_handle, e)), Some((a_handle, a))) => {
                let differences = e.differences(a);
                if !differences.is_empty() {
                    report(path, Status::Mismatch, differences.join("; "))
                } else if e.kind != Kind::File {
                    report(path, Status::Match, "identical")
                } else {
                    compare_contents(
                        path,
                        (expected, e_handle, e.size),
                        (actual, a_handle, a.size),
                        rules,
                        size_limit,
                    )?
                }
            }
            (e, _) => {
                let side = if e.is_none() { "rebuilt" } else { "published" };
                report(path, Status::Mismatch, format!("missing from {side} image"))
            }
        };
        log_entry(&file, e.map(|(_, e)| e), a.map(|(_, a)| a));
        if file.status != Status::Match {
            reports.push(file);
        }
    }
    Ok(reports)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    /// Build a small FAT image containing `files` (paths relative to the root).
    pub fn fat_image(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut img = Cursor::new(vec![0u8; 1 << 20]);
        fatfs::format_volume(&mut img, fatfs::FormatVolumeOptions::new()).unwrap();
        {
            let fs = FileSystem::new(&mut img, FsOptions::new()).unwrap();
            for (path, contents) in files {
                let root = fs.root_dir();
                let (dirs, name) = path.rsplit_once('/').unwrap_or(("", path));
                let mut dir = root;
                for d in dirs.split('/').filter(|d| !d.is_empty()) {
                    dir = dir.create_dir(d).unwrap();
                }
                dir.create_file(name).unwrap().write_all(contents).unwrap();
            }
        }
        img.into_inner()
    }

    fn files(entries: &[(&str, Vec<u8>)]) -> Files {
        entries
            .iter()
            .map(|(p, c)| (p.as_bytes().to_vec(), c.clone()))
            .collect()
    }

    /// Assert that `reports` are of exactly the files in `expected`, with their statuses.
    fn assert_statuses(reports: Vec<FileReport>, expected: &[(&str, Status)]) {
        let found: BTreeMap<_, _> = reports.into_iter().map(|r| (r.path, r.status)).collect();
        let expected = expected.iter().map(|(p, s)| (p.to_string(), *s)).collect();
        assert_eq!(found, expected);
    }

    /// The status of `path` holding `rebuilt` in one image and `published` in the other.
    fn status(path: &str, rebuilt: &[u8], published: &[u8], rules: Rules) -> Status {
        let reports = compare_files(
            &files(&[(path, rebuilt.to_vec())]),
            &files(&[(path, published.to_vec())]),
            rules,
        );
        reports.first().map_or(Status::Match, |r| r.status)
    }

    #[test]
    fn boot_rules() {
        use Status::{ExpectedDifference, Mismatch};
        let root = "a".repeat(64);
        let other_root = "b".repeat(64);
        let grub = |r: &str, s: &str| {
            format!(
                "set dm_verity_root=\"root,,,ro,0 8 verity 1 x y 4096 4096 1 1 sha256 {r} {s} 2\""
            )
            .into_bytes()
        };
        let table = |r: &str| format!("verity 1 a b 4096 4096 1 1 sha256 {r} 00").into_bytes();
        let rebuilt = files(&[
            ("/same", b"same".to_vec()),
            ("/grub/grub.cfg", grub(&root, "00")),
            ("/grub/grub.cfg.sig", b"sig1".to_vec()),
            ("/grub/other.cfg", table(&root)),
            ("/other", b"one".to_vec()),
            ("/only-rebuilt", b"x".to_vec()),
            ("/EFI/BOOT/db.crt", b"cert1".to_vec()),
            ("/EFI/BOOT/rebuilt.crt", b"cert1".to_vec()),
        ]);
        let published = files(&[
            ("/same", b"same".to_vec()),
            ("/grub/grub.cfg", grub(&other_root, "11")),
            ("/grub/grub.cfg.sig", b"sig2".to_vec()),
            ("/grub/other.cfg", table(&other_root)),
            ("/other", b"two".to_vec()),
            ("/EFI/BOOT/db.crt", b"cert2".to_vec()),
            ("/EFI/BOOT/extra.auth", b"x".to_vec()),
        ]);
        assert_statuses(
            compare_files(&rebuilt, &published, Rules::Boot),
            &[
                ("/grub/grub.cfg", ExpectedDifference),
                ("/grub/grub.cfg.sig", ExpectedDifference),
                // Only grub.cfg may differ in its verity digests.
                ("/grub/other.cfg", Mismatch),
                ("/other", Mismatch),
                ("/only-rebuilt", Mismatch),
                // Signing artifacts must be in both images.
                ("/EFI/BOOT/db.crt", ExpectedDifference),
                ("/EFI/BOOT/rebuilt.crt", Mismatch),
                ("/EFI/BOOT/extra.auth", Mismatch),
            ],
        );
    }

    #[test]
    fn root_rules() {
        let sign = crate::kmod::tests::sign;
        let root = |key: &str| {
            format!(
                r#"{{"signed":{{"_type":"root","version":1,"expires":"2027-10-12T17:12:35Z","keys":{{"{key}":{{}}}},"roles":{{}}}},"signatures":[]}}"#
            )
            .into_bytes()
        };
        let role = "/aarch64-bottlerocket-linux-gnu/sys-root/usr/share/updog/root.json";
        for (path, rebuilt, published, expected) in [
            (
                "/m/xfs.ko",
                sign(b"module", b"key1"),
                sign(b"module", b"key2"),
                Status::ExpectedDifference,
            ),
            (
                "/m/xfs.ko",
                sign(b"module", b"key1"),
                sign(b"modulf", b"key2"),
                Status::Mismatch,
            ),
            (
                "/m/xfs.ko",
                b"module".to_vec(),
                sign(b"module", b"key2"),
                Status::Mismatch,
            ),
            // Certificates are only signing artifacts in the boot partitions.
            (
                "/etc/ca.crt",
                b"ca1".to_vec(),
                b"ca2".to_vec(),
                Status::Mismatch,
            ),
            (role, root("aaaa"), root("bbbb"), Status::ExpectedDifference),
            // Anything that is not a root role still fails.
            (
                role,
                root("aaaa"),
                br#"{"signed":{"_type":"targets"}}"#.to_vec(),
                Status::Mismatch,
            ),
        ] {
            let found = status(path, &rebuilt, &published, Rules::Root);
            assert_eq!(found, expected, "{path}: {published:?}");
        }
    }

    const SBOM: &str =
        "/x86_64-bottlerocket-linux-gnu/sys-root/usr/share/bottlerocket/spdx-sbom.json";

    /// An SPDX or CycloneDX SBOM as syft writes it, with random IDs and
    /// timestamps from `seed`, listing `packages` by name and version.
    fn sbom(spdx: bool, seed: &str, packages: &[(&str, &str)]) -> Value {
        let created = format!("2026-10-2{}T00:00:00Z", seed.len());
        let packages = packages.iter().map(|(name, version)| {
            if spdx {
                json!({
                    "SPDXID": format!("SPDXRef-Package-{seed}"),
                    "name": name,
                    "versionInfo": version,
                    "checksums": [{ "algorithm": "SHA256", "checksumValue": "aa" }],
                })
            } else {
                json!({
                    "bom-ref": format!("pkg:{seed}"),
                    "type": "library",
                    "name": name,
                    "version": version,
                    "purl": format!("pkg:rpm/bottlerocket/{name}@{version}"),
                    "licenses": [{ "license": { "id": "LGPL-2.1" } }],
                    "properties": [
                        { "name": "syft:cpe23", "value": format!("cpe:2.3:a:gnu:{name}") },
                        { "name": "syft:cpe23", "value": format!("cpe:2.3:a:{name}:{name}") },
                    ],
                })
            }
        });
        let packages: Vec<_> = packages.collect();
        if spdx {
            json!({
                "spdxVersion": "SPDX-2.3",
                "SPDXID": "SPDXRef-DOCUMENT",
                "creationInfo": { "created": created },
                "documentNamespace": format!("https://anchore.com/{seed}"),
                "packages": packages,
                "relationships": [{ "spdxElementId": format!("SPDXRef-Package-{seed}") }],
            })
        } else {
            json!({
                "bomFormat": "CycloneDX",
                "specVersion": "1.6",
                "serialNumber": format!("urn:uuid:{seed}"),
                "metadata": {
                    "timestamp": created,
                    "component": { "bom-ref": format!("root-{seed}"), "name": "aws-k8s" },
                },
                "components": packages,
                "dependencies": [{ "ref": format!("root-{seed}"), "dependsOn": [format!("pkg:{seed}")] }],
            })
        }
    }

    #[test]
    fn sbom_rules() {
        use Status::{ExpectedDifference, Mismatch};
        let packages = [("glibc", "2.41"), ("ssh", "9")];
        let with = |mut sbom: Value, change: fn(&mut Value)| {
            change(&mut sbom);
            sbom
        };
        fn package(sbom: &mut Value) -> &mut Value {
            let key = if sbom["packages"].is_null() {
                "components"
            } else {
                "packages"
            };
            &mut sbom[key][0]
        }
        let (spdx, cyclonedx) = (sbom(true, "1", &packages), sbom(false, "1", &packages));
        let other_spdx = sbom(true, "22", &packages);
        let other_cyclonedx = sbom(false, "22", &packages);
        let nested = |name: &'static str| {
            move |sbom: &mut Value| {
                sbom["components"][0]["components"] = json!([{ "name": name, "version": "1" }])
            }
        };
        for (i, (rebuilt, published, rules, expected)) in [
            // Other IDs, another time and CPEs in another order are expected.
            (&spdx, other_spdx.clone(), Rules::Root, ExpectedDifference),
            (
                &cyclonedx,
                with(other_cyclonedx.clone(), |s| {
                    package(s)["properties"].as_array_mut().unwrap().reverse()
                }),
                Rules::Root,
                ExpectedDifference,
            ),
            // Only ROOT-A has SBOMs that may differ.
            (&spdx, other_spdx.clone(), Rules::Exact, Mismatch),
            // Other packages, or a package listed once more.
            (
                &spdx,
                sbom(true, "22", &[("glibc", "2.42"), ("ssh", "9")]),
                Rules::Root,
                Mismatch,
            ),
            (
                &spdx,
                sbom(true, "22", &[packages[0], packages[1], packages[1]]),
                Rules::Root,
                Mismatch,
            ),
            // Anything else about a package, or the rest of the document.
            (
                &cyclonedx,
                with(other_cyclonedx.clone(), |s| {
                    package(s)["purl"] = json!("pkg:rpm/other/glibc@2.41")
                }),
                Rules::Root,
                Mismatch,
            ),
            (
                &cyclonedx,
                with(other_cyclonedx.clone(), |s| {
                    package(s)["properties"].as_array_mut().unwrap().pop();
                }),
                Rules::Root,
                Mismatch,
            ),
            (
                &cyclonedx,
                with(other_cyclonedx.clone(), |s| {
                    s["metadata"]["component"]["name"] = json!("aws-ecs")
                }),
                Rules::Root,
                Mismatch,
            ),
            (
                &spdx,
                with(other_spdx.clone(), |s| {
                    package(s)["checksums"][0]["checksumValue"] = json!("bb")
                }),
                Rules::Root,
                Mismatch,
            ),
            // Another format.
            (&spdx, other_cyclonedx.clone(), Rules::Root, Mismatch),
        ]
        .into_iter()
        .enumerate()
        {
            let (rebuilt, published) = (rebuilt.to_string(), published.to_string());
            let found = status(SBOM, rebuilt.as_bytes(), published.as_bytes(), rules);
            assert_eq!(found, expected, "case {i}");
        }

        // Nested CycloneDX components count as packages.
        let mut rebuilt = cyclonedx.clone();
        nested("a")(&mut rebuilt);
        let rebuilt = rebuilt.to_string();
        for (name, expected) in [("a", ExpectedDifference), ("b", Mismatch)] {
            let mut published = other_cyclonedx.clone();
            nested(name)(&mut published);
            let found = status(
                SBOM,
                rebuilt.as_bytes(),
                published.to_string().as_bytes(),
                Rules::Root,
            );
            assert_eq!(found, expected, "{name}");
        }
        assert_eq!(
            status(SBOM, rebuilt.as_bytes(), b"not json", Rules::Root),
            Mismatch
        );
    }

    #[test]
    fn pe_section_rules() {
        let differences = |n: &[&str]| {
            Ok(n.iter()
                .map(|s| SectionDifference::new(&(s.to_string(), 1), Some(b"1"), Some(b"2"), true))
                .collect())
        };
        for (sections, expected) in [
            (&[".vendor_cert"][..], Status::ExpectedDifference),
            (&[".pubkey"], Status::ExpectedDifference),
            (&[".text", ".vendor_cert"], Status::Mismatch),
            // Only the headers differ.
            (&[], Status::Mismatch),
        ] {
            let (status, detail) = describe_pe_difference(&differences(sections));
            assert_eq!(status, expected, "{detail}");
        }
        let (status, _) =
            describe_pe_difference(&Err(snafu::FromString::without_source("bad".to_string())));
        assert_eq!(status, Status::Mismatch);

        // Key sections must be in both binaries.
        let added = SectionDifference::new(&(".pubkey".to_string(), 1), None, Some(b"2"), true);
        assert_eq!(added.expected, None);
    }

    /// A UKI's kernel command line with the given verity root hash and salt.
    pub fn uki_cmdline(root: &str, salt: &str, extra: &str) -> Vec<u8> {
        uki_cmdline_sized(root, salt, 300, extra)
    }

    /// A UKI's kernel command line for `blocks` blocks of verity data.
    pub fn uki_cmdline_sized(root: &str, salt: &str, blocks: u64, extra: &str) -> Vec<u8> {
        let sectors = blocks * 8;
        format!(
            "console=ttyS0 root=/dev/dm-0 rootwait ro{extra} dm-mod.create=\"root,,,ro,0 {sectors} \
             verity 1 PARTUUID=0b5e4d1a-0000-4000-8000-000000000001/PARTNROFF=2 \
             PARTUUID=0b5e4d1a-0000-4000-8000-000000000001/PARTNROFF=3 4096 4096 {blocks} 1 \
             sha256 {root} {salt} 2 restart_on_corruption ignore_zero_blocks\" -- \
             systemd.log_target=journal-or-kmsg"
        )
        .into_bytes()
    }

    /// A minimal UKI with `.linux` and `.cmdline` sections.
    pub fn uki(kernel: &[u8], cmdline: &[u8]) -> Vec<u8> {
        assert!(kernel.len() <= 0x400 && cmdline.len() <= 0x400);
        let mut pe = vec![0u8; 0xa00];
        let put32 = |pe: &mut Vec<u8>, at: usize, v: u32| {
            pe[at..at + 4].copy_from_slice(&v.to_le_bytes());
        };
        pe[0..2].copy_from_slice(b"MZ");
        put32(&mut pe, 0x3c, 0x40); // PE header offset
        pe[0x40..0x44].copy_from_slice(b"PE\0\0");
        pe[0x44..0x46].copy_from_slice(&0x8664u16.to_le_bytes()); // Machine: x86-64
        pe[0x46..0x48].copy_from_slice(&2u16.to_le_bytes()); // NumberOfSections
        pe[0x54..0x56].copy_from_slice(&0xf0u16.to_le_bytes()); // SizeOfOptionalHeader
        pe[0x56..0x58].copy_from_slice(&0x22u16.to_le_bytes()); // Characteristics
        pe[0x58..0x5a].copy_from_slice(&0x20bu16.to_le_bytes()); // Magic: PE32+
        put32(&mut pe, 0x78, 0x1000); // SectionAlignment
        put32(&mut pe, 0x7c, 0x200); // FileAlignment
        put32(&mut pe, 0x90, 0x4000); // SizeOfImage
        put32(&mut pe, 0x94, 0x200); // SizeOfHeaders
        pe[0x9c..0x9e].copy_from_slice(&10u16.to_le_bytes()); // Subsystem: EFI application
        put32(&mut pe, 0xc4, 16); // NumberOfRvaAndSizes

        // 0x400 bytes in the file and a page in memory per section.
        for (i, (name, data)) in [(b".linux\0\0", kernel), (b".cmdline", cmdline)]
            .into_iter()
            .enumerate()
        {
            let header = 0x148 + i * 0x28;
            let raw_offset = 0x200 + i as u32 * 0x400;
            pe[header..header + 8].copy_from_slice(name);
            put32(&mut pe, header + 8, data.len() as u32); // VirtualSize
            put32(&mut pe, header + 12, 0x1000 * (i as u32 + 1)); // VirtualAddress
            put32(&mut pe, header + 16, 0x400); // SizeOfRawData
            put32(&mut pe, header + 20, raw_offset); // PointerToRawData
            put32(&mut pe, header + 36, 0x40000040); // Characteristics: initialized, readable
            let offset = raw_offset as usize;
            pe[offset..offset + data.len()].copy_from_slice(data);
        }
        pe
    }

    #[test]
    fn allowed_section_differences_do_not_hide_header_changes() {
        let (root, other_root) = ("a".repeat(64), "b".repeat(64));
        let rebuilt = uki(b"kernel", &uki_cmdline(&root, "00", ""));
        let resalted = uki(b"kernel", &uki_cmdline(&other_root, "11", ""));
        // The verity digests may only differ in the UKI whose command line BOOT-CONFIG checks.
        assert_eq!(compare_pe(&rebuilt, &resalted, false).0, Status::Mismatch);

        // Make the command line section executable, as if code had been put in it.
        let mut executable = resalted.clone();
        let characteristics = 0x148 + 0x28 + 36;
        executable[characteristics..characteristics + 4]
            .copy_from_slice(&0x60000020u32.to_le_bytes());
        // Move the entry point.
        let mut entry = resalted.clone();
        entry[0x58 + 16..0x58 + 20].copy_from_slice(&0x2000u32.to_le_bytes());
        for published in [executable, entry] {
            let (status, detail) = compare_pe(&rebuilt, &published, true);
            assert_eq!(status, Status::Mismatch, "{detail}");
        }

        // A signature appended to only one of them is not a header change.
        let mut signed = resalted.clone();
        let certificates = 0x58 + 112 + 4 * 8;
        let len = signed.len() as u32;
        signed[certificates..certificates + 4].copy_from_slice(&len.to_le_bytes());
        signed[certificates + 4..certificates + 8].copy_from_slice(&16u32.to_le_bytes());
        signed.extend([7u8; 16]);
        let (status, detail) = compare_pe(&rebuilt, &signed, true);
        assert_eq!(status, Status::ExpectedDifference, "{detail}");
    }

    /// An HMAC file as rpm2img writes it for `target`, holding `data`.
    pub fn hmac_file(target: &str, data: &[u8]) -> Vec<u8> {
        format!("{}  {target}\n", hex::encode(hmac_sha512(HMAC_KEY, data))).into_bytes()
    }

    #[test]
    fn computes_hmac_sha512() {
        // `printf vmlinuz | openssl sha512 -hmac FIPS-FTW-RHT2009`
        assert_eq!(
            hex::encode(hmac_sha512(HMAC_KEY, b"vmlinuz")),
            "a3266dc72b3c02ef60f8d4a5ddb57e1a9e13c7a56bf648039a9aead136f1eb22\
             cfad56ca69fefd5f9949344829a37d53c6533c380bcfd1b6896c0ce34e70f73d"
        );
    }

    #[test]
    fn hmacs_must_match_the_published_file() {
        let vmlinuz = |v: &[u8]| ("/vmlinuz", v.to_vec());
        let hmac = |target: &str, v: &[u8]| ("/.vmlinuz.hmac", hmac_file(target, v));
        let rebuilt = files(&[vmlinuz(b"MZone"), hmac("vmlinuz", b"MZone")]);
        assert!(compare_files(&rebuilt, &rebuilt, Rules::Boot).is_empty());

        for (published, expected) in [
            // Checked against the published kernel, which is compared on its own.
            (
                files(&[vmlinuz(b"MZtwo"), hmac("vmlinuz", b"MZtwo")]),
                Status::ExpectedDifference,
            ),
            (
                files(&[vmlinuz(b"MZtwo"), hmac("vmlinuz", b"MZone")]),
                Status::Mismatch,
            ),
            (
                files(&[vmlinuz(b"MZtwo"), hmac("vmlinux", b"MZtwo")]),
                Status::Mismatch,
            ),
            (
                files(&[("/.vmlinuz.hmac", b"garbage".to_vec())]),
                Status::Mismatch,
            ),
            (files(&[hmac("vmlinuz", b"MZtwo")]), Status::Mismatch),
        ] {
            let reports = compare_files(&rebuilt, &published, Rules::Boot);
            let r = reports.iter().find(|r| r.path == "/.vmlinuz.hmac");
            let r = r.unwrap_or_else(|| panic!("no HMAC report in {reports:#?}"));
            assert_eq!(r.status, expected, "{r:#?}");
        }
    }

    fn entry(mode: u32, uid: u32, size: u64, xattrs: &[(&str, &[u8])]) -> Entry {
        Entry {
            kind: Kind::File,
            mode,
            uid,
            gid: 0,
            link: None,
            xattrs: xattrs
                .iter()
                .map(|(n, v)| (n.as_bytes().to_vec(), v.to_vec()))
                .collect(),
            size,
            device: None,
        }
    }

    #[test]
    fn entries_may_differ_only_in_selinux_label() {
        use Status::{ExpectedDifference, Mismatch};
        let label: &[(&str, &[u8])] = &[("security.selinux", b"system_u:object_r:etc_t:s0")];
        let relabel: &[(&str, &[u8])] = &[("security.selinux", b"system_u:object_r:any_t:s0")];
        let capability: &[u8] = &[1, 0, 0, 2, 0, 0x20, 0, 0];
        let tree = |entries: &[(&str, Entry)]| -> BTreeMap<FsPath, Entry> {
            entries
                .iter()
                .map(|(p, e)| (p.as_bytes().to_vec(), e.clone()))
                .collect()
        };
        let rebuilt = tree(&[
            ("/", entry(0o755, 0, 0, &[])),
            ("/same", entry(0o644, 0, 0, label)),
            ("/relabeled", entry(0o644, 0, 0, label)),
            ("/mode", entry(0o644, 0, 0, label)),
            ("/owner", entry(0o644, 0, 0, &[])),
            ("/other-xattr", entry(0o644, 0, 0, &[])),
            ("/capable", entry(0o644, 0, 0, label)),
            ("/only-rebuilt", entry(0o644, 0, 0, &[])),
        ]);
        let published = tree(&[
            ("/", entry(0o755, 0, 0, label)),
            ("/same", entry(0o644, 0, 0, label)),
            ("/relabeled", entry(0o644, 0, 0, relabel)),
            ("/mode", entry(0o600, 0, 0, relabel)),
            ("/owner", entry(0o644, 5, 0, label)),
            (
                "/other-xattr",
                entry(0o644, 0, 0, &[label[0], ("user.x", b"1")]),
            ),
            (
                "/capable",
                entry(
                    0o644,
                    0,
                    0,
                    &[("security.capability", capability), label[0]],
                ),
            ),
        ]);
        assert_statuses(
            compare_entries(&rebuilt, &published, SELINUX_XATTR),
            &[
                ("/", ExpectedDifference),
                ("/relabeled", ExpectedDifference),
                ("/mode", Mismatch),
                ("/owner", Mismatch),
                ("/other-xattr", Mismatch),
                ("/capable", Mismatch),
                ("/only-rebuilt", Mismatch),
            ],
        );
    }

    /// An in-memory tree whose files can be opened, for testing `compare_trees`.
    struct MemTree(BTreeMap<FsPath, (Vec<u8>, Entry)>);

    impl FileTree for MemTree {
        type Handle = Vec<u8>;

        fn entries(&self) -> Result<BTreeMap<FsPath, (Vec<u8>, Entry)>> {
            Ok(self.0.clone())
        }

        fn open(&self, handle: &Vec<u8>) -> Result<Box<dyn Read + '_>> {
            Ok(Box::new(Cursor::new(handle.clone())))
        }
    }

    fn mem_tree(files: &[(&str, &[u8], u64)]) -> MemTree {
        let files = files.iter().map(|(path, contents, size)| {
            let entry = entry(0o644, 0, *size, &[]);
            (path.as_bytes().to_vec(), (contents.to_vec(), entry))
        });
        MemTree(files.collect())
    }

    #[test]
    fn compare_trees_rules() {
        let packages = [("glibc", "2.41")];
        let (e, a) = (sbom(true, "1", &packages), sbom(true, "22", &packages));
        let (e, a) = (e.to_string(), a.to_string());
        let (e, a) = (e.as_bytes(), a.as_bytes());
        let expected = mem_tree(&[
            ("/a", b"one", 3),
            ("/b", b"two", 3),
            ("/big", b"", 0),
            (SBOM, e, e.len() as u64),
        ]);
        let actual = mem_tree(&[
            ("/a", b"one", 3),
            ("/b", b"tw0", 3),
            // The size alone fails it.
            ("/big", b"", 1 << 40),
            // Read from a tree, as ROOT-A is.
            (SBOM, a, a.len() as u64),
        ]);
        let reports = compare_trees(&expected, &actual, Rules::Root, 1 << 20).unwrap();
        let big = reports.iter().find(|r| r.path == "/big").unwrap();
        assert!(big.detail.contains("not read"), "{big:?}");
        assert_statuses(
            reports,
            &[
                ("/b", Status::Mismatch),
                ("/big", Status::Mismatch),
                (SBOM, Status::ExpectedDifference),
            ],
        );
    }
}
