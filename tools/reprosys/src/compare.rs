//! Compare a rebuilt disk image against the image published in an AMI.

use crate::disk::{self, DiskLayout, Partition, RangeHash};
use crate::erofs::{self, ErofsTree};
use crate::error::Result;
use crate::fsfiles::{self, FileReport, Rules, Status};
use serde::Serialize;
use snafu::{ensure_whatever, ResultExt};
use std::fs::File;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Verified,
    Failed,
}

/// The result of comparing one region of the disk.
#[derive(Debug, Clone, Serialize)]
pub struct RegionReport {
    pub name: String,
    pub status: Status,
    pub offset: u64,
    pub len: u64,
    pub expected_sha256: Option<String>,
    pub actual_sha256: Option<String>,
    pub detail: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FileReport>,
}

/// Whether partition `name` is unused, so must be all zero in the published image.
fn must_be_zero(name: &str) -> bool {
    name.starts_with("RESERVED") || name.ends_with("-B")
}

impl RegionReport {
    /// A region that fails verification, with no digests.
    fn mismatch(name: &str, offset: u64, len: u64, detail: String) -> Self {
        Self {
            name: name.to_string(),
            status: Status::Mismatch,
            offset,
            len,
            expected_sha256: None,
            actual_sha256: None,
            detail,
            files: Vec::new(),
        }
    }

    /// Whether this region fails verification, as every mismatch does.
    fn failed(&self) -> bool {
        self.status == Status::Mismatch
    }

    /// Fail a region whose published bytes are not all zero.
    fn require_zero(&mut self, actual: &RangeHash) {
        if !actual.all_zero {
            self.status = Status::Mismatch;
            self.detail = "published image is not all zero".to_string();
        }
    }

    /// A region whose bytes differ but whose files could not be read.
    fn unreadable(&mut self, e: impl std::fmt::Display) {
        self.detail = format!("contents differ and files could not be read: {e}");
    }

    /// Explain a region whose bytes differ by its differing `files`: it is an
    /// expected difference if none of them is a mismatch.
    fn classify_files(
        &mut self,
        files: Vec<FileReport>,
        all_identical: &str,
        expected: impl FnOnce(usize) -> String,
        mismatch: impl FnOnce(usize, usize) -> String,
    ) {
        let mismatches = files
            .iter()
            .filter(|f| f.status == Status::Mismatch)
            .count();
        self.detail = if files.is_empty() {
            all_identical.to_string()
        } else if mismatches == 0 {
            self.status = Status::ExpectedDifference;
            expected(files.len())
        } else {
            mismatch(mismatches, files.len())
        };
        self.files = files;
    }
}

/// The result of comparing one disk image.
#[derive(Debug, Clone, Serialize)]
pub struct ImageReport {
    pub expected: Option<PathBuf>,
    pub actual: Option<PathBuf>,
    pub expected_len: u64,
    pub actual_len: u64,
    pub regions: Vec<RegionReport>,
}

impl ImageReport {
    fn failed(&self) -> bool {
        self.regions.iter().any(RegionReport::failed)
    }
}

/// The full comparison report.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub verdict: Verdict,
    pub os: ImageReport,
    pub data: Option<ImageReport>,
    pub notes: Vec<String>,
}

const NOTES: &[&str] = &[];

struct Images {
    expected: File,
    actual: File,
    expected_path: PathBuf,
    actual_path: PathBuf,
    expected_len: u64,
    actual_len: u64,
}

impl Images {
    fn open(expected: &Path, actual: &Path) -> Result<Self> {
        let open =
            |p: &Path| File::open(p).whatever_context(format!("failed to open {}", p.display()));
        let (expected_file, actual_file) = (open(expected)?, open(actual)?);
        Ok(Self {
            expected_len: disk::file_len(&expected_file)?,
            actual_len: disk::file_len(&actual_file)?,
            expected: expected_file,
            actual: actual_file,
            expected_path: expected.to_path_buf(),
            actual_path: actual.to_path_buf(),
        })
    }

