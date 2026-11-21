//! Comparison of rebuilt kit images against published ones.
//!
//! A kit image is an OCI image with one layer per directory: `Packages/<package>` for each
//! package, and `repodata` for the repository metadata. Identical manifests verify an
//! architecture. Otherwise the config and the entries of each layer are compared, and packages
//! whose bytes differ are compared by header and payload with `rpm::compare`. A differing
//! `repodata` layer must hold exactly the metadata twoliter regenerated from the published
//! packages, with the tar attributes of the rebuilt layer.

use crate::error::Result;
use crate::fsfiles::Status;
use crate::rpm;
use crate::summary::{marker, shorten_hashes, wrap};
use clap::Args;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2_0_11::{Digest, Sha256};
use snafu::{ensure_whatever, ResultExt};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

/// The layer holding the kit's repository metadata.
const REPODATA: &str = "repodata";

#[derive(Args)]
pub struct CompareKitArgs {
    /// Directory holding the OCI layouts `<arch>/rebuilt` and `<arch>/published` of each
    /// architecture.
    #[arg(long)]
    images_dir: PathBuf,

    /// An architecture to compare and the digest of its published manifest, as
    /// `<arch>=<digest>`. Can be given more than once.
    #[arg(long = "arch", required = true, value_parser = parse_pair)]
    arches: Vec<(String, String)>,

    /// The repository metadata regenerated from the published packages of an architecture, as
    /// `<arch>=<dir>`. Can be given more than once.
    #[arg(long = "regenerated", value_parser = parse_pair)]
    regenerated: Vec<(String, String)>,

    /// File to write the JSON report to
    #[arg(long)]
    report: PathBuf,

    /// List expected differences and show full hashes in the summary
    #[arg(long)]
    verbose: bool,
}

fn parse_pair(value: &str) -> std::result::Result<(String, String), String> {
    let (key, value) = value
        .split_once('=')
        .ok_or_else(|| format!("'{value}' is not <arch>=<value>"))?;
    Ok((key.to_string(), value.to_string()))
}

#[derive(Debug, Deserialize)]
struct Index {
    manifests: Vec<Descriptor>,
}

#[derive(Debug, Deserialize)]
struct Descriptor {
    digest: String,
    size: u64,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    config: Descriptor,
    layers: Vec<Descriptor>,
}

/// An OCI image layout on disk.
struct Layout(PathBuf);

impl Layout {
    /// The path of a blob. Only SHA-256 digests are accepted, since they name paths.
    fn blob(&self, digest: &str) -> Result<PathBuf> {
        Ok(self.0.join("blobs/sha256").join(sha256_hex(digest)?))
    }

    /// The path of a blob, after checking that its contents have `digest` and, if given, `size`:
    /// nothing else checks that a layout's blobs hash to the digests that name them.
    fn verified_blob(&self, digest: &str, size: Option<u64>) -> Result<PathBuf> {
        let path = self.blob(digest)?;
        let file = std::fs::File::open(&path)
            .with_whatever_context(|_| format!("failed to read {}", path.display()))?;
        let (sha256, len) = sha256_of(file)
            .with_whatever_context(|_| format!("failed to read {}", path.display()))?;
        ensure_whatever!(
            sha256 == sha256_hex(digest)?,
            "blob {digest} in {} does not match its digest (sha256:{sha256})",
            self.0.display()
        );
        ensure_whatever!(
            size.is_none_or(|size| size == len),
            "blob {digest} in {} is {len} bytes, not the {} of its descriptor",
            self.0.display(),
            size.unwrap_or_default()
        );
        Ok(path)
    }

    /// Read the manifest with `digest`, after checking that it and every blob it names match
    /// their digests and sizes. Returns it parsed, and as JSON for comparing it whole.
    fn manifest(&self, digest: &str, size: Option<u64>) -> Result<(Manifest, Value)> {
        let value: Value = read_json(&self.verified_blob(digest, size)?)?;
        let manifest = Manifest::deserialize(&value).with_whatever_context(|_| {
            format!("failed to parse manifest {digest} in {}", self.0.display())
        })?;
        for descriptor in std::iter::once(&manifest.config).chain(&manifest.layers) {
            self.verified_blob(&descriptor.digest, Some(descriptor.size))?;
        }
        Ok((manifest, value))
    }

    /// The single manifest in the layout's index.
    fn manifest_descriptor(&self) -> Result<Descriptor> {
        let index: Index = read_json(&self.0.join("index.json"))?;
        match <[Descriptor; 1]>::try_from(index.manifests) {
            Ok([manifest]) => Ok(manifest),
            Err(manifests) => {
                snafu::whatever!(
                    "expected one manifest in {}, found {}",
                    self.0.display(),
                    manifests.len()
                );
            }
        }
    }

