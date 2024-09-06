use super::archive::OCIArchive;
use super::views::ManifestListView;
use super::Override;
use crate::common::fs::create_dir_all;
use crate::docker::ImageUri;
use crate::project::{Image, Project, ValidIdentifier};
use anyhow::{bail, Context, Result};
use base64::Engine;
use futures::{pin_mut, stream, StreamExt, TryStreamExt};
use log::trace;
use oci_cli_wrapper::{DockerArchitecture, ImageTool};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::fmt::{Debug, Display, Formatter};
use std::path::Path;
use tracing::{debug, error, info, instrument};

/// Represents a locked dependency on an image
#[derive(Debug, Clone, Eq, Ord, PartialOrd, Serialize, Deserialize)]
pub(crate) struct LockedImage {
    /// The name of the dependency
    pub name: String,
    /// The version of the dependency
    pub version: Version,
    /// The vendor this dependency came from
    pub vendor: String,
    /// The resolved image uri of the dependency
    pub source: String,
    /// The digest of the image
    pub digest: String,
}

impl PartialEq for LockedImage {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source && self.digest == other.digest
    }
}

impl Display for LockedImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!(
            "{}-{}@{} ({})",
            self.name, self.version, self.vendor, self.source,
        ))
    }
}

#[derive(Deserialize, Debug, Clone)]
pub(crate) struct ImageMetadata {
    /// The name of the kit
    #[allow(dead_code)]
    pub name: String,
    /// The version of the kit
    #[allow(dead_code)]
    pub version: Version,
    /// The required sdk of the kit,
    pub sdk: Image,
    /// Any dependent kits
    #[serde(rename = "kit")]
    pub kits: Vec<Image>,
}

impl TryFrom<EncodedKitMetadata> for ImageMetadata {
    type Error = anyhow::Error;

    fn try_from(value: EncodedKitMetadata) -> Result<Self, Self::Error> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(value.0)
            .context("failed to decode kit metadata as base64")?;
        serde_json::from_slice(bytes.as_slice()).context("failed to parse kit metadata json")
    }
}

/// Encoded kit metadata, which is embedded in a label of the OCI image config.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct EncodedKitMetadata(String);

impl EncodedKitMetadata {
    #[instrument(level = "trace")]
    async fn try_from_image(image_uri: &str, image_tool: &ImageTool) -> Result<Self> {
        tracing::trace!(image_uri, "Extracting kit metadata from OCI image config");
        let config = image_tool.get_config(image_uri).await?;
        let kit_metadata = EncodedKitMetadata(
            config
                .labels
                .get("dev.bottlerocket.kit.v1")
                .context("no metadata stored on image, this image appears to not be a kit")?
                .to_owned(),
        );

        tracing::trace!(
            image_uri,
            image_config = ?config,
            ?kit_metadata,
            "Kit metadata retrieved from image config"
        );

        Ok(kit_metadata)
    }

    /// Infallible method to provide debugging insights into encoded `ImageMetadata`
    ///
    /// Shows a `Debug` view of the encoded `ImageMetadata` if possible, otherwise shows
    /// the encoded form.
    fn try_debug_image_metadata(&self) -> String {
        self.debug_image_metadata().unwrap_or_else(|| {
            format!("<ImageMetadata(encoded) [{}]>", self.0.replace("\n", "\\n"))
        })
    }

    fn debug_image_metadata(&self) -> Option<String> {
        base64::engine::general_purpose::STANDARD
            .decode(&self.0)
            .ok()
            .and_then(|bytes| serde_json::from_slice(bytes.as_slice()).ok())
            .map(|metadata: ImageMetadata| format!("<ImageMetadata(decoded) [{:?}]>", metadata))
    }
}

impl Debug for EncodedKitMetadata {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.try_debug_image_metadata())
    }
}

pub trait ImageResolverImpl: Debug {
    fn name(&self) -> String;
    fn version(&self) -> Result<Version>;
    fn vendor(&self) -> String;
    fn source(&self) -> String;
    fn uri(&self) -> ImageUri;
}

#[derive(Debug)]
pub struct VerbatimImage {
    uri: ImageUri,
    vendor: String,
}

impl ImageResolverImpl for VerbatimImage {
    fn name(&self) -> String {
        self.uri.repo.clone()
    }

    fn version(&self) -> Result<Version> {
        Version::parse(self.uri.tag.trim_start_matches('v')).context("invalid version tag")
    }

    fn source(&self) -> String {
        self.uri.to_string()
    }

    fn vendor(&self) -> String {
        self.vendor.clone()
    }

    fn uri(&self) -> ImageUri {
        self.uri.clone()
    }
}

#[derive(Debug)]
pub struct OverriddenImage {
    base_uri: ImageUri,
    vendor: String,
    override_: Override,
}

impl ImageResolverImpl for OverriddenImage {
    fn name(&self) -> String {
        self.base_uri.repo.clone()
    }

