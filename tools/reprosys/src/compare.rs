//! Compare a rebuilt disk image against the image published in an AMI.

use crate::disk::{self, DiskLayout, Policy, RangeHash};
use crate::error::Result;
use crate::fsfiles::{FileReport, Status};
use serde::Serialize;
use snafu::ResultExt;
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
    pub policy: Policy,
    pub status: Status,
    /// Whether a mismatch in this region fails verification.
    pub fatal: bool,
    pub offset: u64,
    pub len: u64,
    pub expected_sha256: Option<String>,
    pub actual_sha256: Option<String>,
    pub detail: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FileReport>,
}

impl RegionReport {
    /// A region that fails verification, with no digests.
    fn mismatch(name: &str, policy: Policy, offset: u64, len: u64, detail: String) -> Self {
        Self {
            name: name.to_string(),
            policy,
            status: Status::Mismatch,
            fatal: true,
            offset,
            len,
            expected_sha256: None,
            actual_sha256: None,
            detail,
            files: Vec::new(),
        }
    }

    fn failed(&self) -> bool {
        self.fatal && self.status == Status::Mismatch
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
        Ok(Some(assess_region(
            "PADDING",
            Policy::MustBeZero,
            &tail,
            None,
            &a,
        )))
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
    policy: Policy,
    ranges: &[(u64, u64)],
    expected: Option<&RangeHash>,
    actual: &RangeHash,
) -> RegionReport {
    let offset = ranges.first().map(|r| r.0).unwrap_or(0);
    let len = ranges.iter().map(|r| r.1).sum();
    let identical = expected.is_none_or(|e| e == actual);
    let (status, detail) = match policy {
        Policy::MustBeZero if !actual.all_zero => {
            (Status::Mismatch, "published image is not all zero")
        }
        _ if identical => (Status::Match, "identical"),
        _ => (Status::Mismatch, "contents differ"),
    };
    RegionReport {
        status,
        fatal: policy != Policy::Verity,
        expected_sha256: expected.map(RangeHash::hex),
        actual_sha256: Some(actual.hex()),
        ..RegionReport::mismatch(name, policy, offset, len, detail.to_string())
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
            Policy::MustMatch,
            0,
            expected_len,
            smaller(actual_len, expected_len),
        ));
        return Ok(report);
    }

    let layout = DiskLayout::read(&mut images.expected)?;
    let gpt_ranges = layout.gpt_ranges();
    let (e, a) = images.hash(&gpt_ranges)?;
    let mut gpt = assess_region("GPT", Policy::MustMatch, &gpt_ranges, Some(&e), &a);
    gpt.detail = match gpt.status {
        Status::Match => "protective MBR, primary and backup GPT identical".to_string(),
        _ => "partition tables differ; partition comparisons use the rebuilt layout".to_string(),
    };
    report.regions.push(gpt);

    let gaps = layout.unallocated(expected_len);
    if !gaps.is_empty() {
        let (e, a) = images.hash(&gaps)?;
        report.regions.push(assess_region(
            "UNALLOCATED",
            Policy::MustMatch,
            &gaps,
            Some(&e),
            &a,
        ));
    }

    for p in &layout.partitions {
        let policy = p.policy();
        let ranges = [(p.offset, p.len)];
        let (e, a) = images.hash(&ranges)?;
        let r = assess_region(&p.name, policy, &ranges, Some(&e), &a);
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

/// Compare the data image over the rebuilt image's length, and check that the rest is zero.
pub fn compare_data(expected: &Path, actual: &Path) -> Result<ImageReport> {
    let mut images = Images::open(expected, actual)?;
    let mut report = images.report();
    let (expected_len, actual_len) = (images.expected_len, images.actual_len);
    let common = [(0, expected_len.min(actual_len))];
    let (e, a) = images.hash(&common)?;
    let mut r = assess_region("DATA", Policy::MustMatch, &common, Some(&e), &a);
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
        regions: vec![RegionReport::mismatch(
            "DATA",
            Policy::MustMatch,
            0,
            len,
            detail.to_string(),
        )],
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
