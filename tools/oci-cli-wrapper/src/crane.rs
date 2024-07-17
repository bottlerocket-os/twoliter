use std::fs::File;
use std::path::Path;

use async_trait::async_trait;
use snafu::ResultExt;
use tar::Archive as TarArchive;
use tempfile::TempDir;

use crate::{
    cli::CommandLine, error, ConfigView, DockerArchitecture, ImageToolImpl, ImageView, Result,
};

#[derive(Debug)]
pub struct CraneCLI {
    pub(crate) cli: CommandLine,
}

#[async_trait]
impl ImageToolImpl for CraneCLI {
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

    async fn push_oci_archive(&self, path: &Path, uri: &str) -> Result<()> {
        let temp_dir = TempDir::new_in(path.parent().unwrap()).context(error::CraneTempSnafu)?;

        let mut oci_file = File::open(path).context(error::ArchiveReadSnafu)?;

        let mut oci_archive = TarArchive::new(&mut oci_file);
        oci_archive
            .unpack(temp_dir.path())
            .context(error::ArchiveExtractSnafu)?;
        self.cli
            .spawn(
                &["push", &temp_dir.path().to_string_lossy(), uri],
                format!("failed to push image {}", uri),
            )
            .await
    }

    async fn push_multi_platform_manifest(
        &self,
        platform_images: Vec<(DockerArchitecture, String)>,
        uri: &str,
    ) -> Result<()> {
        let images: Vec<&str> = platform_images
            .iter()
            .map(|(_, image)| image.as_str())
            .collect();

        let mut manifest_create_args = vec!["index", "append"];
        for image in images {
            manifest_create_args.extend_from_slice(&["-m", image])
        }
        manifest_create_args.extend_from_slice(&["-t", uri]);
        self.cli
            .output(
                &manifest_create_args,
                format!("could not push multi-platform manifest to {}", uri),
            )
            .await?;

        Ok(())
    }
}
