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
use crate::common;
use anyhow::{anyhow, Context, Result};
use olpc_cjson::CanonicalFormatter as CanonicalJsonFormatter;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeSet;
use std::fmt::Debug;
use std::hash::{Hash, Hasher};
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
#[derive(Debug, Default, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
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

impl VerificationTagger {
    pub fn no_verifications() -> Self {
        Self {
            tags: BTreeSet::new(),
        }
    }
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
    /// with proper handling of concurrent processes using file-based locking
    #[instrument(level = "trace", skip(external_kits_dir))]
    pub(crate) async fn write_tags<P: AsRef<Path>>(&self, external_kits_dir: P) -> Result<()> {
        let external_kits_dir = external_kits_dir.as_ref();

        // Ensure the directory exists
        tokio::fs::create_dir_all(external_kits_dir)
            .await
            .context(format!(
                "failed to create directory '{}'",
                external_kits_dir.display()
            ))?;

        // Create a file locker for this operation
        let lock_path = external_kits_dir.join(".verification.lock");
        let file_locker = common::FileLocker::new(&lock_path);

        // Acquire the lock directly
        let lock = file_locker
            .try_acquire()
            .await?
            .ok_or_else(|| anyhow!("Failed to acquire lock for verification tags"))?;

        // Process tags while holding the lock
        let result = self.process_tags(external_kits_dir).await;

        drop(lock);
        result
    }

    /// Process all tags once the lock is acquired
    async fn process_tags(&self, external_kits_dir: &Path) -> Result<()> {
        // First delete any tag files that we don't have
        for tag_type in VerifyTag::iter() {
            let flag_file = external_kits_dir.join(tag_type.marker_file_name());
            let has_tag = self
                .tags
                .iter()
                .any(|t| t.marker_file_name() == tag_type.marker_file_name());

            if !has_tag && flag_file.exists() {
                debug!("Removing unused tag file '{}'", flag_file.display());
                tokio::fs::remove_file(&flag_file).await.context(format!(
                    "Failed to remove tag file '{}'",
                    flag_file.display()
                ))?;
            }
        }

        // Now process our tags
        for tag in &self.tags {
            let flag_file = external_kits_dir.join(tag.marker_file_name());
            let new_content = tag.manifest().as_canonical_json()?;

            // Check if we need to update the file
            let need_update = if flag_file.exists() {
                match tokio::fs::read(&flag_file).await {
                    Ok(existing) => {
                        Self::calculate_hash(&existing) != Self::calculate_hash(&new_content)
                    }
                    Err(_) => true, // If we can't read it, we'll rewrite it
                }
            } else {
                true // File doesn't exist, need to create it
            };

            if need_update {
                // If the file exists but content is different, remove it first
                if flag_file.exists() {
                    let _ = tokio::fs::remove_file(&flag_file).await;
                }

                debug!("Writing tag file '{}'", flag_file.display());
                tokio::fs::write(&flag_file, &new_content)
                    .await
                    .context(format!(
                        "Failed to write tag file '{}'",
                        flag_file.display()
                    ))?;
            } else {
                debug!("Tag file '{}' unchanged, skipping", flag_file.display());
            }
        }

        Ok(())
    }

    /// Calculate a hash for content
    fn calculate_hash(content: &[u8]) -> u64 {
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        hasher.finish()
    }

    /// Safely removes all verification tags using file-based locking
    ///
    /// This implementation uses proper locking to prevent race conditions between concurrent processes.
    #[instrument(level = "trace", skip(external_kits_dir))]
    pub(crate) async fn cleanup_existing_tags<P: AsRef<Path>>(external_kits_dir: P) -> Result<()> {
        // Create a VerificationTagger with no tags, which will remove all existing tags
        let empty_tagger = Self::no_verifications();
        empty_tagger.write_tags(external_kits_dir).await
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

    #[tokio::test]
    async fn test_content_based_skipping() {
        // Test that we don't rewrite identical content (optimization)
        let kits_dir = tempfile::tempdir().unwrap();
        let tagger = VerificationTagger::from(&SDKResolver);

        // First write
        tagger.write_tags(&kits_dir.path()).await.unwrap();
        let sdk_flag_file = kits_dir.path().join(SDK_VERIFIED_MARKER_FILE);
        let original_metadata = sdk_flag_file.metadata().unwrap();
        let original_modified = original_metadata.modified().unwrap();

        // Sleep to ensure potential timestamp difference would be detectable
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Write again with same content
        tagger.write_tags(&kits_dir.path()).await.unwrap();
        let new_metadata = sdk_flag_file.metadata().unwrap();
        let new_modified = new_metadata.modified().unwrap();

        // File should not have been rewritten (timestamps should match)
        assert_eq!(
            original_modified, new_modified,
            "File was rewritten despite identical content"
        );
    }

    #[tokio::test]
    async fn test_cleanup_functionality() {
        // Test that cleanup_existing_tags properly removes all verification tags
        let kits_dir = tempfile::tempdir().unwrap();

        // First create all types of tag files
        let kit_tagger = VerificationTagger::from(&KitResolver);
        kit_tagger.write_tags(kits_dir.path()).await.unwrap();

        // Verify SDK and Kit tags exist
        let sdk_file = kits_dir.path().join(SDK_VERIFIED_MARKER_FILE);
        let kit_file = kits_dir.path().join(KITS_VERIFIED_MARKER_FILE);
        assert!(sdk_file.exists(), "SDK tag file missing after write");
        assert!(kit_file.exists(), "Kit tag file missing after write");

        // Now remove all tags
        VerificationTagger::cleanup_existing_tags(kits_dir.path())
            .await
            .unwrap();

        // Verify all tags were removed
        assert!(
            !sdk_file.exists(),
            "SDK tag file still exists after cleanup_existing_tags"
        );
        assert!(
            !kit_file.exists(),
            "Kit tag file still exists after cleanup_existing_tags"
        );
    }

    #[tokio::test]
    async fn test_concurrent_tag_writing() {
        // Test that concurrent writes are handled safely
        let kits_dir = tempfile::tempdir().unwrap();

        // Create two different taggers
        let sdk_tagger = VerificationTagger::from(&SDKResolver);
        let kit_tagger = VerificationTagger::from(&KitResolver);

        // Launch concurrent writes
        let sdk_future = sdk_tagger.write_tags(kits_dir.path());
        let kit_future = kit_tagger.write_tags(kits_dir.path());

        // Both should complete without errors
        let (sdk_result, kit_result) = tokio::join!(sdk_future, kit_future);
        assert!(
            sdk_result.is_ok(),
            "SDK tagger failed during concurrent write"
        );
        assert!(
            kit_result.is_ok(),
            "Kit tagger failed during concurrent write"
        );

        // One of the taggers should have "won" - ultimately we should have consistent state
        let sdk_file = kits_dir.path().join(SDK_VERIFIED_MARKER_FILE);
        let kit_file = kits_dir.path().join(KITS_VERIFIED_MARKER_FILE);

        assert!(
            sdk_file.exists(),
            "SDK tag file missing after concurrent writes"
        );

        // Check if the winning write was from the KitResolver (which includes kit tags)
        // or the SDKResolver (which only includes SDK tags)
        if kit_file.exists() {
            // KitResolver won the race
            let kit_contents = tokio::fs::read_to_string(&kit_file).await.unwrap();
            assert_eq!(kit_contents, r#"["kit1","kit2"]"#);
        }

        // Either way, the SDK file should exist with proper content
        let sdk_contents = tokio::fs::read_to_string(&sdk_file).await.unwrap();
        assert_eq!(sdk_contents, r#"["image1","image2"]"#);
    }
}
