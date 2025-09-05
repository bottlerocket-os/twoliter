use super::{snapshot::snapshot_from_image, AmiArgs};
use crate::aws::ami::snapshot::build_progress_bar;
use aws_sdk_ebs::Client as EbsClient;
use aws_sdk_ec2::types::{Filter, ResourceType, Tag, TagSpecification};
use aws_sdk_ec2::{config::Region, Client as Ec2Client};
use coldsnap::{SnapshotUploader, SnapshotWaiter};
use futures::future::OptionFuture;
use futures::TryFutureExt as _;
use indicatif::{MultiProgress, ProgressBar};
use log::{debug, info, warn};
use snafu::{ensure, futures::TryFutureExt as _, OptionExt, ResultExt, Snafu};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

mod merge_toml;
pub(crate) mod mk_amispec;

#[derive(Debug)]
pub(crate) struct RegisteredIds {
    pub(crate) image_id: String,
    pub(crate) snapshot_ids: Vec<String>,
}

/// Helper for `register_image`.  Inserts registered snapshot IDs into `cleanup_snapshot_ids` so
/// they can be cleaned up on failure if desired.
async fn _register_image(
    ami_args: &AmiArgs,
    region: &Region,
    ebs_client: EbsClient,
    ec2_client: &Ec2Client,
    tags: Option<HashMap<String, String>>,
    cleanup_snapshot_ids: Arc<Mutex<Vec<String>>>,
) -> Result<RegisteredIds> {
    let bottlerocket_snapshots = BottlerocketSnapshots::create_snapshots(
        ami_args,
        region,
        ebs_client,
        ec2_client,
        cleanup_snapshot_ids,
    )
    .await?;

    debug!("Creating AMI spec from variant definition and pubsys args");
    let amispec = mk_amispec::create_amispec(ami_args, Some(&bottlerocket_snapshots))
        .context(error::AmispecSnafu)?;

    debug!("Registering AMI with amispec '{amispec:?}'");
    info!("Making register image call in {region}");
    let register_response = amispec
        .as_register_image_call()
        .set_tag_specifications(tags.map(|x| {
            vec![TagSpecification::builder()
                .resource_type(ResourceType::Image)
                .set_tags(Some(
                    x.iter()
                        .map(|(key, value)| Tag::builder().key(key).value(value).build())
                        .collect(),
                ))
                .build()]
        }))
        .send_with(ec2_client)
        .await
        .context(error::RegisterImageSnafu {
            region: region.as_ref(),
        })?;

    let image_id = register_response
        .image_id
        .context(error::MissingImageIdSnafu {
            region: region.as_ref(),
        })?;

    Ok(RegisteredIds {
        image_id,
        snapshot_ids: bottlerocket_snapshots.snapshot_ids(),
    })
}

#[derive(Debug, Clone)]
pub(crate) struct BottlerocketSnapshots {
    pub(crate) os_snapshot: String,
    pub(crate) data_snapshot: Option<String>,
}

impl BottlerocketSnapshots {
    fn snapshot_ids(&self) -> Vec<String> {
        let mut snapshot_ids = vec![self.os_snapshot.clone()];
        if let Some(ref data_snapshot) = self.data_snapshot {
            snapshot_ids.push(data_snapshot.clone());
        }
        snapshot_ids
    }

    /// Creates the EBS snapshots for the OS and (optional) data volume
    ///
    /// Snapshot IDs are added to `cleanup_snapshot_ids` so that they can be deleted on cleanup.
    async fn create_snapshot(
        snapshot_uploader: &SnapshotUploader,
        snapshot_waiter: &SnapshotWaiter,
        image_path: impl AsRef<Path>,
        cleanup_snapshot_ids: Arc<Mutex<Vec<String>>>,
        progress_bar: Option<ProgressBar>,
    ) -> Result<String, CreateSnapshotError> {
        use create_snapshot_error::*;
        let image_path = image_path.as_ref();

        let snapshot_id = snapshot_from_image(image_path, snapshot_uploader, None, progress_bar)
            .await
            .context(UploadSnapshotSnafu { path: image_path })?;

        cleanup_snapshot_ids.lock().await.push(snapshot_id.clone());

        debug!("Waiting for snapshot {snapshot_id} to become available");
        snapshot_waiter
            .wait(&snapshot_id, Default::default())
            .await
            .context(WaitSnapshotSnafu)?;

        Ok(snapshot_id)
    }

