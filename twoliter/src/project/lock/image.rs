use super::archive::OCIArchive;
use super::views::ManifestListView;
use crate::common::fs::create_dir_all;
use crate::compatibility::SUPPORTED_KIT_METADATA_VERSION;
use crate::docker::ImageUri;
use crate::project::{Image, ProjectImage, ValidIdentifier, VendedArtifact};
use anyhow::{bail, ensure, Context, Result};
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
        let config = image_tool.get_config(image_uri).await?;
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
        let encoded_metadata = oci_config
            .labels
            .get(supported_kit_metadata_label().as_str());

        match encoded_metadata {
            Some(encoded_metadata) => Ok(encoded_metadata.to_owned()),
            None => {
                if let Some(kit_label) = oci_config
                    .labels
                    .keys()
                    .find(|label| label.starts_with(KIT_METADATA_LABEL_PREFIX))
                {
                    let kit_version = kit_label.trim_start_matches(KIT_METADATA_LABEL_PREFIX);
                    let meta_relation =
                        Self::compare_version_strs(kit_version, SUPPORTED_KIT_METADATA_VERSION);

                    bail!(
                        "kit appears to be built with metadata version '{kit_version}', possibly by \
                        {meta_relation} version of twoliter with unsupported incompatibilities. \
                        This version of twoliter supports metadata version \
                        '{SUPPORTED_KIT_METADATA_VERSION}'.",
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
            .map(|metadata: ImageMetadata| format!("<ImageMetadata(decoded) [{metadata:?}]>"))
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
    /// Memoized manifest list, fetched at most once per resolver.
    manifest_cache: tokio::sync::OnceCell<(ManifestListView, Vec<u8>)>,
}

impl ImageResolver {
    pub(crate) fn from_image(image: &ProjectImage) -> Result<Self> {
        Ok(Self {
            image: image.clone(),
            skip_metadata_retrieval: false,
            manifest_cache: tokio::sync::OnceCell::new(),
        })
    }

    /// Skip metadata retrieval when resolving images.
    ///
    /// This is useful for SDKs, which don't store image metadata (no deps.)
    pub(crate) fn skip_metadata_retrieval(mut self) -> Self {
        self.skip_metadata_retrieval = true;
        self
    }

    /// Encodes a manifest's SHA-256 as the base64 form stored in `Twoliter.lock`.
    fn manifest_digest_b64(manifest_bytes: &[u8]) -> String {
        let raw = sha2::Sha256::digest(manifest_bytes);
        base64::engine::general_purpose::STANDARD.encode(raw)
    }

    /// Calculates the digest of the locked image by hashing the cached manifest bytes.
    #[instrument(
        level = "trace",
        fields(image = %self.image, uri = %self.image.project_image_uri())
    )]
    async fn calculate_digest(&self, image_tool: &ImageTool) -> Result<String> {
        let image_uri = self.image.project_image_uri();
        let (_list, manifest_bytes) = self.get_manifest_with_bytes(image_tool).await?;
        let digest = Self::manifest_digest_b64(manifest_bytes.as_slice());
        debug!("Calculated digest for locked image '{image_uri}': '{digest}'");
        Ok(digest)
    }

    #[instrument(
        level = "trace",
        fields(image = %self.image, uri = %self.image.project_image_uri())
    )]
    async fn get_manifest(&self, image_tool: &ImageTool) -> Result<ManifestListView> {
        let (list, _bytes) = self.get_manifest_with_bytes(image_tool).await?;
        Ok(list.clone())
    }

    /// Fetches the manifest list once per resolver and returns cached bytes + parse.
    ///
    /// Callers that need to verify the bytes against a lockfile-recorded digest (or
    /// re-parse without a second HTTP round-trip) reuse this. The result is memoized in
    /// `self.manifest_cache`, so the SDK manifest is fetched at most once per command
    /// even though `resolve`, `calculate_digest`, `resolve_arch_digest`, and `extract`
    /// each need it.
    #[instrument(
        level = "trace",
        fields(image = %self.image, uri = %self.image.project_image_uri())
    )]
    async fn get_manifest_with_bytes(
        &self,
        image_tool: &ImageTool,
    ) -> Result<&(ManifestListView, Vec<u8>)> {
        self.manifest_cache
            .get_or_try_init(|| async {
                let uri = self.image.project_image_uri().to_string();
                debug!(image=%self.image, uri, "Fetching image manifest.");
                let manifest_bytes = image_tool.get_manifest(uri.as_str()).await?;
                let list: ManifestListView = serde_json::from_slice(manifest_bytes.as_slice())
                    .context("failed to deserialize manifest list")?;
                anyhow::Ok((list, manifest_bytes))
            })
            .await
    }

    #[instrument(
        level = "trace",
        fields(image = %self.image, uri = %self.image.project_image_uri())
    )]
    pub(crate) async fn resolve(
        &self,
        image_tool: &ImageTool,
    ) -> Result<(LockedImage, Option<ImageMetadata>)> {
        // First get the manifest list
        let uri = self.image.project_image_uri();
        info!("Resolving dependency image dependency '{}'.", self.image);

        let manifest_list = self.get_manifest(image_tool).await?;
        let registry = uri
            .registry
            .as_ref()
            .context("no registry found for image")?;

        let locked_image = LockedImage {
            name: self.image.name().to_owned(),
            version: self.image.version().to_owned(),
            vendor: self.image.vendor_name().to_owned(),
            // The source is the image uri without the tag, which is the digest
            source: self.image.original_source_uri().to_string(),
            digest: self.calculate_digest(image_tool).await?,
        };

        if self.skip_metadata_retrieval {
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
            .context(format!("could not find metadata for kit {uri}"))?;

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

    /// Returns the per-arch image manifest digest (`sha256:<hex>`) after verifying the
    /// fetched manifest-list bytes against `expected_lock_digest` from `Twoliter.lock`.
    #[instrument(
        level = "trace",
        fields(uri = %self.image.project_image_uri(), arch)
    )]
    pub(crate) async fn resolve_arch_digest(
        &self,
        image_tool: &ImageTool,
        arch: &str,
        expected_lock_digest: &str,
    ) -> Result<String> {
        let uri = self.image.project_image_uri();
        let (manifest_list, manifest_bytes) = self.get_manifest_with_bytes(image_tool).await?;

        let computed = Self::manifest_digest_b64(manifest_bytes.as_slice());
        if computed != expected_lock_digest {
            error!(
                %uri,
                expected = %expected_lock_digest,
                actual = %computed,
                "Manifest list digest does not match Twoliter.lock"
            );
            bail!(
                "manifest list digest for {uri} does not match Twoliter.lock \
                 (expected '{expected_lock_digest}', got '{computed}'); \
                 the registry served different bytes than were authorized in the lockfile — \
                 refusing to build a pinned URI from unauthorized content"
            );
        }

        let docker_arch = DockerArchitecture::try_from(arch)?;
        let manifest = manifest_list
            .manifests
            .iter()
            .find(|m| {
                m.platform
                    .as_ref()
                    .map(|p| p.architecture == docker_arch)
                    .unwrap_or(false)
            })
            .cloned()
            .with_context(|| {
                format!("could not find image for architecture '{docker_arch}' at {uri}")
            })?;

        validate_oci_digest(&manifest.digest).with_context(|| {
            format!(
                "manifest for arch '{docker_arch}' at {uri} has malformed digest '{}'",
                manifest.digest
            )
        })?;

        Ok(manifest.digest)
    }

    #[instrument(
        level = "trace",
        fields(uri = %self.image.project_image_uri(), path = %path.as_ref().display())
    )]
    pub(crate) async fn extract<P>(
        &self,
        image_tool: &ImageTool,
        path: P,
        arch: &str,
        expected_lock_digest: &str,
    ) -> Result<()>
    where
        P: AsRef<Path>,
    {
        info!(
            "Extracting kit '{}' to '{}'",
            self.image.name(),
            path.as_ref().display()
        );
        let target_path = path.as_ref().join(format!(
            "{}/{}/{arch}",
            self.image.vendor_name(),
            self.image.name()
        ));
        let cache_path = path.as_ref().join("cache");
        create_dir_all(&target_path).await?;
        create_dir_all(&cache_path).await?;

        let uri = self.image.project_image_uri();
        let arch_digest = self
            .resolve_arch_digest(image_tool, arch, expected_lock_digest)
            .await?;

        let registry = uri.registry.context("failed to resolve image registry")?;
        let oci_archive = OCIArchive::new(
            registry.as_str(),
            uri.repo.as_str(),
            arch_digest.as_str(),
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

/// Builds a `registry/repo@sha256:<hex>` reference pinned to the per-arch image digest,
/// after verifying the manifest list against `expected_lock_digest` from `Twoliter.lock`.
pub(crate) async fn build_pinned_uri(
    image: &ProjectImage,
    image_tool: &ImageTool,
    arch: &str,
    expected_lock_digest: &str,
) -> Result<String> {
    let uri = image.project_image_uri();
    let base = uri_without_tag(&uri)?;
    let arch_digest = ImageResolver::from_image(image)?
        .resolve_arch_digest(image_tool, arch, expected_lock_digest)
        .await?;
    Ok(format!("{base}@{arch_digest}"))
}

fn uri_without_tag(uri: &ImageUri) -> Result<String> {
    let registry = uri.registry.as_ref().with_context(|| {
        format!(
            "cannot build a digest-pinned reference for '{}': no registry recorded — \
             refusing to fall back to Docker Hub",
            uri.repo
        )
    })?;
    Ok(format!("{}/{}", registry, uri.repo))
}

/// Enforces canonical OCI digest form: `sha256:` + 64 lowercase-hex characters.
pub(crate) fn validate_oci_digest(digest: &str) -> Result<()> {
    const PREFIX: &str = "sha256:";
    const HEX_LEN: usize = 64;

    let hex = digest
        .strip_prefix(PREFIX)
        .with_context(|| format!("digest must start with '{PREFIX}', got '{digest}'"))?;
    ensure!(
        hex.len() == HEX_LEN,
        "digest hex portion must be {HEX_LEN} characters, got {} ('{digest}')",
        hex.len()
    );
    ensure!(
        hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
        "digest hex portion must be lowercase hexadecimal ('{digest}')"
    );
    Ok(())
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

    fn hex64(byte: u8) -> String {
        std::iter::repeat_n(char::from(byte), 64).collect()
    }

    #[test]
    fn validate_oci_digest_accepts_canonical_form() {
        validate_oci_digest(&format!("sha256:{}", hex64(b'a'))).expect("all-a hex");
        validate_oci_digest(&format!("sha256:{}", hex64(b'0'))).expect("all-0 hex");
        validate_oci_digest(&format!(
            "sha256:{}",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ))
        .expect("mixed hex");
    }

    #[test]
    fn validate_oci_digest_rejects_empty() {
        let err = validate_oci_digest("").unwrap_err().to_string();
        assert!(err.contains("must start with 'sha256:'"), "got: {err}");
    }

    #[test]
    fn validate_oci_digest_rejects_missing_prefix() {
        let err = validate_oci_digest(&hex64(b'a')).unwrap_err().to_string();
        assert!(err.contains("must start with 'sha256:'"), "got: {err}");
    }

    #[test]
    fn validate_oci_digest_rejects_wrong_algorithm() {
        let err = validate_oci_digest(&format!("sha512:{}", hex64(b'a')))
            .unwrap_err()
            .to_string();
        assert!(err.contains("must start with 'sha256:'"), "got: {err}");
    }

    #[test]
    fn validate_oci_digest_rejects_short_hex() {
        let err = validate_oci_digest("sha256:abc").unwrap_err().to_string();
        assert!(err.contains("64 characters"), "got: {err}");
    }

    #[test]
    fn validate_oci_digest_rejects_long_hex() {
        let err = validate_oci_digest(&format!("sha256:{}a", hex64(b'a')))
            .unwrap_err()
            .to_string();
        assert!(err.contains("64 characters"), "got: {err}");
    }

    #[test]
    fn validate_oci_digest_rejects_uppercase_hex() {
        let err = validate_oci_digest(&format!("sha256:{}", hex64(b'A')))
            .unwrap_err()
            .to_string();
        assert!(err.contains("lowercase"), "got: {err}");
    }

    #[test]
    fn validate_oci_digest_rejects_non_hex() {
        let err = validate_oci_digest(&format!("sha256:{}", hex64(b'z')))
            .unwrap_err()
            .to_string();
        assert!(err.contains("lowercase"), "got: {err}");
    }

    #[test]
    fn validate_oci_digest_rejects_shell_metacharacters() {
        // The critical property: a digest that would inject arguments into a `docker` or
        // `krane` command line if concatenated unquoted must not slip through.
        for injection in [
            "sha256:aaa' ; docker run --privileged evil #",
            "sha256:aaa aaa",
            "sha256:$(pwn)aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "sha256:\naaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ] {
            assert!(
                validate_oci_digest(injection).is_err(),
                "digest with injection payload should be rejected: {injection:?}"
            );
        }
    }

    #[test]
    fn uri_without_tag_requires_registry() {
        let with_registry = ImageUri {
            registry: Some("example.com".to_string()),
            repo: "org/repo".to_string(),
            tag: "v1.0.0".to_string(),
        };
        assert_eq!(
            uri_without_tag(&with_registry).unwrap(),
            "example.com/org/repo"
        );

        let without_registry = ImageUri {
            registry: None,
            repo: "org/repo".to_string(),
            tag: "v1.0.0".to_string(),
        };
        let err = uri_without_tag(&without_registry).unwrap_err().to_string();
        assert!(err.contains("no registry recorded"), "got: {err}");
        assert!(err.contains("Docker Hub"), "got: {err}");
    }

    #[test]
    fn manifest_digest_b64_matches_known_vector() {
        // sha256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        // base64(sha256("")) = 47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=
        assert_eq!(
            ImageResolver::manifest_digest_b64(b""),
            "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
        );
    }
}