    fn version(&self) -> Result<Version> {
        Version::parse(self.base_uri.tag.trim_start_matches('v')).context("invalid version tag")
    }

    fn source(&self) -> String {
        self.base_uri.to_string()
    }

    fn vendor(&self) -> String {
        self.vendor.clone()
    }

    fn uri(&self) -> ImageUri {
        ImageUri {
            registry: self
                .override_
                .registry
                .clone()
                .or(self.base_uri.registry.clone()),
            repo: self
                .override_
                .name
                .clone()
                .unwrap_or(self.base_uri.repo.clone()),
            tag: self.base_uri.tag.clone(),
        }
    }
}

#[derive(Debug)]
pub struct ImageResolver {
    image_resolver_impl: Box<dyn ImageResolverImpl>,
}

impl ImageResolver {
    pub(crate) fn from_image(image: &Image, project: &Project) -> Result<Self> {
        let vendor_name = image.vendor.0.as_str();
        let vendor = project.vendor().get(&image.vendor).context(format!(
            "vendor '{}' is not specified in Twoliter.toml",
            image.vendor
        ))?;
        let override_ = project
            .overrides()
            .get(&image.vendor.to_string())
            .and_then(|x| x.get(&image.name.to_string()));
        if let Some(override_) = override_.as_ref() {
            debug!(
                ?override_,
                "Found override for image '{}' with vendor '{}'", image.name, image.vendor
            );
        }
        Ok(Self {
            image_resolver_impl: if let Some(override_) = override_ {
                Box::new(OverriddenImage {
                    base_uri: ImageUri {
                        registry: Some(vendor.registry.clone()),
                        repo: image.name.to_string(),
                        tag: format!("v{}", image.version),
                    },
                    vendor: vendor_name.to_string(),
                    override_: override_.clone(),
                })
            } else {
                Box::new(VerbatimImage {
                    vendor: vendor_name.to_string(),
                    uri: ImageUri {
                        registry: Some(vendor.registry.clone()),
                        repo: image.name.to_string(),
                        tag: format!("v{}", image.version),
                    },
                })
            },
        })
    }

    pub(crate) fn from_locked_image(locked_image: &LockedImage, project: &Project) -> Result<Self> {
        let vendor_name = &locked_image.vendor;
        let vendor = project
            .vendor()
            .get(&ValidIdentifier(vendor_name.clone()))
            .context(format!(
                "failed to find vendor for kit with name '{}' and vendor '{}'",
                locked_image.name, locked_image.vendor
            ))?;
        let override_ = project
            .overrides()
            .get(&locked_image.vendor)
            .and_then(|x| x.get(&locked_image.name));
        if let Some(override_) = override_.as_ref() {
            debug!(
                ?override_,
                "Found override for image '{}' with vendor '{}'",
                locked_image.name,
                locked_image.vendor
            );
        }
        Ok(Self {
            image_resolver_impl: if let Some(override_) = override_ {
                Box::new(OverriddenImage {
                    base_uri: ImageUri {
                        registry: Some(vendor.registry.clone()),
                        repo: locked_image.name.to_string(),
                        tag: format!("v{}", locked_image.version),
                    },
                    vendor: vendor_name.to_string(),
                    override_: override_.clone(),
                })
            } else {
                Box::new(VerbatimImage {
                    vendor: vendor_name.to_string(),
                    uri: ImageUri {
                        registry: Some(vendor.registry.clone()),
                        repo: locked_image.name.to_string(),
                        tag: format!("v{}", locked_image.version),
                    },
                })
            },
        })
    }

    /// Calculate the digest of the locked image
    async fn calculate_digest(&self, image_tool: &ImageTool) -> Result<String> {
        let image_uri = self.image_resolver_impl.uri();
        let image_uri_str = image_uri.to_string();
        let manifest_bytes = image_tool.get_manifest(image_uri_str.as_str()).await?;
        let digest = sha2::Sha256::digest(manifest_bytes.as_slice());
        let digest = base64::engine::general_purpose::STANDARD.encode(digest.as_slice());
        trace!(
            "Calculated digest for locked image '{}': '{}'",
            image_uri,
            digest,
        );
        Ok(digest)
    }

    async fn get_manifest(&self, image_tool: &ImageTool) -> Result<ManifestListView> {
        let uri = self.image_resolver_impl.uri().to_string();
        let manifest_bytes = image_tool.get_manifest(uri.as_str()).await?;
        serde_json::from_slice(manifest_bytes.as_slice())
            .context("failed to deserialize manifest list")
    }