    /// Creates the EBS snapshots for the OS and (optional) data volume
    ///
    /// Snapshot IDs are added to `cleanup_snapshot_ids` so that they can be deleted on cleanup.
    async fn create_snapshots(
        ami_args: &AmiArgs,
        region: &Region,
        ebs_client: EbsClient,
        ec2_client: &Ec2Client,
        cleanup_snapshot_ids: Arc<Mutex<Vec<String>>>,
    ) -> Result<BottlerocketSnapshots> {
        debug!("Uploading images into EBS snapshots in {region}");

        let multi_progress = MultiProgress::new();

        let create_snapshot_task = |image_path: PathBuf, image_name: &'static str| {
            info!("Registering '{image_name}' snapshot in region '{region}'");
            let progress_bar = (!ami_args.no_progress)
                .then_some({
                    let pb = build_progress_bar(&format!("Upload {image_name} snapshot"))
                        .context(error::ProgressBarSnafu)?;
                    Ok(multi_progress.add(pb))
                })
                .transpose()?;

            let uploader = SnapshotUploader::new(ebs_client.clone());
            let waiter = SnapshotWaiter::new(ec2_client.clone());
            let cloned_image_path = image_path.clone();

            Ok(tokio::spawn(async move {
                Self::create_snapshot(
                    &uploader,
                    &waiter,
                    &cloned_image_path,
                    Arc::clone(&cleanup_snapshot_ids),
                    progress_bar.clone(),
                )
                .err_into()
                .await
            })
            .err_into()
            .map_ok(std::future::ready)
            .try_flatten()
            .context(error::SnapshotSnafu {
                snapshot_type: image_name.to_string(),
                path: image_path,
                region: region.to_string(),
            }))
        };

        let os_upload = create_snapshot_task.clone()(ami_args.os_image.clone(), "root")?;

        let data_upload: OptionFuture<_> = ami_args
            .data_image
            .as_ref()
            .map(|data_image| create_snapshot_task(data_image.clone(), "data"))
            .transpose()?
            .into();

        let (os_snapshot, data_snapshot) = tokio::join!(os_upload, data_upload);
        multi_progress.clear().context(error::ClearProgressSnafu)?;

        let (os_snapshot, data_snapshot) = (os_snapshot?, data_snapshot.transpose()?);

        Ok(BottlerocketSnapshots {
            os_snapshot,
            data_snapshot,
        })
    }
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub(crate) enum CreateSnapshotError {
    #[snafu(display("Failed to create snapshot for {}: {}", path.display(), source))]
    UploadSnapshot {
        path: PathBuf,
        #[snafu(source(from(crate::aws::ami::snapshot::Error, Box::new)))]
        source: Box<crate::aws::ami::snapshot::Error>,
    },
    #[snafu(display("Failed to wait for snapshot to become available: {}", source))]
    WaitSnapshot { source: coldsnap::WaitError },
}

/// Uploads the given images into snapshots and registers an AMI using them as its block device
/// mapping.  Deletes snapshots on failure.
pub(crate) async fn register_image(
    ami_args: &AmiArgs,
    region: &Region,
    ebs_client: EbsClient,
    ec2_client: &Ec2Client,
    tags: Option<HashMap<String, String>>,
) -> Result<RegisteredIds> {
    let cleanup_snapshot_ids = Arc::new(Mutex::new(Vec::new()));
    let register_result = _register_image(
        ami_args,
        region,
        ebs_client,
        ec2_client,
        tags,
        Arc::clone(&cleanup_snapshot_ids),
    )
    .await;

    if register_result.is_err() {
        for snapshot_id in cleanup_snapshot_ids.lock().await.iter() {
            if let Err(e) = ec2_client
                .delete_snapshot()
                .set_snapshot_id(Some(snapshot_id.clone()))
                .send()
                .await
            {
                warn!("While cleaning up, failed to delete snapshot {snapshot_id}: {e}");
            }
        }
    }
    register_result
}

