//! Covers the functionality and implementation of Twoliter.lock which is generated using
//! `twoliter update`. It acts similarly to Cargo.lock as a flattened out representation of all kit
//! and sdk image dependencies with associated digests so twoliter can validate that contents of a kit
//! do not mutate unexpectedly.

/// Contains operations for working with an OCI Archive
mod archive;
/// Covers resolution and validation of a single image dependency in a lock file
mod image;
/// Provides tools for marking artifacts as having been verified against the Twoliter lockfile
mod verification;
/// Implements view models of common OCI manifest and configuration types
mod views;

pub(crate) use self::verification::VerificationTagger;

use crate::common::fs::{create_dir_all, read, write};
use crate::compatibility::SUPPORTED_TWOLITER_LOCK_SCHEMA_VERSION;
use crate::project::{Project, ValidIdentifier};
use crate::schema_version::SchemaVersion;
use anyhow::{bail, ensure, Context, Result};
use futures::{stream, StreamExt, TryStreamExt};
use image::{ImageResolver, LockedImage};
use oci_cli_wrapper::ImageTool;
use olpc_cjson::CanonicalFormatter as CanonicalJsonFormatter;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::cmp::PartialEq;
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::mem::take;
use std::path::Path;
use tokio::fs::read_to_string;
use tracing::{debug, error, info, instrument, warn};

use super::{Locked, ProjectLock, Unlocked};

const TWOLITER_LOCK: &str = "Twoliter.lock";

/// Filename written inside `external-kits/.sdk-digest` after a successful SDK verification.
const SDK_DIGEST_FILE: &str = ".sdk-digest";
/// Filename written inside `external-kits/<vendor>/<kit>/digest` after a successful kit verification.
const KIT_DIGEST_FILE: &str = "digest";

const CONCURRENT_KIT_EXTRACTIONS: usize = 8;

#[derive(Serialize, Debug)]
struct ExternalKitMetadata {
    sdk: LockedImage,
    #[serde(rename = "kit")]
    kits: Vec<LockedImage>,
    #[serde(default = "Bottlerocket")]
    project_vendor: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Ord, PartialOrd, Serialize, Deserialize, Hash)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Override {
    pub name: Option<String>,
    pub registry: Option<String>,
}

/// A resolved and locked project SDK, typically from the Twoliter.lock file for a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockedSDK(pub LockedImage);

impl AsRef<LockedImage> for LockedSDK {
    fn as_ref(&self) -> &LockedImage {
        &self.0
    }
}

impl LockedSDK {
    /// Loads the locked SDK for the given project.
    ///
    /// Re-resolves the project's SDK against the remote registry to ensure that the lockfile
    /// matches the state of the world. If the SDK manifest digest stored in
    /// `external-kits/.sdk-digest` already matches the digest in `Twoliter.lock`, the remote
    /// check is skipped — the digest file is written after each successful remote verification.
    #[instrument(level = "trace", skip(project))]
    pub(super) async fn load(project: &Project<Unlocked>) -> Result<Self> {
        let current_lock = Lock::current_lock_state(project).await?;

        if sdk_digest_matches(&project.external_kits_dir(), &current_lock.sdk).await {
            info!("SDK digest matches local cache, skipping remote SDK verification");
            return Ok(LockedSDK(current_lock.sdk.clone()));
        }

        info!("Resolving SDK project reference to check against lock file");
        let resolved_lock = Self::resolve_sdk(project)
            .await?
            .context("Project does not have explicit SDK image.")?;

        debug!(
            current_sdk=?current_lock.sdk,
            resolved_sdk=?resolved_lock,
            "Comparing resolved SDK to current lock state"
        );
        if &current_lock.sdk != resolved_lock.as_ref() {
            error!(
                current_sdk=?current_lock.sdk,
                resolved_sdk=?resolved_lock,
                "Locked SDK does not match resolved SDK",
            );
            bail!("Changes have occured to Twoliter.toml or the remote SDK image that require an update to Twoliter.lock");
        }

        if let Err(e) = write_sdk_digest(&project.external_kits_dir(), &resolved_lock.0).await {
            warn!("Failed to cache SDK digest: {}", e);
        }

        Ok(resolved_lock)
    }

