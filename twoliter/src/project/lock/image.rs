use super::archive::OCIArchive;
use super::views::ManifestListView;
use crate::common::fs::create_dir_all;
use crate::compatibility::SUPPORTED_KIT_METADATA_VERSION;
use crate::project::{Image, ProjectImage, ValidIdentifier, VendedArtifact};
use anyhow::{bail, Context, Result};
use base64::Engine;
use futures::{pin_mut, stream, StreamExt, TryStreamExt};
use log::trace;
use oci_cli_wrapper::{ConfigView, DockerArchitecture, ImageTool};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::fmt::{Debug, Display, Formatter};
use std::path::Path;
use tracing::{debug, error, info, instrument};

/// The OCI config label prefix to which the supported kit metadata version is appended.
///
/// Kit metadata is embedded in the OCI image under this label.
const KIT_METADATA_LABEL_PREFIX: &str = "dev.bottlerocket.kit.";

pub fn supported_kit_metadata_label() -> String {
    format!("{KIT_METADATA_LABEL_PREFIX}{SUPPORTED_KIT_METADATA_VERSION}")
}

/// Represents a locked dependency on an image
#[derive(Debug, Clone, Eq, Ord, PartialOrd, Serialize, Deserialize)]
pub(crate) struct LockedImage {
    /// The name of the dependency
    pub name: ValidIdentifier,
    /// The version of the dependency
    pub version: Version,
    /// The vendor this dependency came from
    pub vendor: ValidIdentifier,
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

impl VendedArtifact for LockedImage {
    fn artifact_name(&self) -> &ValidIdentifier {
        &self.name
    }

    fn vendor_name(&self) -> &ValidIdentifier {
        &self.vendor
    }

    fn version(&self) -> &Version {
        &self.version
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImageMetadata {
    /// The name of the kit
    #[expect(dead_code)]
    pub name: String,
    /// The version of the kit
    #[expect(dead_code)]
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

        // Get the image configuration
        let config = image_tool.get_config(image_uri).await?;

        // Extract the kit metadata from the config
        let kit_metadata = EncodedKitMetadata(Self::extract_encoded_kit_metadata(&config)?);

        tracing::trace!(
            image_uri,
            image_config = ?config,
            ?kit_metadata,
            "Kit metadata retrieved from image config"
        );

        Ok(kit_metadata)
    }

    fn extract_encoded_kit_metadata(oci_config: &ConfigView) -> Result<String> {
        // Try to get the metadata from the supported label
        let supported_label = supported_kit_metadata_label();
        let encoded_metadata = oci_config.labels.get(&supported_label);

        match encoded_metadata {
            Some(encoded_metadata) => Ok(encoded_metadata.to_owned()),
            None => {
                // Check if there's any kit metadata label with a different version
                if let Some(kit_label) = oci_config
                    .labels
                    .keys()
                    .find(|label| label.starts_with(KIT_METADATA_LABEL_PREFIX))
                {
                    // Extract the version from the label
                    let kit_version = kit_label.trim_start_matches(KIT_METADATA_LABEL_PREFIX);
                    let meta_relation =
                        Self::compare_version_strs(kit_version, SUPPORTED_KIT_METADATA_VERSION);

                    bail!(
                        "kit appears to be built with metadata version '{kit_version}', possibly by \
                        {meta_relation} version of twoliter with unsupported incompatibilities. \
                        This version of twoliter supports metadata version \
                        '{SUPPORTED_KIT_METADATA_VERSION}'."
                    )
                } else {
                    bail!("no metadata stored on image, this image appears not to be a kit")
                }
            }
        }
    }

    /// Compare's kit metadata versions in english. Intended to be used in error messages.
    fn compare_version_strs(lhs: &str, rhs: &str) -> &'static str {
        let lhs: Result<u64, _> = lhs.trim_start_matches('v').parse();
        let rhs = rhs.trim_start_matches('v').parse();

        match (lhs, rhs) {
            (Ok(lhs), Ok(rhs)) => {
                if lhs < rhs {
                    "an older"
                } else {
                    "a newer"
                }
            }
            _ => "a different",
        }
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

#[derive(Debug)]
pub struct ImageResolver {
    image: ProjectImage,
    skip_metadata_retrieval: bool,
}

impl ImageResolver {
    pub(crate) fn from_image(image: &ProjectImage) -> Result<Self> {
        Ok(Self {
            image: image.clone(),
            skip_metadata_retrieval: false,
        })
    }

    /// Skip metadata retrieval when resolving images.
    ///
    /// This is useful for SDKs, which don't store image metadata (no deps.)
    pub(crate) fn skip_metadata_retrieval(mut self) -> Self {
        self.skip_metadata_retrieval = true;
        self
    }

    #[instrument(
        level = "trace",
        fields(image = %self.image, uri = %self.image.project_image_uri())
    )]
    /// Calculate the digest of the locked image
    async fn calculate_digest(&self, image_tool: &ImageTool) -> Result<String> {
        let image_uri = self.image.project_image_uri();
        let image_uri_str = image_uri.to_string();
        let manifest_bytes = image_tool.get_manifest(image_uri_str.as_str()).await?;
        let digest = sha2::Sha256::digest(manifest_bytes.as_slice());
        let digest = base64::engine::general_purpose::STANDARD.encode(digest.as_slice());
        debug!(
            "Calculated digest for locked image '{}': '{}'",
            image_uri, digest,
        );
        Ok(digest)
    }

