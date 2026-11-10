//! File-level comparison of partitions whose bytes are expected to differ.

use crate::error::Result;
use crate::pe;
use crate::verity;
use ext4_view::Ext4;
use fatfs::{FileSystem, FsOptions};
use hmac::{Hmac, KeyInit, Mac};
use pcrsys::pe::PeImage;
use serde::Serialize;
use sha2_0_11::{Digest, Sha256, Sha512};
use snafu::{ensure_whatever, ResultExt};
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

/// The TUF root role, which the rebuild takes from `twoliter verify ami --root-role`.
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

/// Which differences are expected in a filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rules {
    /// EFI-A and BOOT-A: signing-dependent content.
    Boot,
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

/// How a region or a file of the published image compares to the rebuilt one.
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
/// the verity root hash and salt in `.cmdline` may differ.
fn compare_pe(expected: &[u8], actual: &[u8], cmdline_checked: bool) -> (Status, String) {
    let (Ok(rebuilt), Ok(published)) = (PeImage::parse(expected), PeImage::parse(actual)) else {
        return (
            Status::Mismatch,
            "failed to compute Authenticode digest".to_string(),
        );
    };
    let (e, a) = (
        rebuilt.authenticode().sha256,
        published.authenticode().sha256,
    );
    if e == a {
        return (
            Status::ExpectedDifference,
            "signatures differ; Authenticode digests match".to_string(),
        );
    }
    let (e, a) = (hex::encode(e), hex::encode(a));
    let sections = differing_sections(&rebuilt, &published, cmdline_checked);
    let (mut status, mut detail) = describe_pe_difference(&sections);
    if status == Status::ExpectedDifference {
        let allowed: BTreeSet<_> = sections
            .iter()
            .map(|s| (s.name.clone(), s.occurrence))
            .collect();
        let same_beyond_sections = normalize_pe(expected, &rebuilt, &allowed)
            .and_then(|e| Ok(e == normalize_pe(actual, &published, &allowed)?));
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
fn describe_pe_difference(sections: &[SectionDifference]) -> (Status, String) {
    let names = |sections: &[&SectionDifference]| {
        sections
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match sections {
        [] => (
            Status::Mismatch,
            "no section differs, so the headers do".to_string(),
        ),
        sections if sections.iter().all(|s| s.expected.is_some()) => {
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
        sections => (
            Status::Mismatch,
            format!(
                "differs in sections {}; only signatures and the sections holding embedded keys \
                 may differ",
                names(&sections.iter().collect::<Vec<_>>())
            ),
        ),
    }
}

/// Key PE sections by name and occurrence, so same-named sections stay apart.
fn by_occurrence(pe: &PeImage) -> Vec<((String, usize), &[u8])> {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    pe.sections()
        .map(|(name, data)| {
            let n = seen.entry(name).or_default();
            *n += 1;
            ((name.to_string(), *n), data)
        })
        .collect()
}

/// The PE sections that differ, including those in only one binary.
fn differing_sections(
    expected: &PeImage,
    actual: &PeImage,
    cmdline_checked: bool,
) -> Vec<SectionDifference> {
    let expected: BTreeMap<_, _> = by_occurrence(expected).into_iter().collect();
    let actual: BTreeMap<_, _> = by_occurrence(actual).into_iter().collect();
    union_keys(&expected, &actual)
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
        .collect()
}

/// `data`, parsed as `pe`, without its checksum, signatures, or the `allowed` sections'
/// contents and virtual sizes.
fn normalize_pe(data: &[u8], pe: &PeImage, allowed: &BTreeSet<(String, usize)>) -> Result<Vec<u8>> {
    // `PeImage::sections` walks the section table in file order, as `normalize` does.
    let cleared: Vec<usize> = by_occurrence(pe)
        .iter()
        .enumerate()
        .filter(|(_, (key, _))| allowed.contains(key))
        .map(|(i, _)| i)
        .collect();
    pe::normalize(data, &cleared).whatever_context("failed to read PE headers")
}

/// Why a ROOT-A file whose contents differ is expected to differ, if it is.
fn expected_root_difference(path: &[u8]) -> Option<&'static str> {
    SBOMS
        .iter()
        .any(|s| path.ends_with(s))
        .then_some("SBOM; skipped, sbomtool output is not reproducible")
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

/// HMAC-SHA512 of `data` with `HMAC_KEY`, as `openssl sha512 -hmac` computes it.
fn hmac_sha512(data: &[u8]) -> [u8; 64] {
    let mut mac = Hmac::<Sha512>::new_from_slice(HMAC_KEY).expect("HMAC takes keys of any length");
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
    let computed = hex::encode(hmac_sha512(data));
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
            match (e, a) {
                (Some(e), Some(a))
                    if rules == Rules::Boot && p.to_ascii_lowercase().ends_with(HMAC_SUFFIX) =>
                {
                    compare_hmac(p, e, a, actual)
                }
                _ => compare_file(p, e, a, rules),
            }
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
const S_IFMT: u16 = 0xf000;

impl Kind {
    /// The kind of entry an inode's mode describes, as erofs records it.
    pub fn from_mode(mode: u16) -> Option<Self> {
        Some(match mode & S_IFMT {
            0x8000 => Kind::File,
            0x4000 => Kind::Directory,
            0xa000 => Kind::Symlink,
            0x2000 => Kind::CharDevice,
            0x6000 => Kind::BlockDevice,
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

/// The attributes of a filesystem entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub kind: Kind,
    /// Permission bits, including setuid, setgid and sticky.
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub link: Option<Vec<u8>>,
    pub xattrs: Xattrs,
    /// Size of a regular file's contents, which are compared separately.
    pub size: u64,
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
#[derive(PartialEq, Eq)]
pub struct ContentDigest {
    size: u64,
    sha256: [u8; 32],
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

/// A regular file in a tree: the tree, the file's handle and its size.
type TreeFile<'a, T> = (&'a T, &'a <T as FileTree>::Handle, u64);

/// Compare a regular file in both trees by the digests of its contents. Published
/// files larger than both `size_limit` and the rebuilt file are not read.
fn compare_contents<E: FileTree, A: FileTree>(
    path: &[u8],
    (e_tree, e_handle, e_size): TreeFile<'_, E>,
    (a_tree, a_handle, a_size): TreeFile<'_, A>,
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
    let expected = ContentDigest::of(e_tree.open(e_handle)?, e_size)?;
    let actual = ContentDigest::of(a_tree.open(a_handle)?, a_size)?;
    Ok(if expected == actual {
        report(path, Status::Match, "identical")
    } else if let Some(reason) = expected_root_difference(path) {
        report(path, Status::ExpectedDifference, reason)
    } else {
        // A wrong root role is the likely cause, and the user's to fix.
        let hint = if path.ends_with(TUF_ROOT_ROLE) {
            "; the rebuild embeds the root role that twoliter verify ami --root-role names (by \
             default the source's highest-numbered <N>.root.json), so pass the one the AMI was \
             built with"
        } else {
            ""
        };
        report(
            path,
            Status::Mismatch,
            format!("contents differ ({expected} vs {actual}){hint}"),
        )
    })
}

/// Compare two root filesystems entry by entry, apart from timestamps. Contents may
/// differ only where `expected_root_difference` allows. Identical entries are omitted.
pub fn compare_trees<E: FileTree, A: FileTree>(
    expected: &E,
    actual: &A,
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
                        size_limit,
                    )?
                }
            }
            (e, _) => {
                let side = if e.is_none() { "rebuilt" } else { "published" };
                report(path, Status::Mismatch, format!("missing from {side} image"))
            }
        };
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

    const SBOM: &str =
        "/x86_64-bottlerocket-linux-gnu/sys-root/usr/share/bottlerocket/spdx-sbom.json";

    #[test]
    fn pe_section_rules() {
        let differences = |n: &[&str]| -> Vec<_> {
            n.iter()
                .map(|s| SectionDifference::new(&(s.to_string(), 1), Some(b"1"), Some(b"2"), true))
                .collect()
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
        format!("{}  {target}\n", hex::encode(hmac_sha512(data))).into_bytes()
    }

    #[test]
    fn computes_hmac_sha512() {
        // `printf vmlinuz | openssl sha512 -hmac FIPS-FTW-RHT2009`
        assert_eq!(
            hex::encode(hmac_sha512(b"vmlinuz")),
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
        }
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
        let role = "/aarch64-bottlerocket-linux-gnu/sys-root/usr/share/updog/root.json";
        let (e, a): (&[u8], &[u8]) = (b"{\"spdx\":1}", b"{\"spdx\":22}");
        let expected = mem_tree(&[
            ("/a", b"one", 3),
            ("/b", b"two", 3),
            ("/big", b"", 0),
            ("/etc/ca.crt", b"ca1", 3),
            ("/m/xfs.ko", b"module~key1", 11),
            (role, b"aaaa", 4),
            (SBOM, e, e.len() as u64),
        ]);
        let actual = mem_tree(&[
            ("/a", b"one", 3),
            ("/b", b"tw0", 3),
            // The size alone fails it.
            ("/big", b"", 1 << 40),
            // Certificates are only signing artifacts in the boot partitions.
            ("/etc/ca.crt", b"ca2", 3),
            // Kernel modules must match like any other file.
            ("/m/xfs.ko", b"module~key2", 11),
            // The root role must match like any other file: the rebuild embeds the public one.
            (role, b"not json", 8),
            (SBOM, a, a.len() as u64),
        ]);
        let reports = compare_trees(&expected, &actual, 1 << 20).unwrap();
        let big = reports.iter().find(|r| r.path == "/big").unwrap();
        assert!(big.detail.contains("not read"), "{big:?}");
        let root_role = reports.iter().find(|r| r.path == role).unwrap();
        assert!(root_role.detail.contains("--root-role"), "{root_role:?}");
        assert_statuses(
            reports,
            &[
                ("/b", Status::Mismatch),
                ("/big", Status::Mismatch),
                ("/etc/ca.crt", Status::Mismatch),
                ("/m/xfs.ko", Status::Mismatch),
                (role, Status::Mismatch),
                (SBOM, Status::ExpectedDifference),
            ],
        );
    }
}
