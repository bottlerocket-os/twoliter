//! File-level comparison of root filesystem contents and metadata.
use crate::error::Result;
use crate::kmod;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256, Sha512};
use snafu::{ensure_whatever, ResultExt};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{Cursor, Read};

/// The TUF root role, which rebuilds cannot reproduce without the release keys.
const TUF_ROOT_ROLE: &[u8] = b"/usr/share/updog/root.json";

/// The SBOMs in ROOT-A, which sbomtool cannot reproduce yet.
const SBOMS: &[&[u8]] = &[
    b"/usr/share/bottlerocket/spdx-sbom.json",
    b"/usr/share/bottlerocket/cyclonedx-sbom.json",
];

/// Largest file read into memory for a content-aware comparison.
const MAX_BUFFERED_SIZE: u64 = 256 << 20;

/// Which differences are expected in a filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rules {
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

/// Compare all files; identical ones are logged but not returned.
pub fn compare_files(expected: &Files, actual: &Files, rules: Rules) -> Vec<FileReport> {
    union_keys(expected, actual)
        .into_iter()
        .map(|p| {
            let (e, a) = (
                expected.get(p).map(Vec::as_slice),
                actual.get(p).map(Vec::as_slice),
            );
            compare_file(p, e, a, rules)
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

impl Kind {
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
    /// Major and minor number of a character or block device.
    pub device: Option<(u32, u32)>,
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

fn read_contents(reader: impl Read, size: u64) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    reader
        .take(size)
        .read_to_end(&mut out)
        .whatever_context("failed to read file")?;
    Ok(out)
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
        return Ok(compare_file(path, Some(&expected), Some(&actual), rules));
    }
    let expected = ContentDigest::of(e_tree.open(e_handle)?, e_size)?;
    let actual = ContentDigest::of(a_tree.open(a_handle)?, a_size)?;
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
        if file.status != Status::Match {
            reports.push(file);
        }
    }
    Ok(reports)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

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
    fn reports_xattr_differences() {
        let label: &[(&str, &[u8])] = &[("security.selinux", b"system_u:object_r:os_t:s0\0")];
        let rebuilt = entry(0o755, 0, 0, label);
        assert!(rebuilt.differences(&rebuilt.clone()).is_empty());
        let capability: &[u8] = &[1, 0, 0, 2, 0, 0x20, 0, 0];
        let capable = [("security.capability", capability), label[0]];
        let capable = entry(0o755, 0, 0, &capable);
        assert_eq!(rebuilt.differences(&capable).len(), 1);
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