    /// An empty report on these images.
    fn report(&self) -> ImageReport {
        ImageReport {
            expected: Some(self.expected_path.clone()),
            actual: Some(self.actual_path.clone()),
            expected_len: self.expected_len,
            actual_len: self.actual_len,
            regions: Vec::new(),
        }
    }

    /// The published bytes beyond the end of the rebuilt image, which must be zero.
    fn padding(&mut self) -> Result<Option<RegionReport>> {
        if self.actual_len <= self.expected_len {
            return Ok(None);
        }
        let tail = [(self.expected_len, self.actual_len - self.expected_len)];
        let a = disk::hash_ranges(&mut self.actual, &tail)?;
        let mut r = assess_region("PADDING", &tail, None, &a);
        r.require_zero(&a);
        Ok(Some(r))
    }

    fn hash(&mut self, ranges: &[(u64, u64)]) -> Result<(RangeHash, RangeHash)> {
        Ok((
            disk::hash_ranges(&mut self.expected, ranges)?,
            disk::hash_ranges(&mut self.actual, ranges)?,
        ))
    }
}

fn assess_region(
    name: &str,
    ranges: &[(u64, u64)],
    expected: Option<&RangeHash>,
    actual: &RangeHash,
) -> RegionReport {
    let offset = ranges.first().map(|r| r.0).unwrap_or(0);
    let len = ranges.iter().map(|r| r.1).sum();
    let (status, detail) = if expected.is_none_or(|e| e == actual) {
        (Status::Match, "identical")
    } else {
        (Status::Mismatch, "contents differ")
    };
    RegionReport {
        status,
        expected_sha256: expected.map(RangeHash::hex),
        actual_sha256: Some(actual.hex()),
        ..RegionReport::mismatch(name, offset, len, detail.to_string())
    }
}

/// Compare partition-by-partition two images that share a GPT layout.
pub fn compare_os(expected: &Path, actual: &Path) -> Result<ImageReport> {
    let mut images = Images::open(expected, actual)?;
    let mut report = images.report();
    let (expected_len, actual_len) = (images.expected_len, images.actual_len);
    if actual_len < expected_len {
        report.regions.push(RegionReport::mismatch(
            "SIZE",
            0,
            expected_len,
            smaller(actual_len, expected_len),
        ));
        return Ok(report);
    }

    let layout = DiskLayout::read(&mut images.expected)?;
    let gpt_ranges = layout.gpt_ranges();
    let (e, a) = images.hash(&gpt_ranges)?;
    let mut gpt = assess_region("GPT", &gpt_ranges, Some(&e), &a);
    gpt.detail = match gpt.status {
        Status::Match => "protective MBR, primary and backup GPT identical".to_string(),
        _ => "partition tables differ; partition comparisons use the rebuilt layout".to_string(),
    };
    report.regions.push(gpt);

    let gaps = layout.unallocated(expected_len);
    if !gaps.is_empty() {
        let (e, a) = images.hash(&gaps)?;
        report
            .regions
            .push(assess_region("UNALLOCATED", &gaps, Some(&e), &a));
    }

    for p in &layout.partitions {
        let ranges = [(p.offset, p.len)];
        let (e, a) = images.hash(&ranges)?;
        let mut r = assess_region(&p.name, &ranges, Some(&e), &a);
        match p.name.as_str() {
            "HASH-A" if r.status != Status::Match => {
                r.status = Status::ExpectedDifference;
                r.detail =
                    "differs; depends on the random verity salt and is not compared".to_string();
            }
            "ROOT-A" if r.status != Status::Match => compare_root(&images, p, &mut r)?,
            n if must_be_zero(n) => r.require_zero(&a),
            _ => {}
        }
        report.regions.push(r);
    }

    if let Some(mut r) = images.padding()? {
        if r.status == Status::Match {
            r.detail = "bytes beyond the rebuilt image are zero".to_string();
        }
        report.regions.push(r);
    }
    Ok(report)
}