    pub(crate) async fn resolve(
        &self,
        image_tool: &ImageTool,
        skip_metadata: bool,
    ) -> Result<(LockedImage, Option<ImageMetadata>)> {
        // First get the manifest list
        let uri = self.image_resolver_impl.uri();
        let manifest_list = self.get_manifest(image_tool).await?;
        let registry = uri
            .registry
            .as_ref()
            .context("no registry found for image")?;

        let locked_image = LockedImage {
            name: self.image_resolver_impl.name(),
            version: self.image_resolver_impl.version()?,
            vendor: self.image_resolver_impl.vendor(),
            // The source is the image uri without the tag, which is the digest
            source: self.image_resolver_impl.source(),
            digest: self.calculate_digest(image_tool).await?,
        };

        if skip_metadata {
            return Ok((locked_image, None));
        }

        debug!("Extracting kit metadata from OCI image");
        let embedded_kit_metadata = stream::iter(manifest_list.manifests).then(|manifest| {
            let registry = registry.clone();
            let repo = uri.repo.clone();
            async move {
                let image_uri = format!("{registry}/{repo}@{}", manifest.digest);
                EncodedKitMetadata::try_from_image(&image_uri, image_tool).await
            }
        });
        pin_mut!(embedded_kit_metadata);

        let canonical_metadata = embedded_kit_metadata
            .try_next()
            .await?
            .context(format!("could not find metadata for kit {}", uri))?;

        trace!("Checking that all manifests refer to the same kit.");
        while let Some(kit_metadata) = embedded_kit_metadata.try_next().await? {
            if kit_metadata != canonical_metadata {
                error!(
                    ?canonical_metadata,
                    ?kit_metadata,
                    "Mismatched kit metadata in manifest list"
                );
                bail!("Metadata does not match between images in manifest list");
            }
        }
        let metadata = canonical_metadata
            .try_into()
            .context("Failed to decode and parse kit metadata")?;

        Ok((locked_image, Some(metadata)))
    }

    #[instrument(
        level = "trace",
        fields(uri = %self.image_resolver_impl.uri(), path = %path.as_ref().display())
    )]
    pub(crate) async fn extract<P>(&self, image_tool: &ImageTool, path: P, arch: &str) -> Result<()>
    where
        P: AsRef<Path>,
    {
        info!(
            "Extracting kit '{}' to '{}'",
            self.image_resolver_impl.name(),
            path.as_ref().display()
        );
        let target_path = path.as_ref().join(format!(
            "{}/{}/{arch}",
            self.image_resolver_impl.vendor(),
            self.image_resolver_impl.name()
        ));
        let cache_path = path.as_ref().join("cache");
        create_dir_all(&target_path).await?;
        create_dir_all(&cache_path).await?;

        // First get the manifest for the specific requested architecture
        let uri = self.image_resolver_impl.uri();
        let manifest_list = self.get_manifest(image_tool).await?;
        let docker_arch = DockerArchitecture::try_from(arch)?;
        let manifest = manifest_list
            .manifests
            .iter()
            .find(|x| x.platform.as_ref().unwrap().architecture == docker_arch)
            .cloned()
            .context(format!(
                "could not find image for architecture '{}' at {}",
                docker_arch, uri
            ))?;

        let registry = uri.registry.context("failed to resolve image registry")?;
        let oci_archive = OCIArchive::new(
            registry.as_str(),
            uri.repo.as_str(),
            manifest.digest.as_str(),
            &cache_path,
        )?;

        // Checks for the saved image locally, or else pulls and saves it
        oci_archive.pull_image(image_tool).await?;

        // Checks if this archive has already been extracted by checking a digest file
        // otherwise cleans up the path and unpacks the archive
        oci_archive.unpack_layers(&target_path).await?;

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn test_try_debug_image_metadata_succeeds() {
        // Given a valid encoded metadata string,
        // When we attempt to decode it for debugging,
        // Then the debug string is marked as having been decoded.
        let encoded = EncodedKitMetadata(
            "eyJraXQiOltdLCJuYW1lIjoiYm90dGxlcm9ja2V0LWNvcmUta2l0Iiwic2RrIjp7ImRpZ2VzdCI6ImlyY09EUl\
            d3ZmxjTTdzaisrMmszSk5RWkovb3ZDUVRpUlkrRFpvaGdrNlk9IiwibmFtZSI6InRoYXItYmUtYmV0YS1zZGsiL\
            CJzb3VyY2UiOiJwdWJsaWMuZWNyLmF3cy91MWczYzh6NC90aGFyLWJlLWJldGEtc2RrOnYwLjQzLjAiLCJ2ZW5k\
            b3IiOiJib3R0bGVyb2NrZXQtbmV3IiwidmVyc2lvbiI6IjAuNDMuMCJ9LCJ2ZXJzaW9uIjoiMi4wLjAifQo="
                .to_string()
        );
        assert!(encoded.debug_image_metadata().is_some());
    }

    #[test]
    fn test_try_debug_image_metadata_fails() {
        // Given an invalid encoded metadata string,
        // When we attempt to decode it for debugging,
        // Then the debug string is marked as remaining encoded.
        let junk_data = EncodedKitMetadata("abcdefghijklmnophello".to_string());
        assert!(junk_data.debug_image_metadata().is_none());
    }
}