/// Queries EC2 for the given AMI name. If found, returns Ok(Some(id)), if not returns Ok(None).
pub(crate) async fn get_ami_id<S>(
    name: S,
    arch: impl Into<String>,
    region: &Region,
    ec2_client: &Ec2Client,
) -> Result<Option<String>>
where
    S: Into<String>,
{
    let describe_response = ec2_client
        .describe_images()
        .set_owners(Some(vec!["self".to_string()]))
        .set_filters(Some(vec![
            Filter::builder()
                .set_name(Some("name".to_string()))
                .set_values(Some(vec![name.into()]))
                .build(),
            Filter::builder()
                .set_name(Some("architecture".to_string()))
                .set_values(Some(vec![arch.into()]))
                .build(),
            Filter::builder()
                .set_name(Some("image-type".to_string()))
                .set_values(Some(vec!["machine".to_string()]))
                .build(),
            Filter::builder()
                .set_name(Some("virtualization-type".to_string()))
                .set_values(Some(vec![mk_amispec::VIRT_TYPE.to_string()]))
                .build(),
        ]))
        .send()
        .await
        .context(error::DescribeImagesSnafu {
            region: region.as_ref(),
        })?;
    if let Some(mut images) = describe_response.images {
        if images.is_empty() {
            return Ok(None);
        }
        ensure!(
            images.len() == 1,
            error::MultipleImagesSnafu {
                images: images
                    .into_iter()
                    .map(|i| i.image_id.unwrap_or_else(|| "<missing>".to_string()))
                    .collect::<Vec<_>>()
            }
        );
        let image = images.remove(0);
        // If there is an image but we couldn't find the ID of it, fail rather than returning None,
        // which would indicate no image.
        let id = image.image_id.context(error::MissingImageIdSnafu {
            region: region.as_ref(),
        })?;
        Ok(Some(id))
    } else {
        Ok(None)
    }
}

mod error {
    use super::mk_amispec;
    use aws_sdk_ec2::error::SdkError;
    use aws_sdk_ec2::operation::{
        describe_images::DescribeImagesError, register_image::RegisterImageError,
    };
    use snafu::Snafu;
    use std::path::PathBuf;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub(super)))]
    pub(crate) enum Error {
        #[snafu(display("Failed to create an amispec from publication inputs: {}", source))]
        Amispec { source: mk_amispec::AmispecError },

        #[snafu(display("Failed to clear progress bar: {}", source))]
        ClearProgress { source: std::io::Error },

        #[snafu(display("Failed to describe images in {}: {}", region, source))]
        DescribeImages {
            region: String,
            source: SdkError<DescribeImagesError>,
        },

        #[snafu(display("Image response in {} did not include image ID", region))]
        MissingImageId { region: String },

        #[snafu(display("DescribeImages with unique filters returned multiple results: {}", images.join(", ")))]
        MultipleImages { images: Vec<String> },

        #[snafu(display("Failed to register image in {}: {}", region, source))]
        RegisterImage {
            region: String,
            source: SdkError<RegisterImageError>,
        },

        #[snafu(display("Failed to create progress bar: {}", source))]
        ProgressBar {
            source: crate::aws::ami::snapshot::Error,
        },

        #[snafu(display("Failed to upload {} snapshot from {} in {}: {}", snapshot_type, path.display(),region, source))]
        Snapshot {
            snapshot_type: String,
            path: PathBuf,
            region: String,
            source: Box<dyn std::error::Error + Send + Sync>,
        },
    }
}
pub(crate) use error::Error;
type Result<T, E = error::Error> = std::result::Result<T, E>;