    /// Creates a project lock referring to only the resolved SDK image from the project.
    ///
    /// Returns `None` if the project does not have an explicit SDK image.
    #[instrument(level = "trace", skip(project))]
    async fn resolve_sdk(project: &Project<Unlocked>) -> Result<Option<Self>> {
        debug!("Attempting to resolve workspace SDK");
        let sdk = match project.direct_sdk_image_dep() {
            Some(sdk) => sdk?,
            None => {
                debug!("No explicit SDK image provided");
                return Ok(None);
            }
        };

        debug!(?sdk, "Resolving workspace SDK");
        let image_tool = ImageTool::from_builtin_krane();
        ImageResolver::from_image(&sdk)?
            .skip_metadata_retrieval() // SDKs don't have metadata
            .resolve(&image_tool)
            .await
            .map(|(sdk, _)| Some(Self(sdk)))
    }
}

/// Represents the structure of a `Twoliter.lock` lock file.
#[derive(Debug, Clone, Eq, Ord, PartialOrd, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Lock {
    /// The supported version of the Twoliter.lock format.
    ///
    /// This version is independent of the Twoliter.toml schema version.
    pub schema_version: SchemaVersion<SUPPORTED_TWOLITER_LOCK_SCHEMA_VERSION>,
    /// The resolved bottlerocket sdk
    pub sdk: LockedImage,
    /// Resolved kit dependencies
    pub kit: Vec<LockedImage>,
    /// The project vendor
    pub project_vendor: String,
}

#[allow(dead_code)]
impl Lock {
    #[instrument(level = "trace", skip(project))]
    pub(super) async fn create(project: &Project<Unlocked>) -> Result<Self> {
        let lock_file_path = project.project_dir().join(TWOLITER_LOCK);

        info!("Resolving project references to create lock file");
        let lock_state = Self::resolve(project).await?;
        let lock_str = toml::to_string(&lock_state).context("failed to serialize lock file")?;

        debug!("Writing new lock file to '{}'", lock_file_path.display());
        write(&lock_file_path, lock_str)
            .await
            .context("failed to write lock file")?;
        Ok(lock_state)
    }

    /// Loads the lockfile for the given project.
    ///
    /// Re-resolves all kit and SDK dependencies against the remote registry to ensure that the
    /// lockfile matches the state of the world. If every kit and SDK manifest digest stored under
    /// `external-kits/` already matches the corresponding digest in `Twoliter.lock`, the remote
    /// check is skipped. Digest files are written after each successful remote verification.
    #[instrument(level = "trace", skip(project))]
    pub(super) async fn load(project: &Project<Unlocked>) -> Result<Self> {
        let current_lock = Self::current_lock_state(project).await?;

        if Self::all_local_digests_match(project, &current_lock).await {
            info!("All kit and SDK digests match local cache, skipping remote verification");
            return Ok(current_lock);
        }

        info!("Resolving project references to check against lock file");
        let resolved_lock = Self::resolve(project).await?;

        debug!(
            current_lock=?current_lock,
            resolved_lock=?resolved_lock,
            "Comparing resolved lock to current lock state"
        );
        if current_lock != resolved_lock {
            error!(
                current_lock=?current_lock,
                resolved_lock=?resolved_lock,
                "Locked dependencies do not match resolved dependencies"
            );
            bail!("changes have occured to Twoliter.toml or the remote kit images that require an update to Twoliter.lock");
        }

        if let Err(e) = Self::write_all_digests(project, &resolved_lock).await {
            warn!("Failed to cache artifact digests: {}", e);
        }

        Ok(resolved_lock)
    }

    /// Returns the state of the lockfile for the given `Project`
    async fn current_lock_state<L: ProjectLock>(project: &Project<L>) -> Result<Self> {
        let lock_file_path = project.project_dir().join(TWOLITER_LOCK);
        ensure!(
            lock_file_path.exists(),
            "Twoliter.lock does not exist, please run `twoliter update` first"
        );
        debug!("Loading existing lockfile '{}'", lock_file_path.display());
        let lock_str = read_to_string(&lock_file_path)
            .await
            .context("failed to read lockfile")?;
        let lock: Self =
            toml::from_str(lock_str.as_str()).context("failed to deserialize lockfile")?;

        lock.validate_version_source_consistency()
            .context("lockfile validation failed")?;

        Ok(lock)
    }

    fn external_kit_metadata(&self) -> ExternalKitMetadata {
        ExternalKitMetadata {
            sdk: self.sdk.clone(),
            kits: self.kit.clone(),
            project_vendor: self.project_vendor.clone(),
        }
    }

    /// Validates that the version and source URI of each locked image in the lockfile are consistent.
    fn validate_version_source_consistency(&self) -> Result<()> {
        Self::validate_locked_image_consistency(&self.sdk)?;

        for kit in &self.kit {
            Self::validate_locked_image_consistency(kit)?;
        }

        Ok(())
    }

