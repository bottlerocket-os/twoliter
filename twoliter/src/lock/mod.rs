/// Covers the functionality and implementation of Twoliter.lock which is generated using
/// `twoliter update`. It acts similarly to Cargo.lock as a flattened out representation of all kit
/// and sdk image dependencies with associated digests so twoliter can validate that contents of a kit
/// do not mutate unexpectedly.

/// Contains operations for working with an OCI Archive
pub mod archive;
/// Covers resolution and validation of a single image dependency in a lock file
pub mod image;
/// Implements view models of common OCI manifest and configuration types
pub mod views;

use crate::common::fs::{create_dir_all, read, write};
use crate::project::{Image, Project, ValidIdentifier};
use crate::schema_version::SchemaVersion;
use anyhow::{ensure, Context, Result};
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
use tracing::{debug, info, instrument};

const TWOLITER_LOCK: &str = "Twoliter.lock";

#[derive(Serialize, Debug)]
struct ExternalKitMetadata {
    sdk: LockedImage,
    #[serde(rename = "kit")]
    kits: Vec<LockedImage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Override {
    pub name: Option<String>,
    pub registry: Option<String>,
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
    pub(crate) async fn create(project: &Project) -> Result<Self> {
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

    #[instrument(level = "trace", skip(project))]
    pub(crate) async fn load(project: &Project) -> Result<Self> {
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
        info!("Resolving project references to check against lock file");
        let lock_state = Self::resolve(project).await?;

        ensure!(lock_state == lock, "changes have occured to Twoliter.toml or the remote kit images that require an update to Twoliter.lock");
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
    pub(crate) async fn fetch(&self, project: &Project, arch: &str) -> Result<()> {
        let image_tool = ImageTool::from_environment()?;
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
            let vendor = project
                .vendor()
                .get(&ValidIdentifier(image.vendor.clone()))
                .context(format!(
                    "failed to find vendor for kit with name '{}' and vendor '{}'",
                    image.name, image.vendor
                ))?;
            let override_ = project
                .overrides()
                .get(&image.vendor)
                .and_then(|x| x.get(&image.name));
            let resolver =
                ImageResolver::from_locked_image(image, image.vendor.as_str(), vendor, override_);
            resolver
                .extract(&image_tool, &project.external_kits_dir(), arch)
                .await?;
        }

        self.synchronize_metadata(project).await
    }

    pub(crate) async fn synchronize_metadata(&self, project: &Project) -> Result<()> {
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
    async fn resolve(project: &Project) -> Result<Self> {
        let vendor_table = project.vendor();
        let mut known: HashMap<(ValidIdentifier, ValidIdentifier), Version> = HashMap::new();
        let mut locked: Vec<LockedImage> = Vec::new();
        let image_tool = ImageTool::from_environment()?;
        let overrides = project.overrides();
        let mut remaining: Vec<Image> = project.kits();
        let mut sdk_set: HashSet<Image> = HashSet::new();
        if let Some(sdk) = project.sdk_image() {
            // We don't scan over the sdk images as they are not kit images and there is no kit metadata to fetch
            sdk_set.insert(sdk.clone());
        }
        while !remaining.is_empty() {
            let working_set: Vec<_> = take(&mut remaining);
            for image in working_set.iter() {
                debug!(%image, "Resolving kit '{}'", image.name);
                if let Some(version) = known.get(&(image.name.clone(), image.vendor.clone())) {
                    let name = image.name.clone();
                    let left_version = image.version.clone();
                    let vendor = image.vendor.clone();
                    ensure!(
                        image.version == *version,
                        "cannot have multiple versions of the same kit ({name}-{left_version}@{vendor} != {name}-{version}@{vendor}",
                    );
                    debug!(
                        ?image,
                        "Skipping kit '{}' as it has already been resolved", image.name
                    );
                    continue;
                }
                let vendor = vendor_table.get(&image.vendor).context(format!(
                    "vendor '{}' is not specified in Twoliter.toml",
                    image.vendor
                ))?;
                known.insert(
                    (image.name.clone(), image.vendor.clone()),
                    image.version.clone(),
                );
                let override_ = overrides
                    .get(&image.vendor.to_string())
                    .and_then(|x| x.get(&image.name.to_string()));
                if let Some(override_) = override_.as_ref() {
                    debug!(
                        ?override_,
                        "Found override for kit '{}' with vendor '{}'", image.name, image.vendor
                    );
                }
                let image_resolver =
                    ImageResolver::from_image(image, image.vendor.0.as_str(), vendor, override_);
                let (locked_image, metadata) = image_resolver.resolve(&image_tool, false).await?;
                let metadata = metadata.context(format!(
                    "failed to validate kit image with name {} from vendor {}",
                    locked_image.name, locked_image.vendor
                ))?;
                locked.push(locked_image);
                sdk_set.insert(metadata.sdk);
                for dep in metadata.kits {
                    remaining.push(dep);
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
        let vendor = vendor_table.get(&sdk.vendor).context(format!(
            "vendor '{}' is not specified in Twoliter.toml",
            sdk.vendor
        ))?;
        let sdk_override = overrides
            .get(&sdk.vendor.to_string())
            .and_then(|x| x.get(&sdk.name.to_string()));
        let sdk_resolver =
            ImageResolver::from_image(sdk, sdk.vendor.0.as_str(), vendor, sdk_override);
        let (sdk, _) = sdk_resolver.resolve(&image_tool, true).await?;
        Ok(Self {
            schema_version: project.schema_version(),
            sdk,
            kit: locked,
        })
    }
}
