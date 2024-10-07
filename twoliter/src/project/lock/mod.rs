/// Covers the functionality and implementation of Twoliter.lock which is generated using
/// `twoliter update`. It acts similarly to Cargo.lock as a flattened out representation of all kit
/// and sdk image dependencies with associated digests so twoliter can validate that contents of a kit
/// do not mutate unexpectedly.

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
use crate::project::{Project, ValidIdentifier};
use crate::schema_version::SchemaVersion;
use anyhow::{bail, ensure, Context, Result};
use image::{ImageResolver, LockedImage};
use oci_cli_wrapper::ImageTool;
use olpc_cjson::CanonicalFormatter as CanonicalJsonFormatter;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::cmp::PartialEq;
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::mem::take;
use tokio::fs::read_to_string;
use tracing::{debug, error, info, instrument};

use super::{Locked, ProjectLock, Unlocked};

const TWOLITER_LOCK: &str = "Twoliter.lock";

#[derive(Serialize, Debug)]
struct ExternalKitMetadata {
    sdk: LockedImage,
    #[serde(rename = "kit")]
    kits: Vec<LockedImage>,
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
    /// Re-resolves the project's SDK to ensure that the lockfile matches the state of the world.
    #[instrument(level = "trace", skip(project))]
    pub(super) async fn load(project: &Project<Unlocked>) -> Result<Self> {
        info!("Resolving SDK project reference to check against lock file");

        let current_lock = Lock::current_lock_state(project).await?;
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
#[derive(Debug, Clone, Eq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Lock {
    /// The version of the Twoliter.toml this was generated from
    pub schema_version: SchemaVersion<1>,
    /// The resolved bottlerocket sdk
    pub sdk: LockedImage,
    /// Resolved kit dependencies
    pub kit: Vec<LockedImage>,
}

impl PartialEq for Lock {
    fn eq(&self, other: &Self) -> bool {
        self.schema_version == other.schema_version
            && self.sdk == other.sdk
            && self.kit == other.kit
    }
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
    /// Re-resolves the project's dependencies to ensure that the lockfile matches the state of the
    /// world.
    #[instrument(level = "trace", skip(project))]
    pub(super) async fn load(project: &Project<Unlocked>) -> Result<Self> {
        info!("Resolving project references to check against lock file");

        let current_lock = Self::current_lock_state(project).await?;
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
        Ok(lock)
    }

    fn external_kit_metadata(&self) -> ExternalKitMetadata {
        ExternalKitMetadata {
            sdk: self.sdk.clone(),
            kits: self.kit.clone(),
        }
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
        for image in self.kit.iter() {
            let image = project.as_project_image(image)?;
            let resolver = ImageResolver::from_image(&image)?;
            resolver
                .extract(&image_tool, &project.external_kits_dir(), arch)
                .await?;
        }

        self.synchronize_metadata(project).await
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
            schema_version: project.schema_version(),
            kit: locked,
            sdk,
        })
    }
}
