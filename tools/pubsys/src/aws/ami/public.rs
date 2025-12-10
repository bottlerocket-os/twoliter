use aws_sdk_ec2::Client as Ec2Client;
use snafu::{ensure, OptionExt, ResultExt};

/// Returns whether or not the given AMI ID refers to a public AMI.
pub(crate) async fn ami_is_public(
    ec2_client: &Ec2Client,
    region: &str,
    ami_id: &str,
) -> Result<bool> {
    let ec2_response = ec2_client
        .describe_images()
        .image_ids(ami_id.to_string())
        .send()
        .await
        .map_err(error_utils::AwsSdkError::from)
        .context(error::DescribeImagesSnafu { ami_id, region })?;

    let returned_images = ec2_response.images();

    ensure!(
        returned_images.len() <= 1,
        error::TooManyImagesSnafu {
            ami_id: ami_id.to_string(),
            region: region.to_string(),
        }
    );

    Ok(returned_images
        .first()
        .context(error::NoSuchImageSnafu {
            ami_id: ami_id.to_string(),
            region: region.to_string(),
        })?
        .public()
        .unwrap_or(false))
}

mod error {
    use aws_sdk_ec2::operation::describe_images::DescribeImagesError;
    use error_utils::AwsSdkError;
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub(super)))]
    pub(crate) enum Error {
        #[snafu(display("Error describing AMI {} in {}: {}", ami_id, region, source))]
        DescribeImages {
            ami_id: String,
            region: String,
            #[snafu(source(from(AwsSdkError<DescribeImagesError>, Box::new)))]
            source: Box<AwsSdkError<DescribeImagesError>>,
        },

        #[snafu(display("AMI {} not found in {}", ami_id, region))]
        NoSuchImage { ami_id: String, region: String },

        #[snafu(display("Multiples AMIs with ID {} found in  {}", ami_id, region))]
        TooManyImages { ami_id: String, region: String },
    }
}
pub(crate) use error::Error;
type Result<T> = std::result::Result<T, error::Error>;