/// Compare a ROOT-A whose bytes differ file by file. Only erofs is supported;
/// any other ROOT-A that differs fails.
fn compare_root(images: &Images, p: &Partition, r: &mut RegionReport) -> Result<()> {
    let fs_name = |erofs: bool| if erofs { "erofs" } else { "not erofs" };
    let expected_erofs = erofs::is_erofs(&images.expected, p.offset)?;
    let actual_erofs = erofs::is_erofs(&images.actual, p.offset)?;
    if !(expected_erofs && actual_erofs) {
        r.detail = format!(
            "the rebuilt ROOT-A is {} and the published ROOT-A is {}; only erofs root \
             filesystems are supported, so the files were not compared",
            fs_name(expected_erofs),
            fs_name(actual_erofs)
        );
        return Ok(());
    }
    // Extract next to the published image, where there is room.
    let work_dir = images
        .actual_path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let files = (|| {
        // Compressed files can expand to any size, so bound them before extracting.
        let expected_size = erofs::total_file_size(&images.expected_path, p.offset)?;
        let actual_size = erofs::total_file_size(&images.actual_path, p.offset)?;
        ensure_whatever!(
            actual_size <= expected_size.saturating_add(p.len),
            "the published ROOT-A holds {actual_size} bytes of files, more than the rebuilt \
             ROOT-A ({expected_size} bytes) plus the size of the partition; it was not \
             extracted"
        );
        let e = ErofsTree::extract(&images.expected_path, p.offset, p.len, work_dir)?;
        let a = ErofsTree::extract(&images.actual_path, p.offset, p.len, work_dir)?;
        fsfiles::compare_trees(&e, &a, p.len)
    })();
    match files {
        Ok(files) => r.classify_files(
            files,
            "contents differ, but all entries are identical; the difference is in what is not \
             compared (timestamps or filesystem layout)",
            |n| format!("files differ only in the SBOMs; {n} differing files listed"),
            |mismatches, n| format!("{mismatches} of {n} differing files differ beyond the SBOMs"),
        ),
        Err(e) => r.unreadable(e),
    }
    Ok(())
}

/// Compare the data image over the rebuilt image's length, and check that the rest is zero.
pub fn compare_data(expected: &Path, actual: &Path) -> Result<ImageReport> {
    let mut images = Images::open(expected, actual)?;
    let mut report = images.report();
    let (expected_len, actual_len) = (images.expected_len, images.actual_len);
    let common = [(0, expected_len.min(actual_len))];
    let (e, a) = images.hash(&common)?;
    let mut r = assess_region("DATA", &common, Some(&e), &a);
    if actual_len < expected_len {
        r.status = Status::Mismatch;
        r.detail = smaller(actual_len, expected_len);
    }
    report.regions.push(r);
    report.regions.extend(images.padding()?);
    Ok(report)
}

fn smaller(actual_len: u64, expected_len: u64) -> String {
    format!(
        "published image ({actual_len} bytes) is smaller than the rebuilt image ({expected_len} \
         bytes)"
    )
}

/// Report a data image that exists on only one side, which fails verification.
fn missing_data(expected: Option<&Path>, actual: Option<&Path>) -> Result<ImageReport> {
    let Some(path) = expected.or(actual) else {
        snafu::whatever!("no data image given");
    };
    let detail = if expected.is_some() {
        "the rebuild produced a data image, but the AMI has no data volume"
    } else {
        "the AMI has a data volume, but the rebuild produced no data image"
    };
    let len = disk::file_len(
        &File::open(path).whatever_context(format!("failed to open {}", path.display()))?,
    )?;
    Ok(ImageReport {
        expected: expected.map(Path::to_path_buf),
        actual: actual.map(Path::to_path_buf),
        expected_len: if expected.is_some() { len } else { 0 },
        actual_len: if actual.is_some() { len } else { 0 },
        regions: vec![RegionReport::mismatch("DATA", 0, len, detail.to_string())],
    })
}

