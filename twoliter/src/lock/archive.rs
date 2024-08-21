use super::views::{IndexView, ManifestLayoutView};
use crate::common::fs::{create_dir_all, read, read_to_string, remove_dir_all, write};
use anyhow::{Context, Result};
use oci_cli_wrapper::ImageTool;
use std::fs::File;
use std::path::{Path, PathBuf};
use tar::Archive as TarArchive;
use tracing::{debug, instrument, trace};

#[derive(Debug)]
pub(crate) struct OCIArchive {
    registry: String,
    repository: String,
    digest: String,
    cache_dir: PathBuf,
}

impl OCIArchive {
    pub fn new<P>(registry: &str, repository: &str, digest: &str, cache_dir: P) -> Result<Self>
    where
        P: AsRef<Path>,
    {
        Ok(Self {
            registry: registry.into(),
            repository: repository.into(),
            digest: digest.into(),
            cache_dir: cache_dir.as_ref().to_path_buf(),
        })
    }

    pub fn archive_path(&self) -> PathBuf {
        self.cache_dir.join(self.digest.replace(':', "-"))
    }

    pub fn uri(&self) -> String {
        format!("{}/{}@{}", self.registry, self.repository, self.digest)
    }

    #[instrument(level = "trace", skip_all, fields(registry = %self.registry, repository = %self.repository, digest = %self.digest))]
    pub async fn pull_image(&self, image_tool: &ImageTool) -> Result<()> {
        let digest_uri = self.uri();
        debug!("Pulling image '{}'", digest_uri);
        let oci_archive_path = self.archive_path();
        if !oci_archive_path.exists() {
            create_dir_all(&oci_archive_path).await?;
            image_tool
                .pull_oci_image(oci_archive_path.as_path(), digest_uri.as_str())
                .await?;
        } else {
            debug!(
                "Image from '{}' already present -- no need to pull.",
                digest_uri
            );
        }
        Ok(())
    }

    #[instrument(
        level = "trace",
        skip_all,
        fields(registry = %self.registry, repository = %self.repository, digest = %self.digest, out_dir = %out_dir.as_ref().display()),
    )]
    pub async fn unpack_layers<P>(&self, out_dir: P) -> Result<()>
    where
        P: AsRef<Path>,
    {
        let path = out_dir.as_ref();
        let digest_file = path.join("digest");
        let digest_uri = self.uri();
        if digest_file.exists() {
            let digest = read_to_string(&digest_file).await.context(format!(
                "failed to read digest file at {}",
                digest_file.display()
            ))?;
            if digest == self.digest {
                trace!(
                    "Found existing digest file for image from '{}' at '{}'",
                    digest_uri,
                    digest_file.display()
                );
                return Ok(());
            }
        }

        debug!("Unpacking layers for image from '{}'", digest_uri);
        remove_dir_all(path).await?;
        create_dir_all(path).await?;
        let index_bytes = read(self.archive_path().join("index.json")).await?;
        let index: IndexView = serde_json::from_slice(index_bytes.as_slice())
            .context("failed to deserialize oci image index")?;

        // Read the manifest so we can get the layer digests
        trace!(from = %digest_uri, "Extracting layer digests from image manifest");
        let digest = index
            .manifests
            .first()
            .context("empty oci image")?
            .digest
            .replace(':', "/");
        let manifest_bytes = read(self.archive_path().join(format!("blobs/{digest}")))
            .await
            .context("failed to read manifest blob")?;
        let manifest_layout: ManifestLayoutView = serde_json::from_slice(manifest_bytes.as_slice())
            .context("failed to deserialize oci manifest")?;

        // Extract each layer into the target directory
        trace!(from = %digest_uri, "Extracting image layers");
        for layer in manifest_layout.layers {
            let digest = layer.digest.to_string().replace(':', "/");
            let layer_blob = File::open(self.archive_path().join(format!("blobs/{digest}")))
                .context("failed to read layer of oci image")?;
            let mut layer_archive = TarArchive::new(layer_blob);
            layer_archive
                .unpack(path)
                .context("failed to unpack layer to disk")?;
        }
        write(&digest_file, self.digest.as_str())
            .await
            .context(format!(
                "failed to record digest to {}",
                digest_file.display()
            ))?;

        Ok(())
    }
}