    #[instrument(
        level = "trace",
        fields(image = %self.image, uri = %self.image.project_image_uri())
    )]
    async fn get_manifest(&self, image_tool: &ImageTool) -> Result<ManifestListView> {
        let uri = self.image.project_image_uri().to_string();
        debug!(image=%self.image, uri, "Fetching image manifest.");
        let manifest_bytes = image_tool.get_manifest(uri.as_str()).await?;
        serde_json::from_slice(manifest_bytes.as_slice())
            .context("failed to deserialize manifest list")
    }

    #[instrument(
        level = "trace",
        fields(image = %self.image, uri = %self.image.project_image_uri())
    )]
    pub(crate) async fn resolve(
        &self,
        image_tool: &ImageTool,
    ) -> Result<(LockedImage, Option<ImageMetadata>)> {
        let uri = self.image.project_image_uri();
        info!("Resolving dependency image '{}'", self.image);

        // Get the manifest list for the image
        let manifest_list = self.get_manifest(image_tool).await?;

        // Get the registry from the URI
        let registry = uri
            .registry
            .as_ref()
            .context("no registry found for image")?;

        // Create the locked image representation
        let locked_image = LockedImage {
            name: self.image.name().to_owned(),
            version: self.image.version().to_owned(),
            vendor: self.image.vendor_name().to_owned(),
            source: self.image.original_source_uri().to_string(),
            digest: self.calculate_digest(image_tool).await?,
        };

        // Skip metadata retrieval if requested (for SDKs)
        if self.skip_metadata_retrieval {
            return Ok((locked_image, None));
        }

        // Extract metadata from each manifest in the manifest list
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

        // Get the first metadata entry
        let canonical_metadata = embedded_kit_metadata
            .try_next()
            .await?
            .context(format!("could not find metadata for kit {}", uri))?;

        // Verify all manifests have the same metadata
        trace!("Checking that all manifests refer to the same kit");
        while let Some(kit_metadata) = embedded_kit_metadata.try_next().await? {
            if kit_metadata != canonical_metadata {
                error!(
                    ?canonical_metadata,
                    ?kit_metadata,
                    "Mismatched kit metadata in manifest list"
                );
                bail!("metadata does not match between images in manifest list");
            }
        }

        // Decode the metadata
        let metadata = canonical_metadata
            .try_into()
            .context("failed to decode and parse kit metadata")?;

        Ok((locked_image, Some(metadata)))
    }

    #[instrument(
        level = "trace",
        fields(uri = %self.image.project_image_uri(), path = %path.as_ref().display())
    )]
    pub(crate) async fn extract<P>(&self, image_tool: &ImageTool, path: P, arch: &str) -> Result<()>
    where
        P: AsRef<Path>,
    {
        let image_name = self.image.name();
        let vendor_name = self.image.vendor_name();
        let uri = self.image.project_image_uri();

        info!(
            "Extracting kit '{}-{}@{}' to '{}'",
            image_name,
            self.image.version(),
            vendor_name,
            path.as_ref().display()
        );

        // Create target paths
        let target_path = path
            .as_ref()
            .join(format!("{}/{}/{arch}", vendor_name, image_name));
        let cache_path = path.as_ref().join("cache");

        create_dir_all(&target_path).await?;
        create_dir_all(&cache_path).await?;

        // Get the manifest list for the image
        let manifest_list = self.get_manifest(image_tool).await?;

        // Find the manifest for the requested architecture
        let docker_arch = DockerArchitecture::try_from(arch)?;
        let manifest = manifest_list
            .manifests
            .iter()
            .find(|x| {
                x.platform
                    .as_ref()
                    .is_some_and(|p| p.architecture == docker_arch)
            })
            .cloned()
            .context(format!(
                "could not find image for architecture '{}' at {}",
                docker_arch, uri
            ))?;

        // Get the registry from the URI
        let registry = uri.registry.context("failed to resolve image registry")?;

        // Create an OCI archive for the image
        let oci_archive = OCIArchive::new(&registry, &uri.repo, &manifest.digest, &cache_path)?;

        // Pull the image if needed
        oci_archive.pull_image(image_tool).await?;

        // Unpack the image layers
        oci_archive.unpack_layers(&target_path).await?;

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::collections::HashMap;

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

    #[test]
    fn test_extract_encoded_kit_metadata_fails_no_label() {
        EncodedKitMetadata::extract_encoded_kit_metadata(&ConfigView {
            labels: HashMap::from([("foo".to_string(), "bar".to_string())]),
        })
        .expect_err("no label");
    }

    #[test]
    fn test_extract_encoded_kit_metadata_fails_older_metadata() {
        let err = EncodedKitMetadata::extract_encoded_kit_metadata(&ConfigView {
            labels: HashMap::from([(format!("{KIT_METADATA_LABEL_PREFIX}v0"), "bar".to_string())]),
        })
        .expect_err("too old")
        .to_string();

        assert!(err.contains("older") && err.contains("incompatibilities"));
    }

    #[test]
    fn test_extract_encoded_kit_metadata_fails_newer_metadata() {
        let err = EncodedKitMetadata::extract_encoded_kit_metadata(&ConfigView {
            labels: HashMap::from([(
                format!("{KIT_METADATA_LABEL_PREFIX}v9999"),
                "bar".to_string(),
            )]),
        })
        .expect_err("too new")
        .to_string();

        assert!(err.contains("newer") && err.contains("incompatibilities"));
    }

    #[test]
    fn test_extract_encoded_kit_metadata_fails_metadata_ver_unparseable() {
        let err = EncodedKitMetadata::extract_encoded_kit_metadata(&ConfigView {
            labels: HashMap::from([(
                format!("{KIT_METADATA_LABEL_PREFIX}notaversion"),
                "foo".to_string(),
            )]),
        })
        .expect_err("not a version")
        .to_string();

        assert!(err.contains("different") && err.contains("incompatibilities"));
    }

    #[test]
    fn test_extract_encoded_kit_metadata_succeeds_current_metadata_version() {
        assert_eq!(
            EncodedKitMetadata::extract_encoded_kit_metadata(&ConfigView {
                labels: HashMap::from([(
                    format!("{KIT_METADATA_LABEL_PREFIX}{SUPPORTED_KIT_METADATA_VERSION}"),
                    "bar".to_string(),
                )]),
            })
            .unwrap(),
            "bar".to_string()
        );
    }
}