/// Compare the OS image and the data images.
pub fn compare(
    expected: &Path,
    actual: &Path,
    expected_data: Option<&Path>,
    actual_data: Option<&Path>,
) -> Result<Report> {
    let os = compare_os(expected, actual)?;
    let data = match (expected_data, actual_data) {
        (Some(e), Some(a)) => Some(compare_data(e, a)?),
        (None, None) => None,
        (e, a) => Some(missing_data(e, a)?),
    };
    let failed = os.failed() || data.as_ref().is_some_and(ImageReport::failed);
    Ok(Report {
        verdict: if failed {
            Verdict::Failed
        } else {
            Verdict::Verified
        },
        os,
        data,
        notes: NOTES.iter().map(|s| s.to_string()).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::tests::{write_gpt, MIB};
    use crate::disk::DATA_TYPE_GUID;
    use std::os::unix::fs::FileExt;

    fn image(dir: &Path, name: &str, len: u64) -> PathBuf {
        let path = dir.join(name);
        let mut f = File::create_new(&path).unwrap();
        write_gpt(
            &mut f,
            len,
            &[
                ("BOTTLEROCKET-ROOT-A", [1; 16], 1, 2),
                ("BOTTLEROCKET-PRIVATE", [2; 16], 3, 1),
                ("BOTTLEROCKET-ROOT-B", [1; 16], 4, 1),
                ("", DATA_TYPE_GUID, 5, 2),
            ],
        );
        f.write_all_at(b"rootfs", MIB).unwrap();
        f.write_all_at(b"private", 3 * MIB).unwrap();
        f.write_all_at(b"data", 5 * MIB).unwrap();
        path
    }

    pub(super) fn region<'a>(report: &'a ImageReport, name: &str) -> &'a RegionReport {
        report.regions.iter().find(|r| r.name == name).unwrap()
    }

    #[test]
    fn strict_regions() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let rebuilt = image(d, "rebuilt.img", 8 * MIB);
        // How the published image is changed, and the region that fails, if any.
        type Change = fn(&File);
        let cases: [(Change, Option<&str>); 5] = [
            // The published snapshot is padded to a whole volume size.
            (|f| f.set_len(16 * MIB).unwrap(), None),
            (|f| f.write_all_at(b"R", MIB).unwrap(), Some("ROOT-A")),
            (|f| f.write_all_at(b"x", 4 * MIB).unwrap(), Some("ROOT-B")),
            (
                |f| {
                    f.set_len(16 * MIB).unwrap();
                    f.write_all_at(b"x", 12 * MIB).unwrap();
                },
                Some("PADDING"),
            ),
            (|f| f.set_len(5).unwrap(), Some("SIZE")),
        ];
        for (i, (change, failing)) in cases.into_iter().enumerate() {
            let published = image(d, &format!("{i}.img"), 8 * MIB);
            change(&File::options().write(true).open(&published).unwrap());
            let report = compare(&rebuilt, &published, None, None).unwrap();
            match failing {
                None => assert_eq!(report.verdict, Verdict::Verified, "{report:#?}"),
                Some(name) => {
                    assert_eq!(report.verdict, Verdict::Failed, "{report:#?}");
                    assert_eq!(region(&report.os, name).status, Status::Mismatch);
                }
            }
        }
    }

    #[test]
    fn data_image() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let os_e = image(d, "e.img", 8 * MIB);
        let os_a = image(d, "a.img", 8 * MIB);
        type Data = Option<&'static [u8]>;
        let cases: [(Data, Data, Verdict); 4] = [
            (Some(b"data"), Some(b"data\0\0\0\0"), Verdict::Verified),
            (Some(b"data"), Some(b"dat4"), Verdict::Failed),
            (Some(b"data"), None, Verdict::Failed),
            (None, Some(b"data"), Verdict::Failed),
        ];
        for (i, (rebuilt, published, verdict)) in cases.into_iter().enumerate() {
            let data = |side: &str, contents: Option<&[u8]>| {
                contents.map(|contents| {
                    let path = d.join(format!("{i}-{side}.img"));
                    std::fs::write(&path, contents).unwrap();
                    path
                })
            };
            let (e, a) = (data("e", rebuilt), data("a", published));
            let report = compare(&os_e, &os_a, e.as_deref(), a.as_deref()).unwrap();
            assert_eq!(report.verdict, verdict, "{i}: {report:#?}");
            if verdict == Verdict::Failed {
                let data = report.data.as_ref().unwrap();
                assert_eq!(region(data, "DATA").status, Status::Mismatch);
            }
        }
    }
}