    /// The image config without the uncompressed layer digests, which are compared with the
    /// layers instead, and whether those digests are the manifest's layer digests, in order.
    /// Kit layers are uncompressed, so each layer's digest is its uncompressed digest.
    fn config(&self, manifest: &Manifest) -> Result<(Value, bool)> {
        let mut config: Value = read_json(&self.blob(&manifest.config.digest)?)?;
        let diff_ids = config
            .get_mut("rootfs")
            .and_then(Value::as_object_mut)
            .and_then(|rootfs| rootfs.remove("diff_ids"));
        let layers: Vec<Value> = manifest
            .layers
            .iter()
            .map(|l| l.digest.clone().into())
            .collect();
        Ok((config, diff_ids == Some(Value::Array(layers))))
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = std::fs::read(path)
        .with_whatever_context(|_| format!("failed to read {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_whatever_context(|_| format!("failed to parse {}", path.display()))
}

#[derive(Debug, Serialize)]
struct Report {
    verdict: &'static str,
    arches: Vec<ArchReport>,
}

#[derive(Debug, Serialize)]
struct ArchReport {
    arch: String,
    /// The digest of the published manifest.
    published: String,
    /// The digest of the rebuilt manifest.
    rebuilt: String,
    /// `match` when the manifests are identical, `expected-difference` when every difference is
    /// expected, and `mismatch` otherwise.
    status: Status,
    differences: Vec<Difference>,
}

#[derive(Debug, Serialize)]
struct Difference {
    /// `config`, `manifest`, or the directory a layer holds, such as `Packages/glibc`.
    item: String,
    /// The most severe status of its files: `mismatch` or `expected-difference`.
    status: Status,
    detail: String,
    files: Vec<FileDifference>,
}

/// A differing file in a layer. For a package, `header` and `files` say how it differs.
#[derive(Debug, Serialize)]
struct FileDifference {
    path: String,
    #[serde(flatten)]
    package: rpm::PackageReport,
}

impl FileDifference {
    fn new(path: &str, status: Status, detail: String) -> Self {
        Self {
            path: path.to_string(),
            package: rpm::PackageReport::new(status, detail),
        }
    }
}

/// The layers of an image keyed by the directory each holds, with each layer's digest and entries.
type Layers = BTreeMap<String, (String, BTreeMap<String, Entry>)>;

/// A tar entry in a layer.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Entry {
    kind: String,
    mode: u32,
    uid: u64,
    gid: u64,
    mtime: u64,
    link: Option<String>,
    /// The hex SHA-256 of the contents of a regular file.
    sha256: Option<String>,
}

/// The hex of a SHA-256 digest. Only SHA-256 digests are accepted, since they name paths.
fn sha256_hex(digest: &str) -> Result<&str> {
    let hex = digest.strip_prefix("sha256:").filter(|hex| {
        hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    });
    match hex {
        Some(hex) => Ok(hex),
        None => {
            snafu::whatever!("'{digest}' is not a SHA-256 digest");
        }
    }
}

/// The hex SHA-256 and length of what `reader` reads.
fn sha256_of(mut reader: impl Read) -> std::io::Result<(String, u64)> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 1 << 20];
    let mut len = 0;
    loop {
        match reader.read(&mut buf)? {
            0 => break,
            n => {
                hasher.update(&buf[..n]);
                len += n as u64;
            }
        }
    }
    Ok((hex::encode(hasher.finalize()), len))
}

/// The name of a tar entry, without the trailing slash of a directory.
fn entry_name<R: Read>(entry: &tar::Entry<R>) -> std::io::Result<String> {
    Ok(entry
        .path()?
        .to_string_lossy()
        .trim_end_matches('/')
        .to_string())
}

/// The contents of the regular file `name` in a layer.
fn read_file(layer: &Path, name: &str) -> Result<Vec<u8>> {
    let context = || format!("failed to read {name} from {}", layer.display());
    let file = std::fs::File::open(layer).with_whatever_context(|_| context())?;
    let mut archive = tar::Archive::new(file);
    for entry in archive.entries().with_whatever_context(|_| context())? {
        let mut entry = entry.with_whatever_context(|_| context())?;
        if entry_name(&entry).with_whatever_context(|_| context())? == name {
            let mut out = Vec::new();
            entry
                .read_to_end(&mut out)
                .with_whatever_context(|_| context())?;
            return Ok(out);
        }
    }
    snafu::whatever!("{name} is not in {}", layer.display());
}

