//! Download an AMI's snapshots, copying them into the caller's account if needed.

use crate::ami::AmiInfo;
use crate::error::Result;
use aws_sdk_ebs::operation::list_snapshot_blocks::ListSnapshotBlocksError;
use aws_sdk_ebs::Client as EbsClient;
use aws_sdk_ec2::types::SnapshotState;
use aws_sdk_ec2::Client as Ec2Client;
use coldsnap::SnapshotDownloader;
use serde::{Deserialize, Serialize};
use snafu::{whatever, OptionExt, ResultExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const COPY_POLL_INTERVAL: Duration = Duration::from_secs(15);
const COPY_TIMEOUT: Duration = Duration::from_secs(3 * 60 * 60);
/// Consecutive DescribeSnapshots failures tolerated while waiting for a copy.
const MAX_DESCRIBE_FAILURES: u32 = 5;
/// How long to keep retrying a completed snapshot that the EBS direct APIs do not find yet. They
/// lag behind EC2: a snapshot uploaded through them reads as completed in DescribeSnapshots about
/// a minute before ListSnapshotBlocks finds it.
const NOT_FOUND_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Paths of the downloaded images.
#[derive(Debug, Serialize, Deserialize)]
pub struct Downloaded {
    pub root_image: PathBuf,
    pub data_image: Option<PathBuf>,
}

pub struct Downloader {
    ec2: Ec2Client,
    ebs: EbsClient,
    region: String,
    /// Snapshot copies not yet deleted.
    copies: Mutex<Vec<String>>,
}

impl Downloader {
    pub fn new(ec2: Ec2Client, ebs: EbsClient, region: String) -> Self {
        Self {
            ec2,
            ebs,
            region,
            copies: Mutex::new(Vec::new()),
        }
    }

    /// Delete the snapshot copies of an interrupted download.
    pub async fn cleanup(&self) {
        let copies = std::mem::take(&mut *self.copies.lock().expect("lock poisoned"));
        for copy in copies {
            self.cleanup_copy(&copy).await;
        }
    }

    /// Delete a snapshot copy, reporting failures so it can be deleted manually.
    async fn cleanup_copy(&self, copy: &str) {
        self.copies
            .lock()
            .expect("lock poisoned")
            .retain(|c| c != copy);
        if let Err(e) = self.ec2.delete_snapshot().snapshot_id(copy).send().await {
            eprintln!(
                "Warning: failed to delete snapshot copy {copy}; delete it manually: {}",
                snafu::Report::from_error(e)
            );
        }
    }

    /// Download the AMI's root (and data, if present) snapshots into `out_dir`.
    pub async fn download(&self, info: &AmiInfo, out_dir: &Path) -> Result<Downloaded> {
        std::fs::create_dir_all(out_dir)
            .whatever_context(format!("failed to create {}", out_dir.display()))?;

        let root_image = out_dir.join("root.img");
        self.download_snapshot(info, &info.root_snapshot, &root_image)
            .await?;

        let data_image = match &info.data_snapshot {
            Some(snapshot) => {
                let path = out_dir.join("data.img");
                self.download_snapshot(info, snapshot, &path).await?;
                Some(path)
            }
            None => None,
        };

        Ok(Downloaded {
            root_image,
            data_image,
        })
    }

    async fn download_snapshot(&self, info: &AmiInfo, snapshot: &str, path: &Path) -> Result<()> {
        if info.owned_by_caller {
            // An AMI registered moments ago can still have pending snapshots.
            self.wait_for_snapshot(snapshot).await?;
            eprintln!("Downloading {snapshot} to {}", path.display());
            return self.coldsnap(snapshot, path).await;
        }

        // EBS direct APIs only read snapshots owned by the caller, so copy it first.
        let copy = self.copy_snapshot(info, snapshot).await?;
        eprintln!(
            "Downloading {copy} (copy of {snapshot}) to {}",
            path.display()
        );
        let result = self.coldsnap(&copy, path).await;
        self.cleanup_copy(&copy).await;
        result
    }

    async fn coldsnap(&self, snapshot: &str, path: &Path) -> Result<()> {
        let start = Instant::now();
        loop {
            // coldsnap refuses to overwrite an existing file.
            if path.exists() {
                std::fs::remove_file(path)
                    .whatever_context(format!("failed to remove {}", path.display()))?;
            }
            let result = SnapshotDownloader::new(self.ebs.clone())
                .download_to_file(snapshot, path, None, None, None)
                .await;
            match result {
                Ok(()) => return Ok(()),
                Err(e) if is_not_found(&e) && start.elapsed() < NOT_FOUND_TIMEOUT => {
                    eprintln!(
                        "Snapshot {snapshot} is not readable through the EBS direct APIs yet; \
                         retrying in {}s",
                        COPY_POLL_INTERVAL.as_secs()
                    );
                    tokio::time::sleep(COPY_POLL_INTERVAL).await;
                }
                Err(e) if is_not_found(&e) => {
                    return Err(e).whatever_context(format!(
                        "the EBS direct APIs did not find completed snapshot {snapshot} within \
                         {} minutes",
                        NOT_FOUND_TIMEOUT.as_secs() / 60
                    ));
                }
                Err(e) => {
                    return Err(e).whatever_context(format!(
                        "failed to download snapshot {snapshot}; the caller needs \
                         ebs:ListSnapshotBlocks and ebs:GetSnapshotBlock"
                    ));
                }
            }
        }
    }

    async fn copy_snapshot(&self, info: &AmiInfo, snapshot: &str) -> Result<String> {
        eprintln!(
            "Snapshot {snapshot} is owned by {}; copying it into this account",
            info.owner
        );
        let response = self
            .ec2
            .copy_snapshot()
            .source_region(&self.region)
            .source_snapshot_id(snapshot)
            .description(format!(
                "reprosys copy of {snapshot} from {} for verification",
                info.ami_id
            ))
            .send()
            .await
            .whatever_context(format!(
                "failed to copy snapshot {snapshot}; the caller needs ec2:CopySnapshot"
            ))?;
        let copy = response
            .snapshot_id()
            .whatever_context("CopySnapshot returned no snapshot ID")?
            .to_string();
        self.copies
            .lock()
            .expect("lock poisoned")
            .push(copy.clone());

        if let Err(e) = self.wait_for_snapshot(&copy).await {
            self.cleanup_copy(&copy).await;
            return Err(e);
        }
        Ok(copy)
    }

    async fn wait_for_snapshot(&self, snapshot: &str) -> Result<()> {
        let start = Instant::now();
        let mut failures = 0;
        loop {
            let response = match self
                .ec2
                .describe_snapshots()
                .snapshot_ids(snapshot)
                .send()
                .await
            {
                Ok(response) => {
                    failures = 0;
                    response
                }
                // A transient error should not end a wait that can last hours.
                Err(e) if failures + 1 < MAX_DESCRIBE_FAILURES => {
                    failures += 1;
                    eprintln!(
                        "Warning: failed to describe snapshot {snapshot} (attempt {failures} of \
                         {MAX_DESCRIBE_FAILURES}): {}",
                        snafu::Report::from_error(e)
                    );
                    tokio::time::sleep(COPY_POLL_INTERVAL).await;
                    continue;
                }
                Err(e) => {
                    return Err(e)
                        .whatever_context(format!("failed to describe snapshot {snapshot}"));
                }
            };
            let snap = response
                .snapshots()
                .first()
                .whatever_context(format!("snapshot {snapshot} not found"))?;
            match snap.state() {
                Some(SnapshotState::Completed) => return Ok(()),
                Some(SnapshotState::Error) => {
                    whatever!(
                        "snapshot {snapshot} failed: {}",
                        snap.state_message().unwrap_or("unknown error")
                    );
                }
                _ => {}
            }
            if start.elapsed() > COPY_TIMEOUT {
                whatever!("timed out waiting for snapshot {snapshot}");
            }
            eprintln!(
                "Waiting for snapshot {snapshot} ({})",
                snap.progress().unwrap_or("0%")
            );
            tokio::time::sleep(COPY_POLL_INTERVAL).await;
        }
    }
}

/// Whether the EBS direct APIs reported the snapshot as not found.
fn is_not_found(e: &coldsnap::DownloadError) -> bool {
    std::iter::successors(Some(e as &dyn std::error::Error), |e| e.source()).any(|e| {
        // An SDK service error's source is the operation's error.
        e.downcast_ref::<ListSnapshotBlocksError>()
            .is_some_and(ListSnapshotBlocksError::is_resource_not_found_exception)
    })
}
