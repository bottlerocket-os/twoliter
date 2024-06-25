use std::path::Path;

use async_trait::async_trait;
use snafu::ResultExt;

use crate::{cli::CommandLine, error, ConfigView, ImageTool, ImageView, Result};

pub struct CraneCLI {
    pub(crate) cli: CommandLine,
}

#[async_trait]
impl ImageTool for CraneCLI {
    async fn pull_oci_image(&self, path: &Path, uri: &str) -> Result<()> {
        let archive_path = path.to_string_lossy();
        self.cli
            .spawn(
                &["pull", "--format", "oci", uri, archive_path.as_ref()],
                format!("failed to pull image archive from {}", uri),
            )
            .await?;
        Ok(())
    }

    async fn get_manifest(&self, uri: &str) -> Result<Vec<u8>> {
        self.cli
            .output(
                &["manifest", uri],
                format!("failed to fetch manifest for resource at {}", uri),
            )
            .await
    }

    async fn get_config(&self, uri: &str) -> Result<ConfigView> {
        let bytes = self
            .cli
            .output(
                &["config", uri],
                format!("failed to fetch image config from {}", uri),
            )
            .await?;
        let image_view: ImageView =
            serde_json::from_slice(bytes.as_slice()).context(error::ConfigDeserializeSnafu)?;
        Ok(image_view.config)
    }
}