    fn validate_locked_image_consistency(image: &LockedImage) -> Result<()> {
        let expected_tag = format!("v{}", image.version);

        let actual_tag = image
            .source
            .rsplit(':')
            .next()
            .context("source URI does not contain a tag (missing ':' separator)")?;

        if actual_tag != expected_tag {
            bail!(
                "Version-source mismatch in lockfile for {}: \
                version field is '{}' but source URI '{}' has tag '{}'. \
                Expected source to end with ':{expected_tag}'. \
                This usually happens when the lockfile was manually edited. \
                You should prevent manually change the twoliter.lock file.",
                image,
                image.version,
                image.source,
                actual_tag
            );
        }
        Ok(())
    }

    /// Fetches all external kits defined in a Twoliter.lock to the build directory
    #[instrument(level = "trace", skip_all)]
    pub(crate) async fn fetch(&self, project: &Project<Locked>, arch: &str) -> Result<()> {
        let image_tool = ImageTool::from_builtin_krane();
        let target_dir = project.external_kits_dir();
        create_dir_all(&target_dir).await.context(format!(
            "failed to create external-kits directory at {}",
            target_dir.display()
        ))?;

        info!(
            dependencies = ?self.kit.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "Extracting kit dependencies."
        );

        let kit_stream = stream::iter(self.kit.iter());
        kit_stream
            .map(|kit| {
                Result::<_, anyhow::Error>::Ok(async {
                    let image = project.as_project_image(kit)?;
                    let resolver = ImageResolver::from_image(&image)?;
                    resolver
                        .extract(&image_tool, &project.external_kits_dir(), arch)
                        .await?;
                    Ok(())
                })
            })
            .try_buffer_unordered(CONCURRENT_KIT_EXTRACTIONS)
            .try_collect::<Vec<_>>()
            .await?;

        self.synchronize_metadata(project).await?;

        info!("Finished fetching kit dependencies.");

        Ok(())
    }

    pub(crate) async fn synchronize_metadata(&self, project: &Project<Locked>) -> Result<()> {
        let mut kit_list = Vec::new();
        let mut ser =
            serde_json::Serializer::with_formatter(&mut kit_list, CanonicalJsonFormatter::new());
        self.external_kit_metadata()
            .serialize(&mut ser)
            .context("failed to serialize external kit metadata")?;
        // Compare the output of the serialize if the file exists
        let external_metadata_file = project.external_kits_metadata();
        if external_metadata_file.exists() {
            let existing = read(&external_metadata_file).await.context(format!(
                "failed to read external kit metadata: {}",
                external_metadata_file.display()
            ))?;
            // If this is the same as what we generated skip the write
            if existing == kit_list {
                return Ok(());
            }
        }
        write(project.external_kits_metadata(), kit_list.as_slice())
            .await
            .context(format!(
                "failed to write external kit metadata: {}",
                project.external_kits_metadata().display()
            ))?;
        Ok(())
    }

