//! AMI-based PCR prediction support.

use super::parse_aws_uefi_data;
use crate::efi::EfiVars;
use crate::error::Result;
use aws_sdk_ec2::types::ImageAttributeName;
use aws_sdk_ec2::Client;
use snafu::{OptionExt, ResultExt};

/// Retrieve UEFI data from an AMI using DescribeImageAttribute.
pub async fn get_uefi_data(ec2_client: &Client, ami_id: &str) -> Result<EfiVars> {
    let response = ec2_client
        .describe_image_attribute()
        .image_id(ami_id)
        .attribute(ImageAttributeName::UefiData)
        .send()
        .await
        .whatever_context("failed to describe image attribute")?;

    let uefi_b64 = response
        .uefi_data()
        .and_then(|v| v.value())
        .whatever_context("AMI has no UEFI data (not a UEFI boot mode AMI?)")?;

    parse_aws_uefi_data(uefi_b64)
}

/// Find the root snapshot ID from an AMI.
pub async fn get_root_snapshot_id(ec2_client: &Client, ami_id: &str) -> Result<String> {
    let response = ec2_client
        .describe_images()
        .image_ids(ami_id)
        .send()
        .await
        .whatever_context("failed to describe images")?;

    let image = response
        .images()
        .first()
        .whatever_context("AMI not found")?;

    let root_device = image.root_device_name().unwrap_or("/dev/xvda");

    image
        .block_device_mappings()
        .iter()
        .find(|bdm| bdm.device_name() == Some(root_device))
        .and_then(|bdm| bdm.ebs())
        .and_then(|ebs| ebs.snapshot_id())
        .map(String::from)
        .whatever_context("no root snapshot found in AMI")
}
