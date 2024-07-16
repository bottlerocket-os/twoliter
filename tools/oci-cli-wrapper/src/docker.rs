use std::path::Path;

use async_trait::async_trait;
use regex::Regex;
use snafu::{OptionExt, ResultExt};
use std::fs::File;
use tar::Archive;
use tempfile::NamedTempFile;

use crate::cli::CommandLine;
use crate::{error, ConfigView, DockerArchitecture, ImageTool, Result};

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
                &["image", "inspect", uri, "--format", "{{ json .Config }}"],
                format!("failed to fetch image config from {}", uri),
            )
            .await?;
        serde_json::from_slice(bytes.as_slice()).context(error::ConfigDeserializeSnafu)
    }

    async fn push_oci_archive(&self, path: &Path, uri: &str) -> Result<()> {
        let out = self
            .cli
            .output(
                &["load", format!("--input={}", path.display()).as_str()],
                format!("could not load archive from {}", path.display()),
            )
            .await?;
        let out = String::from_utf8_lossy(&out);
        let digest_expression =
            Regex::new("(?<digest>sha256:[0-9a-f]{64})").context(error::RegexSnafu)?;
        let caps = digest_expression
            .captures(&out)
            .context(error::NoDigestSnafu)?;
        let digest = &caps["digest"];

        self.cli
            .output(
                &["tag", digest, uri],
                format!("could not tag image as {uri}"),
            )
            .await?;

        self.cli
            .spawn(&["push", uri], format!("failed to push image '{uri}'"))
            .await?;

        Ok(())
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

        let mut manifest_create_args = vec!["manifest", "create", uri];
        manifest_create_args.extend_from_slice(&images);
        self.cli
            .output(
                &manifest_create_args,
                format!("could not create manifest list {uri}"),
            )
            .await?;

        for (arch, image) in platform_images.iter() {
            self.cli
                .output(
                    &[
                        "manifest",
                        "annotate",
                        format!("--arch={}", arch).as_str(),
                        uri,
                        image,
                    ],
                    format!("could not annotate manifest {uri} for arch {arch}"),
                )
                .await?;
        }

        self.cli
            .output(
                &["manifest", "push", uri],
                format!("could not push manifest to {uri}"),
            )
            .await?;

        self.cli
            .output(
                &["manifest", "rm", uri],
                format!("could not delete manifest {uri}"),
            )
            .await?;

        Ok(())
    }
}
