use std::path::Path;

use async_trait::async_trait;
use snafu::ResultExt;
use std::fs::File;
use tar::Archive;
use tempfile::NamedTempFile;

use crate::cli::CommandLine;
use crate::{error, ConfigView, ImageTool, Result};

pub struct DockerCLI {
    pub(crate) cli: CommandLine,
}

#[async_trait]
impl ImageTool for DockerCLI {
    async fn pull_oci_image(&self, path: &Path, uri: &str) -> Result<()> {
        // First we pull the image to local daemon
        self.cli
            .spawn(
                &["pull", uri],
                format!("failed to pull image to local docker from {}", uri),
            )
            .await?;
        // Now we can use docker save to save the archive to a temppath
        let temp_file = NamedTempFile::new_in(path).context(crate::error::DockerTempSnafu)?;
        let tmp_path = temp_file.path().to_string_lossy();
        self.cli
            .spawn(
                &["save", uri, "-o", tmp_path.as_ref()],
                format!("failed to save image archive from {} to {}", uri, tmp_path),
            )
            .await?;
        let archive_file = File::open(temp_file.path()).context(crate::error::ArchiveReadSnafu)?;
        let mut archive = Archive::new(archive_file);
        archive
            .unpack(path)
            .context(crate::error::ArchiveExtractSnafu)?;
        Ok(())
    }

    async fn get_manifest(&self, uri: &str) -> Result<Vec<u8>> {
        self.cli
            .output(
                &["manifest", "inspect", uri],
                format!("failed to inspect manifest of resource at {}", uri),
            )
            .await
    }

    async fn get_config(&self, uri: &str) -> Result<ConfigView> {
        self.cli
            .spawn(&["pull", uri], format!("failed to pull image from {}", uri))
            .await?;
        let bytes = self
            .cli
            .output(
                &[
                    "image",
                    "inspect",
                    uri,
                    "--format",
                    "\"{{ json .Config }}\"",
                ],
                format!("failed to fetch image config from {}", uri),
            )
            .await?;
        serde_json::from_slice(bytes.as_slice()).context(error::ConfigDeserializeSnafu)
    }
}