/// Comparisons of real filesystems, which need `mkfs.ext4` and erofs-utils.
#[cfg(test)]
mod rootfs_tests {
    use super::*;
    use crate::disk::tests::{write_gpt, MIB};
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{FileExt, PermissionsExt};
    use std::process::Command;

    struct Rootfs<'a> {
        /// Stands in for the signature appended to a kernel module.
        signing_key: &'a [u8],
        sbom: &'a str,
        os_release: &'a str,
        all_root: bool,
        /// Value of `user.big` on /etc/os-release, long enough to be shown by digest.
        big_xattr: &'a [u8],
        /// Whether /console is a character device, a copy of /dev/null.
        console: bool,
    }

    const BIG_XATTR: &[u8] = &[b'x'; 600];

    const DEFAULT: Rootfs<'static> = Rootfs {
        signing_key: b"key-one",
        sbom: "{\"spdx\":1}",
        os_release: "v1",
        all_root: true,
        big_xattr: BIG_XATTR,
        console: false,
    };

    fn set_xattr(path: &Path, name: &str, value: &[u8]) {
        let path = CString::new(path.as_os_str().as_bytes()).unwrap();
        let name = CString::new(name).unwrap();
        // SAFETY: the pointers are valid for the duration of the call.
        let rc = unsafe {
            libc::lsetxattr(
                path.as_ptr(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
            )
        };
        assert_eq!(rc, 0, "{}", std::io::Error::last_os_error());
    }

    fn populate(src: &Path, rootfs: &Rootfs) {
        let modules = src.join("lib/modules/6.1/kernel");
        std::fs::create_dir_all(&modules).unwrap();
        std::fs::create_dir_all(src.join("etc")).unwrap();
        std::fs::create_dir_all(src.join("usr/share/bottlerocket")).unwrap();
        std::fs::write(
            modules.join("xfs.ko"),
            [&b"\x7fELFxfs"[..], rootfs.signing_key].concat(),
        )
        .unwrap();
        std::fs::write(
            src.join("usr/share/bottlerocket/spdx-sbom.json"),
            rootfs.sbom,
        )
        .unwrap();
        std::fs::write(src.join("etc/os-release"), rootfs.os_release).unwrap();
        std::fs::write(src.join("etc/shadow"), "root:*").unwrap();
        std::fs::write(src.join("etc/sudo"), "suid").unwrap();
        std::os::unix::fs::symlink("../lib", src.join("etc/lib")).unwrap();
        // On enough entries that erofs stores it once, as a shared xattr.
        for p in ["etc/shadow", "etc/sudo", "etc/os-release", "lib"] {
            set_xattr(&src.join(p), "user.small", b"1");
        }
        set_xattr(&src.join("etc/os-release"), "user.big", rootfs.big_xattr);
        let chmod =
            |p: &str, mode| std::fs::set_permissions(src.join(p), PermissionsExt::from_mode(mode));
        chmod("etc/shadow", 0o400).unwrap();
        chmod("etc/sudo", 0o4755).unwrap();
        chmod("lib/modules", 0o555).unwrap();
    }

    fn mkfs(dir: &Path, name: &str, rootfs: &Rootfs) -> Vec<u8> {
        let src = dir.join(format!("{name}-src"));
        populate(&src, rootfs);
        let img = dir.join(format!("{name}.fs"));
        let mut cmd = Command::new("mkfs.erofs");
        cmd.args(["-T", "1", "-U", "11111111-1111-1111-1111-111111111111"]);
        cmd.args(["-z", "lz4hc,12"]);
        if rootfs.all_root {
            cmd.arg("--all-root");
        }
        if rootfs.console {
            // Creating device nodes needs root, so archive an existing one under a new name.
            let dev = "dev/null";
            let tar = dir.join(format!("{name}.tar"));
            let output = Command::new("tar")
                .args(["--format=posix", "--xattrs", "--xattrs-include=user.*"])
                .arg(format!("--transform=s,^{dev}$,./console,"))
                .arg("-cf")
                .arg(&tar)
                .arg("-C")
                .arg(&src)
                .arg(".")
                .args(["-C", "/", dev])
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            cmd.arg("--tar=f").arg(&img).arg(tar);
        } else {
            cmd.arg(&img).arg(&src);
        }
        let output = cmd.output().unwrap();
        assert!(output.status.success(), "{output:?}");
        std::fs::set_permissions(src.join("lib/modules"), PermissionsExt::from_mode(0o755))
            .unwrap();
        std::fs::read(img).unwrap()
    }

    /// A disk image whose ROOT-A holds `root`.
    fn root_image(dir: &Path, name: &str, root: &[u8]) -> PathBuf {
        let path = dir.join(format!("{name}.img"));
        let mut f = File::create_new(&path).unwrap();
        write_gpt(&mut f, 8 * MIB, &[("BOTTLEROCKET-ROOT-A", [1; 16], 1, 2)]);
        f.write_all_at(root, MIB).unwrap();
        path
    }

    fn os_image(dir: &Path, name: &str, rootfs: &Rootfs) -> PathBuf {
        root_image(dir, name, &mkfs(dir, name, rootfs))
    }

    #[test]
    #[ignore = "needs erofs-utils"]
    fn erofs_root() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let resigned = Rootfs {
            signing_key: b"key-two",
            ..DEFAULT
        };
        let new_sbom = Rootfs {
            sbom: "{\"spdx\":2}",
            ..DEFAULT
        };
        let mut big = BIG_XATTR.to_vec();
        *big.last_mut().unwrap() = b'y';
        // Bottlerocket root filesystems hold no device nodes, so one fails ROOT-A even when
        // both sides have it and the rest differs only in an SBOM.
        let console = Rootfs {
            console: true,
            ..DEFAULT
        };
        for (i, (rebuilt, published, status)) in [
            (&DEFAULT, DEFAULT, Status::Match),
            (&DEFAULT, Rootfs { ..new_sbom }, Status::ExpectedDifference),
            // Kernel modules must match like any other file; another key implies another kernel.
            (&DEFAULT, resigned, Status::Mismatch),
            // Any other difference fails, even alongside an expected one.
            (
                &DEFAULT,
                Rootfs {
                    os_release: "v2",
                    ..new_sbom
                },
                Status::Mismatch,
            ),
            (
                &DEFAULT,
                Rootfs {
                    big_xattr: &big,
                    ..new_sbom
                },
                Status::Mismatch,
            ),
            (
                &DEFAULT,
                Rootfs {
                    all_root: false,
                    ..DEFAULT
                },
                Status::Mismatch,
            ),
            (
                &console,
                Rootfs {
                    console: true,
                    ..new_sbom
                },
                Status::Mismatch,
            ),
        ]
        .iter()
        .enumerate()
        {
            let report = compare(
                &os_image(d, &format!("{i}-rebuilt"), rebuilt),
                &os_image(d, &format!("{i}-published"), published),
                None,
                None,
            )
            .unwrap();
            let r = super::tests::region(&report.os, "ROOT-A");
            assert_eq!(r.status, *status, "{i}: {r:#?}");
            if rebuilt.console {
                assert!(r.detail.contains("is a character device"), "{i}: {r:#?}");
            }
        }
    }
}
