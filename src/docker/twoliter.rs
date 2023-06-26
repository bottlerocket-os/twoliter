use crate::docker::{DockerBuild, ImageArchUri, ImageUri};
use anyhow::{Context, Result};
use tempfile::TempDir;
use tokio::fs;

const TWOLITER_DOCKERFILE: &str = include_str!("Twoliter.dockerfile");

/// Creates the container needed for twoliter to use as its build environment.
pub(crate) async fn create_twoliter_image_if_not_exists(sdk: &ImageArchUri) -> Result<ImageUri> {
    // TODO - exit early if exists https://github.com/bottlerocket-os/twoliter/issues/12
    let temp_dir = TempDir::new()
        .context("Unable to create a temporary directory for Twoliter image creation")?;
    let empty_dir = temp_dir.path().join("context");
    fs::create_dir_all(&empty_dir).await.context(format!(
        "Unable to create directory '{}'",
        empty_dir.display()
    ))?;

    // TODO - copy buildsys, etc https://github.com/bottlerocket-os/twoliter/issues/9
    fs::write(empty_dir.join("buildsys"), "echo \"Hello from buildsys!\"")
        .await
        .context(format!(
            "Unable to write to '{}'",
            empty_dir.join("buildsys").display()
        ))?;

    let dockerfile_path = temp_dir.path().join("Twoliter.dockerfile");
    fs::write(&dockerfile_path, TWOLITER_DOCKERFILE)
        .await
        .context(format!(
            "Unable to write to '{}'",
            dockerfile_path.display()
        ))?;

    // TODO - correctly tag https://github.com/bottlerocket-os/twoliter/issues/12
    let image_uri = ImageUri::new(None, "twoliter", "latest");

    DockerBuild::default()
        .dockerfile(dockerfile_path)
        .context_dir(empty_dir)
        .build_arg("BASE", sdk.uri())
        .tag(image_uri.clone())
        .execute()
        .await
        .context("Unable to build the twoliter container")?;

    Ok(image_uri)
}