    #[instrument(level = "trace", skip(project))]
    async fn resolve(project: &Project<Unlocked>) -> Result<Self> {
        let mut known: HashMap<(ValidIdentifier, ValidIdentifier), Version> = HashMap::new();
        let mut locked: Vec<LockedImage> = Vec::new();
        let image_tool = ImageTool::from_builtin_krane();
        let mut remaining = project.direct_kit_deps()?;

        let mut sdk_set = HashSet::new();
        if let Some(sdk) = project.direct_sdk_image_dep() {
            // We don't scan over the sdk images as they are not kit images and there is no kit metadata to fetch
            sdk_set.insert(sdk?.clone());
        }
        while !remaining.is_empty() {
            let working_set: Vec<_> = take(&mut remaining);
            for image in working_set.iter() {
                debug!(%image, "Resolving kit '{}'", image.name());
                if let Some(version) =
                    known.get(&(image.name().clone(), image.vendor_name().clone()))
                {
                    let name = image.name().clone();
                    let left_version = image.version().clone();
                    let vendor = image.vendor_name().clone();
                    ensure!(
                        image.version() == version,
                        "cannot have multiple versions of the same kit ({name}-{left_version}@{vendor} \
                        != {name}-{version}@{vendor}",
                    );
                    debug!(
                        ?image,
                        "Skipping kit '{}' as it has already been resolved",
                        image.name()
                    );
                    continue;
                }
                known.insert(
                    (image.name().clone(), image.vendor_name().clone()),
                    image.version().clone(),
                );
                let image_resolver = ImageResolver::from_image(image)?;
                let (locked_image, metadata) = image_resolver.resolve(&image_tool).await?;
                let metadata = metadata.context(format!(
                    "failed to validate kit image with name {} from vendor {}",
                    locked_image.name, locked_image.vendor
                ))?;
                locked.push(locked_image);
                sdk_set.insert(project.as_project_image(&metadata.sdk)?);
                for dep in metadata.kits {
                    remaining.push(project.as_project_image(&dep)?);
                }
            }
        }
        debug!(?sdk_set, "Resolving workspace SDK");
        ensure!(
            sdk_set.len() <= 1,
            "cannot use multiple sdks (found sdk: {})",
            sdk_set
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        let sdk = sdk_set
            .iter()
            .next()
            .context("no sdk was found for use, please specify a sdk in Twoliter.toml")?;

        debug!(?sdk, "Resolving workspace SDK");
        let (sdk, _metadata) = ImageResolver::from_image(sdk)?
            .skip_metadata_retrieval() // SDKs don't have metadata
            .resolve(&image_tool)
            .await?;

        Ok(Self {
            schema_version: SchemaVersion::<SUPPORTED_TWOLITER_LOCK_SCHEMA_VERSION>,
            kit: locked,
            sdk,
            project_vendor: project.project_vendor.clone(),
        })
    }

    /// Returns true if every kit and SDK digest file under `external-kits/` matches
    /// the corresponding digest in `lock`. Returns false if any file is missing or mismatches.
    async fn all_local_digests_match(project: &Project<Unlocked>, lock: &Lock) -> bool {
        let dir = project.external_kits_dir();
        if !sdk_digest_matches(&dir, &lock.sdk).await {
            return false;
        }
        for kit in &lock.kit {
            if !kit_digest_matches(&dir, kit).await {
                return false;
            }
        }
        true
    }

    /// Writes digest files for all kits and the SDK after a successful remote verification.
    async fn write_all_digests(project: &Project<Unlocked>, lock: &Lock) -> Result<()> {
        let dir = project.external_kits_dir();
        write_sdk_digest(&dir, &lock.sdk).await?;
        for kit in &lock.kit {
            write_kit_digest(&dir, kit).await?;
        }
        Ok(())
    }
}

/// Returns the path of the cached digest file for a kit:
/// `external-kits/<vendor>/<kit-name>/digest`
fn kit_digest_path(external_kits_dir: &Path, kit: &LockedImage) -> std::path::PathBuf {
    external_kits_dir
        .join(kit.vendor.as_ref())
        .join(kit.name.as_ref())
        .join(KIT_DIGEST_FILE)
}

/// Returns the path of the cached SDK digest file: `external-kits/.sdk-digest`
fn sdk_digest_path(external_kits_dir: &Path) -> std::path::PathBuf {
    external_kits_dir.join(SDK_DIGEST_FILE)
}

/// Returns true if the stored kit digest matches `kit.digest`.
async fn kit_digest_matches(external_kits_dir: &Path, kit: &LockedImage) -> bool {
    let path = kit_digest_path(external_kits_dir, kit);
    match tokio::fs::read_to_string(&path).await {
        Ok(stored) => stored.trim() == kit.digest,
        Err(_) => false,
    }
}

/// Returns true if the stored SDK digest matches `sdk.digest`.
async fn sdk_digest_matches(external_kits_dir: &Path, sdk: &LockedImage) -> bool {
    let path = sdk_digest_path(external_kits_dir);
    match tokio::fs::read_to_string(&path).await {
        Ok(stored) => stored.trim() == sdk.digest,
        Err(_) => false,
    }
}

/// Writes the kit's manifest digest to its digest file, creating parent directories as needed.
async fn write_kit_digest(external_kits_dir: &Path, kit: &LockedImage) -> Result<()> {
    let path = kit_digest_path(external_kits_dir, kit);
    tokio::fs::create_dir_all(path.parent().expect("kit digest path always has a parent"))
        .await
        .context("failed to create directory for kit digest file")?;
    tokio::fs::write(&path, &kit.digest)
        .await
        .with_context(|| format!("failed to write kit digest file '{}'", path.display()))
}

/// Writes the SDK's manifest digest to its digest file, creating the directory as needed.
async fn write_sdk_digest(external_kits_dir: &Path, sdk: &LockedImage) -> Result<()> {
    let path = sdk_digest_path(external_kits_dir);
    tokio::fs::create_dir_all(path.parent().expect("sdk digest path always has a parent"))
        .await
        .context("failed to create external-kits directory for SDK digest file")?;
    tokio::fs::write(&path, &sdk.digest)
        .await
        .with_context(|| format!("failed to write SDK digest file '{}'", path.display()))
}
