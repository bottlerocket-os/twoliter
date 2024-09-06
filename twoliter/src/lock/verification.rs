//! This module contains utilities for marking that certain Twoliter artifacts have been resolved
//! and verified against a project's Lockfile.
//!
//! An overview of the contained abstractions:
//! * The [`LockfileVerifier`] trait allows a type to announce that it has resolved and verified
//!   a set of artifacts.
//! * Verified artifacts are identified via a [`VerifyTag`].
//! * Each [`VerifyTag`] has a [`VerificationManifest`] containing a list of the verified artifacts
//!   of that tag type.
//! * The [`VerificationTagger`] writes files containing [`VerifyTag`]s that are produced by
//!   [`LockfileVerifier`]s.
use super::image::LockedImage;
use super::{Lock, LockedSDK};
use anyhow::{Context, Result};
use olpc_cjson::CanonicalFormatter as CanonicalJsonFormatter;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt::Debug;
use std::path::Path;
use strum::{EnumIter, IntoEnumIterator};
use tracing::{debug, instrument};

const SDK_VERIFIED_MARKER_FILE: &str = ".sdk-verified";
const KITS_VERIFIED_MARKER_FILE: &str = ".kits-verified";

/// A tag indicating that Twoliter artifacts have been resolved and verified against the lockfile
#[derive(Debug, PartialEq, Eq, Ord, PartialOrd, EnumIter)]
pub(crate) enum VerifyTag {
    Sdk(VerificationManifest),
    Kits(VerificationManifest),
}

impl VerifyTag {
    /// Returns the marker file marking an artifact type that has been verified against the lock
    pub(crate) fn marker_file_name(&self) -> &'static str {
        match self {
            VerifyTag::Sdk(_) => SDK_VERIFIED_MARKER_FILE,
            VerifyTag::Kits(_) => KITS_VERIFIED_MARKER_FILE,
        }
    }

    pub(crate) fn manifest(&self) -> &VerificationManifest {
        match self {
            VerifyTag::Sdk(manifest) => manifest,
            VerifyTag::Kits(manifest) => manifest,
        }
    }
}

/// A manifest containing the list of elements that were verified by a `LockfileVerifier`
#[derive(Debug, Clone, Default, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct VerificationManifest {
    verified_images: BTreeSet<String>,
}

impl VerificationManifest {
    fn as_canonical_json(&self) -> Result<Vec<u8>> {
        let mut manifest = Vec::new();
        let mut ser =
            serde_json::Serializer::with_formatter(&mut manifest, CanonicalJsonFormatter::new());
        self.serialize(&mut ser)
            .context("failed to serialize external kit metadata")?;
        Ok(manifest)
    }
}

impl From<&LockedImage> for VerificationManifest {
    fn from(image: &LockedImage) -> Self {
        [image].as_slice().into()
    }
}

impl From<&[&LockedImage]> for VerificationManifest {
    fn from(images: &[&LockedImage]) -> Self {
        Self {
            verified_images: images.iter().map(ToString::to_string).collect(),
        }
    }
}

/// A `LockfileVerifier` can return a set of `VerifyTag` structs, claiming that those artifacts
/// have been resolved and verified against the lockfile.
pub(crate) trait LockfileVerifier {
    fn verified(&self) -> BTreeSet<VerifyTag>;
}

impl LockfileVerifier for LockedSDK {
    fn verified(&self) -> BTreeSet<VerifyTag> {
        [VerifyTag::Sdk((&self.0).into())].into()
    }
}

impl LockfileVerifier for Lock {
    fn verified(&self) -> BTreeSet<VerifyTag> {
        [
            VerifyTag::Sdk((&self.sdk).into()),
            VerifyTag::Kits(self.kit.iter().collect::<Vec<_>>().as_slice().into()),
        ]
        .into()
    }
}

/// Writes marker files indicating which artifacts have been resolved and verified against the lock
#[derive(Debug)]
pub(crate) struct VerificationTagger {
    tags: BTreeSet<VerifyTag>,
}

impl<V: LockfileVerifier> From<&V> for VerificationTagger {
    fn from(resolver: &V) -> Self {
        Self {
            tags: resolver.verified(),
        }
    }
}

impl VerificationTagger {
    /// Creates marker files for artifacts that have been verified against the lockfile
    #[instrument(level = "trace", skip(external_kits_dir))]
    pub(crate) async fn write_tags<P: AsRef<Path>>(&self, external_kits_dir: P) -> Result<()> {
        let external_kits_dir = external_kits_dir.as_ref();
        Self::cleanup_existing_tags(&external_kits_dir).await?;

        debug!("Writing tag files for verified artifacts");
        tokio::fs::create_dir_all(&external_kits_dir)
            .await
            .context(format!(
                "failed to create external-kits directory at '{}'",
                external_kits_dir.display()
            ))?;

        for tag in self.tags.iter() {
            let flag_file = external_kits_dir.join(tag.marker_file_name());
            debug!(
                "Writing tag file for verified artifacts: '{}'",
                flag_file.display()
            );
            tokio::fs::write(&flag_file, tag.manifest().as_canonical_json()?)
                .await
                .context(format!(
                    "failed to write verification tag file: '{}'",
                    flag_file.display()
                ))?;
        }
        Ok(())
    }

