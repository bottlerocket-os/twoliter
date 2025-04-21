use super::{snapshot::snapshot_from_image, AmiArgs};
use aws_sdk_ebs::Client as EbsClient;
use aws_sdk_ec2::types::Filter;
use aws_sdk_ec2::{config::Region, Client as Ec2Client};
use coldsnap::{SnapshotUploader, SnapshotWaiter};
use log::{debug, info, warn};
use snafu::{ensure, OptionExt, ResultExt};

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
    cleanup_snapshot_ids: &mut Vec<String>,
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

    debug!("Registering AMI with amispec '{:?}'", amispec);
    info!("Making register image call in {}", region);
    let register_response = amispec
        .as_register_image_call()
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
    async fn create_snapshots(
        ami_args: &AmiArgs,
        region: &Region,
        ebs_client: EbsClient,
        ec2_client: &Ec2Client,
        cleanup_snapshot_ids: &mut Vec<String>,
    ) -> Result<BottlerocketSnapshots> {
        debug!("Uploading images into EBS snapshots in {}", region);
        let uploader = SnapshotUploader::new(ebs_client);
        let os_snapshot =
            snapshot_from_image(&ami_args.os_image, &uploader, None, ami_args.no_progress)
                .await
                .context(error::SnapshotSnafu {
                    path: &ami_args.os_image,
                    region: region.as_ref(),
                })?;
        cleanup_snapshot_ids.push(os_snapshot.clone());

        let mut data_snapshot = None;
        if let Some(data_image) = &ami_args.data_image {
            let snapshot = snapshot_from_image(data_image, &uploader, None, ami_args.no_progress)
                .await
                .context(error::SnapshotSnafu {
                    path: &ami_args.os_image,
                    region: region.as_ref(),
                })?;
            cleanup_snapshot_ids.push(snapshot.clone());
            data_snapshot = Some(snapshot);
        }

        info!("Waiting for snapshots to become available in {}", region);
        let waiter = SnapshotWaiter::new(ec2_client.clone());
        waiter
            .wait(&os_snapshot, Default::default())
            .await
            .context(error::WaitSnapshotSnafu {
                snapshot_type: "root",
            })?;

        if let Some(ref data_snapshot) = data_snapshot {
            waiter
                .wait(&data_snapshot, Default::default())
                .await
                .context(error::WaitSnapshotSnafu {
                    snapshot_type: "data",
                })?;
        }

        Ok(BottlerocketSnapshots {
            os_snapshot,
            data_snapshot,
        })
    }
}

/// Uploads the given images into snapshots and registers an AMI using them as its block device
/// mapping.  Deletes snapshots on failure.
pub(crate) async fn register_image(
    ami_args: &AmiArgs,
    region: &Region,
    ebs_client: EbsClient,
    ec2_client: &Ec2Client,
) -> Result<RegisteredIds> {
    let mut cleanup_snapshot_ids = Vec::new();
    let register_result = _register_image(
        ami_args,
        region,
        ebs_client,
        ec2_client,
        &mut cleanup_snapshot_ids,
    )
    .await;

    if register_result.is_err() {
        for snapshot_id in cleanup_snapshot_ids {
            if let Err(e) = ec2_client
                .delete_snapshot()
                .set_snapshot_id(Some(snapshot_id.clone()))
                .send()
                .await
            {
                warn!(
                    "While cleaning up, failed to delete snapshot {}: {}",
                    snapshot_id, e
                );
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
    use crate::aws::ami;
    use aws_sdk_ec2::error::SdkError;
    use aws_sdk_ec2::operation::{
        describe_images::DescribeImagesError, register_image::RegisterImageError,
    };
    use snafu::Snafu;
    use std::path::PathBuf;

    use super::mk_amispec;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub(super)))]
    pub(crate) enum Error {
        #[snafu(display("Failed to create an amispec from publication inputs: {}", source))]
        Amispec { source: mk_amispec::AmispecError },

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

        #[snafu(display("Failed to upload snapshot from {} in {}: {}", path.display(),region, source))]
        Snapshot {
            path: PathBuf,
            region: String,
            source: ami::snapshot::Error,
        },

        #[snafu(display("{} snapshot did not become available: {}", snapshot_type, source))]
        WaitSnapshot {
            snapshot_type: String,
            source: coldsnap::WaitError,
        },
    }
}
pub(crate) use error::Error;
type Result<T> = std::result::Result<T, error::Error>;
