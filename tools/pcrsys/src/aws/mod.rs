//! AWS-specific functionality for PCR prediction.

pub mod ami;
mod uefi;
mod zlib_dict;

pub use uefi::parse_aws_uefi_data;

use aws_config::profile::ProfileFileCredentialsProvider;
use aws_types::region::Region;
use aws_types::SdkConfig;

/// Build AWS SDK config, handling region and profile options like coldsnap.
pub async fn build_client_config(region: Option<&String>, profile: Option<&String>) -> SdkConfig {
    let config = match (region, profile) {
        (Some(r), _) => aws_config::from_env().region(Region::new(r.clone())),
        (None, Some(p)) => aws_config::from_env().region(
            aws_config::profile::ProfileFileRegionProvider::builder()
                .profile_name(p)
                .build(),
        ),
        (None, None) => aws_config::from_env(),
    };

    let config = match profile {
        Some(p) => config.credentials_provider(
            ProfileFileCredentialsProvider::builder()
                .profile_name(p)
                .build(),
        ),
        None => config,
    };

    config.load().await
}
