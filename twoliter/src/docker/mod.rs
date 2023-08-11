mod commands;
mod image;
mod twoliter;

pub(crate) use self::commands::DockerBuild;
pub(crate) use self::image::{ImageArchUri, ImageUri};
pub(crate) use self::twoliter::create_twoliter_image_if_not_exists;

pub(super) const DEFAULT_REGISTRY: &str = "public.ecr.aws/bottlerocket";
pub(super) const DEFAULT_SDK_NAME: &str = "bottlerocket-sdk";
// TODO - get this from lock file: https://github.com/bottlerocket-os/twoliter/issues/11
pub(super) const DEFAULT_SDK_VERSION: &str = "v0.33.0";