/// Compare the rebuilt kit images against the published ones, write the report and print its
/// summary.
pub fn run(args: CompareKitArgs) -> Result<ExitCode> {
    let regenerated: BTreeMap<&str, &Path> = args
        .regenerated
        .iter()
        .map(|(arch, dir)| (arch.as_str(), Path::new(dir)))
        .collect();
    let arches = args
        .arches
        .iter()
        .map(|(arch, digest)| {
            let dir = args.images_dir.join(arch);
            compare(
                arch,
                &Layout(dir.join("published")),
                digest,
                &Layout(dir.join("rebuilt")),
                regenerated.get(arch.as_str()).copied(),
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let verified = arches.iter().all(|r| r.status != Status::Mismatch);
    let report = Report {
        verdict: if verified { "verified" } else { "failed" },
        arches,
    };
    let json =
        serde_json::to_string_pretty(&report).whatever_context("failed to serialize report")?;
    std::fs::write(&args.report, json)
        .with_whatever_context(|_| format!("failed to write {}", args.report.display()))?;
    print!("{}", summary(&report, args.verbose));
    Ok(if verified {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(crate::EXIT_NOT_VERIFIED)
    })
}

/// Compare the rebuilt image of one architecture against the published one.
///
/// Identical manifest digests verify the image, since the manifest hashes the config and every
/// layer. Otherwise the config and layers are compared to explain the difference, each package
/// whose bytes differ is compared by header and payload, and the repository metadata is compared
/// against `regenerated`.
fn compare(
    arch: &str,
    published: &Layout,
    published_digest: &str,
    rebuilt: &Layout,
    regenerated: Option<&Path>,
) -> Result<ArchReport> {
    let rebuilt_descriptor = rebuilt.manifest_descriptor()?;
    let (published_manifest, published_value) = published.manifest(published_digest, None)?;
    let (rebuilt_manifest, rebuilt_value) =
        rebuilt.manifest(&rebuilt_descriptor.digest, Some(rebuilt_descriptor.size))?;
    let mut report = ArchReport {
        arch: arch.to_string(),
        published: published_digest.to_string(),
        rebuilt: rebuilt_descriptor.digest,
        status: Status::Match,
        differences: Vec::new(),
    };
    if report.rebuilt == report.published {
        return Ok(report);
    }

    let mismatch = |item: &str, detail: String| Difference {
        item: item.to_string(),
        status: Status::Mismatch,
        detail,
        files: Vec::new(),
    };
    let (rebuilt_config, _) = rebuilt.config(&rebuilt_manifest)?;
    let (published_config, diff_ids_match) = published.config(&published_manifest)?;
    if !diff_ids_match {
        report.differences.push(mismatch(
            "config",
            "the published config's rootfs.diff_ids are not its manifest's layer digests"
                .to_string(),
        ));
    }
    if rebuilt_config != published_config {
        report.differences.push(mismatch(
            "config",
            format!(
                "config differs ({} vs {})",
                rebuilt_manifest.config.digest, published_manifest.config.digest
            ),
        ));
    }

    let published_layers = read_layers(published, &published_manifest)?;
    let rebuilt_layers = read_layers(rebuilt, &rebuilt_manifest)?;
    let names: BTreeSet<&String> = published_layers
        .keys()
        .chain(rebuilt_layers.keys())
        .collect();

    for name in names {
        match (rebuilt_layers.get(name), published_layers.get(name)) {
            (Some(rebuilt_layer), Some(published_layer))
                if rebuilt_layer.0 != published_layer.0 =>
            {
                let files = if name == REPODATA {
                    compare_repodata(&rebuilt_layer.1, &published_layer.1, regenerated)?
                } else {
                    let (rebuilt_blob, published_blob) = (
                        rebuilt.blob(&rebuilt_layer.0)?,
                        published.blob(&published_layer.0)?,
                    );
                    let compare_package = |path: &str| {
                        let rebuilt = read_file(&rebuilt_blob, path)?;
                        let published = read_file(&published_blob, path)?;
                        rpm::compare(&rebuilt, &published)
                            .with_whatever_context(|_| format!("failed to compare {path}"))
                    };
                    compare_entries(&rebuilt_layer.1, &published_layer.1, &compare_package)?
                };
                report.differences.push(Difference {
                    item: name.clone(),
                    // A layer can differ without any of its entries differing, such as in order.
                    status: max_status(files.iter().map(|f| f.package.status)),
                    detail: format!("layer {} vs {}", rebuilt_layer.0, published_layer.0),
                    files,
                });
            }
            (Some(_), Some(_)) => {}
            (rebuilt_layer, _) => {
                let side = if rebuilt_layer.is_some() {
                    "rebuilt"
                } else {
                    "published"
                };
                report
                    .differences
                    .push(mismatch(name, format!("only in the {side} kit")));
            }
        }
    }

    // The config and layers were compared above. When the kits have the same layers, anything
    // else in the manifests, such as layer order, annotations, media types or sizes, must match.
    let same_layers = rebuilt_layers.keys().eq(published_layers.keys());
    let differs = normalized(&rebuilt_value, &rebuilt_layers)
        != normalized(&published_value, &published_layers);
    if (same_layers && differs) || report.differences.is_empty() {
        report.differences.push(mismatch(
            "manifest",
            "the manifests differ apart from the digests and sizes of their config and layers, \
             such as in layer order, annotations or media types"
                .to_string(),
        ));
    }
    report.status = max_status(report.differences.iter().map(|d| d.status));
    Ok(report)
}

/// `manifest` with the digest of its config and of each layer replaced by `config` and the layer's
/// name, and their sizes removed: blobs are checked against their sizes, and compared by name.
fn normalized(manifest: &Value, layers: &Layers) -> Value {
    let names: BTreeMap<&str, &str> = layers
        .iter()
        .map(|(name, (digest, _))| (digest.as_str(), name.as_str()))
        .collect();
    let mut out = manifest.clone();
    let set = |descriptor: &mut Value, name: Option<&str>| {
        if let Some(descriptor) = descriptor.as_object_mut() {
            descriptor.remove("size");
            if let Some(name) = name {
                descriptor.insert("digest".to_string(), name.into());
            }
        }
    };
    if let Some(config) = out.get_mut("config") {
        set(config, Some("config"));
    }
    for layer in out
        .get_mut("layers")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        let name = layer.get("digest").and_then(Value::as_str);
        let name = name.and_then(|digest| names.get(digest).copied());
        set(layer, name);
    }
    out
}

/// The most severe of `statuses`, or a mismatch when there are none.
fn max_status(statuses: impl Iterator<Item = Status>) -> Status {
    statuses.max().unwrap_or(Status::Mismatch)
}

/// The SHA-256 of each file in the regenerated repository metadata `dir`, named as in the
/// `repodata` layer.
fn regenerated_files(dir: &Path) -> Result<BTreeMap<String, String>> {
    let context = || format!("failed to read {}", dir.display());
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(dir).with_whatever_context(|_| context())? {
        let path = entry.with_whatever_context(|_| context())?.path();
        let name = path.file_name().expect("listed").to_string_lossy();
        let file = std::fs::File::open(&path).with_whatever_context(|_| context())?;
        let (sha256, _) = sha256_of(file).with_whatever_context(|_| context())?;
        files.insert(format!("{REPODATA}/{name}"), sha256);
    }
    Ok(files)
}

/// Compare a differing `repodata` layer: its files must be the metadata regenerated from the
/// published packages, byte for byte, and its tar attributes those of the rebuilt layer.
fn compare_repodata(
    rebuilt: &BTreeMap<String, Entry>,
    published: &BTreeMap<String, Entry>,
    regenerated: Option<&Path>,
) -> Result<Vec<FileDifference>> {
    let mismatch = |path: &str, detail: String| FileDifference::new(path, Status::Mismatch, detail);
    let Some(dir) = regenerated else {
        let detail = "no repository metadata was regenerated to compare it against".to_string();
        return Ok(vec![mismatch(REPODATA, detail)]);
    };
    let regenerated = regenerated_files(dir)?;
    let published_files: BTreeMap<String, String> = published
        .iter()
        .filter_map(|(path, e)| Some((path.clone(), e.sha256.clone()?)))
        .collect();
    let mut files = Vec::new();
    for path in published_files
        .keys()
        .chain(regenerated.keys())
        .collect::<BTreeSet<_>>()
    {
        let detail = match (published_files.get(path), regenerated.get(path)) {
            (Some(p), Some(r)) if p == r => continue,
            (Some(p), Some(r)) => format!("sha256 {p}, but regenerated {r}"),
            (Some(_), None) => "not in the regenerated metadata".to_string(),
            (None, _) => "only in the regenerated metadata".to_string(),
        };
        files.push(mismatch(path, detail));
    }
    let attributes = |entries: &BTreeMap<String, Entry>| {
        let mut attributes: Vec<Entry> = entries
            .values()
            .map(|e| Entry {
                sha256: None,
                ..e.clone()
            })
            .collect();
        attributes.sort();
        attributes
    };
    if attributes(rebuilt) != attributes(published) {
        let detail = "the tar attributes of its entries differ from the rebuilt layer's";
        files.push(mismatch(REPODATA, detail.to_string()));
    }
    if files.is_empty() {
        files.push(FileDifference::new(
            REPODATA,
            Status::ExpectedDifference,
            "identical to the metadata regenerated from the published packages".to_string(),
        ));
    }
    Ok(files)
}

/// Read each layer of an image, keyed by the directory it holds: `Packages/<package>` or
/// `repodata`. The value is the layer's digest and its entries.
fn read_layers(layout: &Layout, manifest: &Manifest) -> Result<Layers> {
    let mut layers = BTreeMap::new();
    for layer in &manifest.layers {
        let entries = read_entries(&layout.blob(&layer.digest)?)?;
        // Every entry is under the layer's directory, which sorts first.
        let name = entries
            .keys()
            .next()
            .filter(|name| !layers.contains_key(*name))
            .cloned()
            .unwrap_or_else(|| layer.digest.clone());
        layers.insert(name, (layer.digest.clone(), entries));
    }
    Ok(layers)
}

fn read_entries(path: &Path) -> Result<BTreeMap<String, Entry>> {
    let context = || format!("failed to read {}", path.display());
    let file = std::fs::File::open(path).with_whatever_context(|_| context())?;
    let mut archive = tar::Archive::new(file);
    let mut entries = BTreeMap::new();
    for entry in archive.entries().with_whatever_context(|_| context())? {
        let mut entry = entry.with_whatever_context(|_| context())?;
        let name = entry_name(&entry).with_whatever_context(|_| context())?;
        ensure_whatever!(
            Path::new(&name)
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
            "{name} in {} is not a relative path",
            path.display()
        );
        let info = (|| {
            let header = entry.header();
            let kind = header.entry_type();
            let mut info = Entry {
                kind: format!("{kind:?}"),
                mode: header.mode()?,
                uid: header.uid()?,
                gid: header.gid()?,
                mtime: header.mtime()?,
                link: entry
                    .link_name()?
                    .map(|link| link.to_string_lossy().into_owned()),
                sha256: None,
            };
            if kind.is_file() {
                info.sha256 = Some(sha256_of(&mut entry)?.0);
            }
            Ok::<_, std::io::Error>(info)
        })()
        .with_whatever_context(|_| format!("failed to read {name} in {}", path.display()))?;
        // Unpacking a layer keeps the last of several entries for a path, and reading a package
        // from it takes the first.
        ensure_whatever!(
            !entries.contains_key(&name),
            "{name} appears more than once in {}",
            path.display()
        );
        entries.insert(name, info);
    }
    Ok(entries)
}

/// Compare the entries of two layers. Packages whose contents differ are compared by header and
/// payload with `compare_package`, given their path.
fn compare_entries(
    rebuilt: &BTreeMap<String, Entry>,
    published: &BTreeMap<String, Entry>,
    compare_package: &dyn Fn(&str) -> Result<rpm::PackageReport>,
) -> Result<Vec<FileDifference>> {
    let paths: BTreeSet<&String> = rebuilt.keys().chain(published.keys()).collect();
    let mut files = Vec::new();
    for path in paths {
        let file = match (rebuilt.get(path), published.get(path)) {
            (Some(rebuilt), Some(published)) if rebuilt == published => continue,
            (Some(rebuilt), Some(published)) => {
                let attributes = |e: &Entry| Entry {
                    sha256: None,
                    ..e.clone()
                };
                let contents_only = attributes(rebuilt) == attributes(published)
                    && rebuilt.sha256.is_some()
                    && published.sha256.is_some();
                if contents_only && path.ends_with(".rpm") {
                    FileDifference {
                        path: path.clone(),
                        package: compare_package(path)?,
                    }
                } else {
                    let detail = format!("{rebuilt:?} vs {published:?}");
                    FileDifference::new(path, Status::Mismatch, detail)
                }
            }
            (rebuilt, _) => {
                let side = if rebuilt.is_some() {
                    "rebuilt"
                } else {
                    "published"
                };
                FileDifference::new(path, Status::Mismatch, format!("only in the {side} kit"))
            }
        };
        files.push(file);
    }
    Ok(files)
}

/// Render the comparison report for the terminal: each difference, failures first. Expected
/// differences within a layer are listed, and hashes shown in full, only when `verbose` is set.
fn summary(report: &Report, verbose: bool) -> String {
    let mut out = vec![String::new()];
    let mut line = |indent: usize, status: Status, item: &str, detail: &str| {
        if verbose || indent == 0 || status == Status::Mismatch {
            let text = format!("{item}: {detail}");
            let text = if verbose { text } else { shorten_hashes(&text) };
            let (pad, marker) = (" ".repeat(indent), marker(status));
            out.push(format!("{pad}{marker:<5} {}", wrap(&text, indent + 6)));
        }
    };
    for arch in &report.arches {
        let detail = format!("manifest {} vs published {}", arch.rebuilt, arch.published);
        line(0, arch.status, &arch.arch, &detail);
        let mut differences: Vec<&Difference> = arch.differences.iter().collect();
        differences.sort_by_key(|d| Reverse(d.status));
        for difference in differences {
            line(0, difference.status, &difference.item, &difference.detail);
            for file in &difference.files {
                let package = &file.package;
                line(2, package.status, &file.path, &package.detail);
                for header in &package.header {
                    line(4, package.status, "header", header);
                }
                for payload in &package.files {
                    line(4, payload.status, &payload.path, &payload.detail);
                }
            }
        }
    }
    out.push(String::new());
    out.push(wrap(
        "Differences are rebuilt vs published. FAIL fails verification; note is a difference \
         explained by a Go build ID, a per-build module signature or the kernel's per-build \
         module signing certificate, or repository metadata regenerated from the published \
         packages. --verbose lists every difference.",
        0,
    ));
    out.push(format!(
        "Result: {}\n",
        if report.verdict == "verified" {
            "VERIFIED"
        } else {
            "FAILED"
        }
    ));
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goelf::tests::{go_elf, PUBLISHED, REBUILT};
    use crate::kernel::tests::x86_image;
    use crate::kmod::tests::sign;
    use crate::rpm::tests::package;
    use tempfile::TempDir;

    /// A layer to write: the directory it holds, and the names and contents of its files.
    type LayerSpec<'a> = (&'a str, &'a [(&'a str, &'a [u8])]);

    /// Regenerated repository metadata: the names and contents of its files.
    type Files<'a> = &'a [(&'a str, &'a [u8])];

    const EPOCH: u64 = 1_792_536_015;
    const CREATED: &str = "2026-10-20T22:40:15Z";

    fn sha256_digest(bytes: &[u8]) -> String {
        format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
    }

    /// Write a blob, returning its descriptor.
    fn write_blob(dir: &Path, bytes: &[u8]) -> Value {
        let blobs = dir.join("blobs/sha256");
        std::fs::create_dir_all(&blobs).unwrap();
        let digest = sha256_digest(bytes);
        std::fs::write(blobs.join(digest.trim_start_matches("sha256:")), bytes).unwrap();
        serde_json::json!({ "digest": digest, "size": bytes.len() })
    }

    /// The manifest of an OCI layout like rpm2kit's, with one layer per directory in `layers`,
    /// each holding the given files and their contents, in the order given. `edit_config` changes
    /// the config. Writes the layout's blobs.
    fn write_manifest(
        dir: &Path,
        created: &str,
        layers: &[LayerSpec],
        edit_config: impl FnOnce(&mut Value),
    ) -> Value {
        let mut descriptors = Vec::new();
        for (name, files) in layers {
            let mut builder = tar::Builder::new(Vec::new());
            let mut append = |path: String, kind, mode, contents: &[u8]| {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(kind);
                header.set_mode(mode);
                header.set_uid(0);
                header.set_gid(0);
                header.set_mtime(EPOCH);
                header.set_size(contents.len() as u64);
                builder.append_data(&mut header, path, contents).unwrap();
            };
            append(format!("{name}/"), tar::EntryType::Directory, 0o755, b"");
            for (file, contents) in *files {
                let path = format!("{name}/{file}");
                append(path, tar::EntryType::Regular, 0o644, contents);
            }
            descriptors.push(write_blob(dir, &builder.into_inner().unwrap()));
        }
        let diff_ids: Vec<&Value> = descriptors.iter().map(|layer| &layer["digest"]).collect();
        let mut config = serde_json::json!({
            "architecture": "amd64",
            "created": created,
            "rootfs": { "type": "layers", "diff_ids": diff_ids },
        });
        edit_config(&mut config);
        let config = write_blob(dir, config.to_string().as_bytes());
        serde_json::json!({
            "schemaVersion": 2,
            "config": config,
            "layers": descriptors,
        })
    }

    /// Write a layout, returning its manifest digest.
    fn write_layout(dir: &Path, manifest: &Value) -> String {
        let manifest = write_blob(dir, manifest.to_string().as_bytes());
        let index = serde_json::json!({ "manifests": [manifest] });
        std::fs::write(dir.join("index.json"), index.to_string()).unwrap();
        manifest["digest"].as_str().unwrap().to_string()
    }

    fn try_compare_dirs(dir: &TempDir, digest: &str) -> Result<ArchReport> {
        let (rebuilt, published) = (dir.path().join("rebuilt"), dir.path().join("published"));
        let regenerated = dir.path().join("regenerated");
        compare(
            "x86_64",
            &Layout(published),
            digest,
            &Layout(rebuilt),
            regenerated.exists().then_some(regenerated.as_path()),
        )
    }

    /// Compare a rebuilt and a published kit, given the repository metadata regenerated from the
    /// published packages. `edit` changes the rebuilt manifest, and `edit_config` the published
    /// config.
    fn compare_edited(
        rebuilt: &[LayerSpec],
        published: &[LayerSpec],
        published_created: &str,
        regenerated: Option<Files>,
        edit: impl FnOnce(&mut Value),
        edit_config: impl FnOnce(&mut Value),
    ) -> ArchReport {
        let dir = TempDir::new().unwrap();
        let rebuilt_dir = dir.path().join("rebuilt");
        let mut manifest = write_manifest(&rebuilt_dir, CREATED, rebuilt, |_| {});
        edit(&mut manifest);
        write_layout(&rebuilt_dir, &manifest);
        let published_dir = dir.path().join("published");
        let manifest = write_manifest(&published_dir, published_created, published, edit_config);
        let digest = write_layout(&published_dir, &manifest);
        if let Some(files) = regenerated {
            let regenerated = dir.path().join("regenerated");
            std::fs::create_dir(&regenerated).unwrap();
            for (name, contents) in files {
                std::fs::write(regenerated.join(name), contents).unwrap();
            }
        }
        try_compare_dirs(&dir, &digest).unwrap()
    }

    fn compare_layouts(
        rebuilt: &[LayerSpec],
        published: &[LayerSpec],
        regenerated: Option<Files>,
    ) -> ArchReport {
        compare_edited(rebuilt, published, CREATED, regenerated, |_| {}, |_| {})
    }

    fn rpm(files: &[(&str, u32, &[u8])]) -> Vec<u8> {
        package(files)
    }

    /// A kernel package whose kernel image and module are signed with the per-build key `key`.
    fn kernel(key: u8) -> Vec<u8> {
        let image = x86_image(key, key, b"x");
        let module = sign(b"\x7fELF xfs", &[key; 16]);
        rpm(&[
            ("/boot/vmlinuz", 0o100644, &image),
            ("/lib/modules/xfs.ko", 0o100644, &module),
        ])
    }

    /// A package holding a Go binary with the rebuilt or the published build ID.
    fn go_tool(published: bool) -> Vec<u8> {
        let tool = if published {
            go_elf(PUBLISHED, &[2; 20], b"code")
        } else {
            go_elf(REBUILT, &[1; 20], b"code")
        };
        rpm(&[("/usr/bin/tool", 0o100755, &tool)])
    }

    fn summarize(report: ArchReport) -> String {
        let failed = report.status == Status::Mismatch;
        summary(
            &Report {
                verdict: if failed { "failed" } else { "verified" },
                arches: vec![report],
            },
            false,
        )
    }

    fn statuses(report: &ArchReport) -> Vec<(&str, Status)> {
        report
            .differences
            .iter()
            .map(|d| (d.item.as_str(), d.status))
            .collect()
    }

    #[test]
    fn identical_kits_match() {
        let layers: &[LayerSpec] = &[
            ("Packages/glibc", &[("glibc.rpm", b"glibc")]),
            ("repodata", &[("repomd.xml", b"repomd")]),
        ];
        let report = compare_layouts(layers, layers, None);
        assert_eq!(report.status, Status::Match);
        assert!(report.differences.is_empty());
    }

    #[test]
    fn differing_kits_list_config_layers_and_files() {
        let (rebuilt_bash, published_bash) = (
            rpm(&[("/usr/bin/bash", 0o100755, b"rebuilt")]),
            rpm(&[("/usr/bin/bash", 0o100755, b"published")]),
        );
        let report = compare_edited(
            &[
                ("Packages/bash", &[("bash.rpm", &rebuilt_bash)]),
                ("Packages/glibc", &[("glibc.rpm", b"glibc")]),
                ("Packages/tool", &[("tool.rpm", &go_tool(false))]),
                ("repodata", &[("repomd.xml", b"repomd rebuilt")]),
            ],
            &[
                ("Packages/bash", &[("bash.rpm", &published_bash)]),
                ("Packages/glibc", &[("glibc.rpm", b"glibc")]),
                ("Packages/old", &[("old.rpm", b"old")]),
                ("Packages/tool", &[("tool.rpm", &go_tool(true))]),
                ("repodata", &[("repomd.xml", b"repomd")]),
            ],
            "2026-10-21T00:00:00Z",
            None,
            |_| {},
            |_| {},
        );
        assert_eq!(report.status, Status::Mismatch);
        let items: Vec<&str> = report.differences.iter().map(|d| d.item.as_str()).collect();
        assert_eq!(
            items,
            [
                "config",
                "Packages/bash",
                "Packages/old",
                "Packages/tool",
                "repodata"
            ]
        );
        assert!(report.differences[0].detail.starts_with("config differs"));
        let bash = &report.differences[1].files;
        assert_eq!(bash.len(), 1);
        assert_eq!(bash[0].path, "Packages/bash/bash.rpm");
        assert_eq!(bash[0].package.files[0].path, "/usr/bin/bash");
        assert_eq!(report.differences[2].detail, "only in the published kit");
        let repodata = &report.differences[4].files[0];
        assert!(repodata
            .package
            .detail
            .contains("no repository metadata was regenerated"));

        // The summary lists failures first, and expected differences without their files.
        let text = summarize(report);
        let fail = text.find("FAIL  Packages/bash:").unwrap();
        assert!(fail < text.find("note  Packages/tool:").unwrap(), "{text}");
        assert!(text.contains("    FAIL  /usr/bin/bash:"), "{text}");
        assert!(
            !text.contains("glibc") && !text.contains("tool.rpm"),
            "{text}"
        );
        assert!(text.ends_with("Result: FAILED\n"));
    }

    /// A rebuilt and a published kit whose kernel differs only in its signing key.
    fn kernel_kits(regenerated: Files) -> ArchReport {
        let (rebuilt_kernel, published_kernel) = (kernel(1), kernel(2));
        compare_layouts(
            &[
                ("Packages/kernel", &[("kernel.rpm", &rebuilt_kernel)]),
                (
                    "repodata",
                    &[("abc-primary.xml.zst", b"a"), ("repomd.xml", b"x")],
                ),
            ],
            &[
                ("Packages/kernel", &[("kernel.rpm", &published_kernel)]),
                (
                    "repodata",
                    &[("def-primary.xml.zst", b"d"), ("repomd.xml", b"y")],
                ),
            ],
            Some(regenerated),
        )
    }

    const REGENERATED: Files = &[("def-primary.xml.zst", b"d"), ("repomd.xml", b"y")];

    #[test]
    fn expected_kernel_differences_verify() {
        let report = kernel_kits(REGENERATED);
        assert_eq!(report.status, Status::ExpectedDifference, "{report:#?}");
        assert_eq!(
            statuses(&report),
            [
                ("Packages/kernel", Status::ExpectedDifference),
                (REPODATA, Status::ExpectedDifference)
            ]
        );
        let text = summarize(report);
        assert!(text.contains("note  Packages/kernel: layer "), "{text}");
        assert!(!text.contains("kernel.rpm"), "{text}");
        assert!(text.ends_with("Result: VERIFIED\n"));
    }

    #[test]
    fn repodata_must_be_regenerated_from_the_published_packages() {
        let regenerations: [Files; 3] = [
            // The rebuilt metadata, rather than what the published packages give.
            &[("abc-primary.xml.zst", b"a"), ("repomd.xml", b"x")],
            &[("def-primary.xml.zst", b"d"), ("repomd.xml", b"x")],
            &[("def-primary.xml.zst", b"d")],
        ];
        for regenerated in regenerations {
            let report = kernel_kits(regenerated);
            assert_eq!(report.status, Status::Mismatch, "{regenerated:?}");
            assert_eq!(statuses(&report)[1], (REPODATA, Status::Mismatch));
        }
    }

    #[test]
    fn repodata_attributes_must_match_the_rebuilt_layer() {
        let (rebuilt, published) = (go_tool(false), go_tool(true));
        let report = compare_edited(
            &[
                ("Packages/tool", &[("tool.rpm", &rebuilt)]),
                ("repodata", &[("repomd.xml", b"x")]),
            ],
            &[
                ("Packages/tool", &[("tool.rpm", &published)]),
                ("repodata", &[("repomd.xml", b"y")]),
            ],
            CREATED,
            Some(&[("repomd.xml", b"y")]),
            |_| {},
            |_| {},
        );
        // The Go build ID is the only difference: a core kit verifies.
        assert_eq!(report.status, Status::ExpectedDifference, "{report:#?}");

        // The published files are the regenerated ones, but the rebuilt layer has another entry.
        let report = compare_layouts(
            &[
                ("Packages/tool", &[("tool.rpm", &rebuilt)]),
                ("repodata", &[("repomd.xml", b"x"), ("other.xml", b"o")]),
            ],
            &[
                ("Packages/tool", &[("tool.rpm", &published)]),
                ("repodata", &[("repomd.xml", b"y")]),
            ],
            Some(&[("repomd.xml", b"y")]),
        );
        assert_eq!(report.status, Status::Mismatch, "{report:#?}");
        let repodata = &report.differences[1].files;
        assert_eq!(repodata.len(), 1, "{repodata:#?}");
        assert!(repodata[0].package.detail.contains("tar attributes"));
    }

    #[test]
    fn tampered_diff_ids_fail() {
        let (rebuilt, published) = (go_tool(false), go_tool(true));
        let report = compare_edited(
            &[("Packages/tool", &[("tool.rpm", &rebuilt)])],
            &[("Packages/tool", &[("tool.rpm", &published)])],
            CREATED,
            None,
            |_| {},
            |config| config["rootfs"]["diff_ids"] = serde_json::json!([]),
        );
        assert_eq!(report.status, Status::Mismatch);
        assert!(
            report.differences[0].detail.contains("diff_ids"),
            "{report:#?}"
        );
    }

    #[test]
    fn manifest_differences_beside_expected_layers_fail() {
        let (rebuilt, published) = (go_tool(false), go_tool(true));
        let compare_kits = |edit: fn(&mut Value)| {
            compare_edited(
                &[
                    ("Packages/glibc", &[("glibc.rpm", b"glibc")]),
                    ("Packages/tool", &[("tool.rpm", &rebuilt)]),
                    ("repodata", &[("repomd.xml", b"x")]),
                ],
                &[
                    ("Packages/glibc", &[("glibc.rpm", b"glibc")]),
                    ("Packages/tool", &[("tool.rpm", &published)]),
                    ("repodata", &[("repomd.xml", b"y")]),
                ],
                CREATED,
                Some(&[("repomd.xml", b"y")]),
                edit,
                |_| {},
            )
        };
        let report = compare_kits(|_| {});
        assert_eq!(report.status, Status::ExpectedDifference, "{report:#?}");

        let edits: [fn(&mut Value); 3] = [
            |m| m["layers"].as_array_mut().unwrap().swap(0, 1),
            |m| m["annotations"] = serde_json::json!({ "extra": "x" }),
            |m| m["layers"][0]["mediaType"] = "application/x-other".into(),
        ];
        for edit in edits {
            let report = compare_kits(edit);
            assert_eq!(report.status, Status::Mismatch, "{report:#?}");
            let manifest = report.differences.iter().find(|d| d.item == "manifest");
            assert!(manifest.is_some(), "{report:#?}");
        }
    }

    #[test]
    fn tampered_blobs_fail() {
        let layers: &[LayerSpec] = &[("Packages/glibc", &[("glibc.rpm", b"glibc")])];
        for side in ["rebuilt", "published"] {
            let dir = TempDir::new().unwrap();
            let rebuilt = dir.path().join("rebuilt");
            write_layout(&rebuilt, &write_manifest(&rebuilt, CREATED, layers, |_| {}));
            let published = dir.path().join("published");
            let manifest = write_manifest(&published, CREATED, layers, |_| {});
            let digest = write_layout(&published, &manifest);
            let manifest: Manifest =
                read_json(&Layout(dir.path().join(side)).blob(&digest).unwrap()).unwrap();
            // The index still names the right digest, but the layer's bytes are not it.
            let layer = Layout(dir.path().join(side))
                .blob(&manifest.layers[0].digest)
                .unwrap();
            let mut bytes = std::fs::read(&layer).unwrap();
            bytes[0] ^= 1;
            std::fs::write(&layer, bytes).unwrap();
            let err = try_compare_dirs(&dir, &digest).unwrap_err();
            assert!(
                err.to_string().contains("does not match its digest"),
                "{side}: {err}"
            );
        }
    }
    #[test]
    fn rejects_duplicate_entries() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("layer.tar");
        let mut builder = tar::Builder::new(std::fs::File::create(&path).unwrap());
        for contents in [&b"checked"[..], b"installed"] {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(EPOCH);
            header.set_size(contents.len() as u64);
            builder
                .append_data(&mut header, "Packages/foo/foo.rpm", contents)
                .unwrap();
        }
        builder.finish().unwrap();
        let err = read_entries(&path).unwrap_err();
        assert!(err.to_string().contains("appears more than once"), "{err}");
    }

    #[test]
    fn rejects_invalid_digests() {
        let layout = Layout(PathBuf::from("/layout"));
        for digest in [
            "sha256:../../x",
            "sha512:abcd",
            &format!("sha256:{}", "A".repeat(64)),
        ] {
            assert!(layout.blob(digest).is_err(), "{digest}");
        }
        let digest = format!("sha256:{}", "0a".repeat(32));
        assert!(layout.blob(&digest).is_ok());
    }
}