    /// Deletes any existing verifier marker files in the kits directory
    #[instrument(level = "trace", skip(external_kits_dir))]
    pub(crate) async fn cleanup_existing_tags<P: AsRef<Path>>(external_kits_dir: P) -> Result<()> {
        let external_kits_dir = external_kits_dir.as_ref();

        debug!("Cleaning up any existing tag files for resolved artifacts",);
        for resolve_tag in VerifyTag::iter() {
            let flag_file = external_kits_dir.join(resolve_tag.marker_file_name());
            if flag_file.exists() {
                debug!(
                    "Removing existing verification tag file '{}'",
                    flag_file.display()
                );
                tokio::fs::remove_file(&flag_file).await.context(format!(
                    "failed to remove existing verification tag file: {}",
                    flag_file.display()
                ))?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    struct SDKResolver;

    impl LockfileVerifier for SDKResolver {
        fn verified(&self) -> BTreeSet<VerifyTag> {
            [VerifyTag::Sdk(VerificationManifest {
                verified_images: ["image1".into(), "image2".into()].into(),
            })]
            .into()
        }
    }

    struct KitResolver;

    impl LockfileVerifier for KitResolver {
        fn verified(&self) -> BTreeSet<VerifyTag> {
            [
                VerifyTag::Sdk(VerificationManifest {
                    verified_images: ["image1".into(), "image2".into()].into(),
                }),
                VerifyTag::Kits(VerificationManifest {
                    verified_images: ["kit1".into(), "kit2".into()].into(),
                }),
            ]
            .into()
        }
    }

    #[tokio::test]
    async fn test_cleanup_existing_tags() {
        let kits_dir = tempfile::tempdir().unwrap();
        let flag_file = kits_dir.path().join(SDK_VERIFIED_MARKER_FILE);
        tokio::fs::write(&flag_file, "test").await.unwrap();

        VerificationTagger::cleanup_existing_tags(&kits_dir.path())
            .await
            .unwrap();
        assert!(!flag_file.exists());
    }

    #[tokio::test]
    async fn test_write_sdk_tags() {
        let kits_dir = tempfile::tempdir().unwrap();
        let tagger = VerificationTagger::from(&SDKResolver);
        tagger.write_tags(&kits_dir.path()).await.unwrap();

        let flag_file = kits_dir.path().join(SDK_VERIFIED_MARKER_FILE);
        assert!(flag_file.exists());
        let contents = tokio::fs::read_to_string(&flag_file).await.unwrap();
        assert_eq!(contents, r#"["image1","image2"]"#);
    }

    #[tokio::test]
    async fn test_write_kit_tags() {
        let kits_dir = tempfile::tempdir().unwrap();
        let tagger = VerificationTagger::from(&KitResolver);
        tagger.write_tags(&kits_dir.path()).await.unwrap();

        let sdk_flag_file = kits_dir.path().join(SDK_VERIFIED_MARKER_FILE);
        assert!(sdk_flag_file.exists());
        let sdk_contents = tokio::fs::read_to_string(&sdk_flag_file).await.unwrap();
        assert_eq!(sdk_contents, r#"["image1","image2"]"#);

        let kit_flag_file = kits_dir.path().join(KITS_VERIFIED_MARKER_FILE);
        assert!(kit_flag_file.exists());
        let kit_contents = tokio::fs::read_to_string(&kit_flag_file).await.unwrap();
        assert_eq!(kit_contents, r#"["kit1","kit2"]"#);
    }

    #[tokio::test]
    async fn test_previous_tags_removed() {
        let kits_dir = tempfile::tempdir().unwrap();
        let flag_file = kits_dir.path().join(KITS_VERIFIED_MARKER_FILE);
        tokio::fs::write(&flag_file, "test").await.unwrap();

        let tagger = VerificationTagger::from(&SDKResolver);
        tagger.write_tags(&kits_dir.path()).await.unwrap();

        assert!(!flag_file.exists());

        let sdk_flag_file = kits_dir.path().join(SDK_VERIFIED_MARKER_FILE);
        assert!(sdk_flag_file.exists());
        let sdk_contents = tokio::fs::read_to_string(&sdk_flag_file).await.unwrap();
        assert_eq!(sdk_contents, r#"["image1","image2"]"#);
    }
}
