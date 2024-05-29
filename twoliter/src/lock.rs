use crate::common::fs::{remove_file, write};
use crate::project::{Image, Project, Vendor};
use crate::schema_version::SchemaVersion;
use anyhow::{ensure, Context, Result};
use base64::Engine;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::hash::{Hash, Hasher};
use std::mem::take;
use tokio::fs::read_to_string;
use tokio::process::Command;

const TWOLITER_LOCK: &str = "Twoliter.lock";

/// Represents the structure of a `Twoliter.lock` lock file.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Lock {
    /// The version of the Twoliter.toml this was generated from
    pub schema_version: SchemaVersion<1>,
    /// The workspace release version
    pub release_version: String,
    /// The resolved bottlerocket sdk
    pub sdk: LockedImage,
    /// Resolved kit dependencies
    pub kit: Vec<LockedImage>,
    /// sha256 digest of the Project this was generated from
    pub digest: String,
}

/// Represents a locked dependency on an image
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub(crate) struct LockedImage {
    /// The name of the dependency
    pub name: String,
    /// The version of the dependency
    pub version: Version,
    /// The vendor this dependency came from
    pub vendor: String,
    /// The resolved image uri of the dependency
    pub source: String,
}

impl LockedImage {
    pub fn new(vendor: &Vendor, image: &Image) -> Self {
        Self {
            name: image.name.clone(),
            version: image.version.clone(),
            vendor: image.vendor.clone(),
            source: format!("{}/{}:v{}", vendor.registry, image.name, image.version),
        }
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

/// The hash should not contain the source to allow for collision detection
impl Hash for LockedImage {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.version.hash(state);
        self.vendor.hash(state);
    }
}

#[derive(Deserialize, Debug)]
struct ImageMetadata {
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

#[derive(Deserialize, Debug)]
struct ManifestListView {
    manifests: Vec<ManifestView>,
}

#[derive(Deserialize, Debug)]
struct ManifestView {
    digest: String,
}

macro_rules! docker {
    ($arg: expr, $error_msg: expr) => {{
        let output = Command::new("docker")
            .args($arg)
            .output()
            .await
            .context($error_msg)?;
        ensure!(output.status.success(), $error_msg);
        output.stdout
    }};
}

#[allow(dead_code)]
impl Lock {
    pub(crate) async fn load(project: &Project) -> Result<Self> {
        let lock_file_path = project.project_dir().join(TWOLITER_LOCK);
        if lock_file_path.exists() {
            let lock_str = read_to_string(&lock_file_path)
                .await
                .context("failed to read lockfile")?;
            let lock: Self =
                toml::from_str(lock_str.as_str()).context("failed to deserialize lockfile")?;
            // The digests must match, if changes are needed twoliter
            ensure!(lock.digest == project.digest()?, "changes have occurred to Twoliter.toml that require an update to Twoliter.lock, if intentional please run twoliter update");
            return Ok(lock);
        }
        Self::create(project).await
    }

    pub(crate) async fn create(project: &Project) -> Result<Self> {
        let lock_file_path = project.project_dir().join(TWOLITER_LOCK);
        if lock_file_path.exists() {
            remove_file(&lock_file_path).await?;
        }
        let lock = Self::resolve(project).await?;
        let lock_str = toml::to_string(&lock).context("failed to serialize lock file")?;
        write(&lock_file_path, lock_str)
            .await
            .context("failed to write lock file")?;
        Ok(lock)
    }

    async fn resolve(project: &Project) -> Result<Self> {
        let vendor_table = project.vendor();
        let mut known: HashMap<(String, String), Version> = HashMap::new();
        let mut locked: Vec<LockedImage> = Vec::new();

        let mut remaining: Vec<Image> = project.kits();
        let mut sdk_set: HashSet<Image> = HashSet::new();
        if let Some(sdk) = project.sdk_image() {
            remaining.push(sdk.clone());
            sdk_set.insert(sdk.clone());
        }
        while !remaining.is_empty() {
            let working_set: Vec<_> = take(&mut remaining);
            for image in working_set.iter() {
                if let Some(version) = known.get(&(image.name.clone(), image.vendor.clone())) {
                    let name = image.name.clone();
                    let left_version = image.version.clone();
                    let vendor = image.vendor.clone();
                    ensure!(
                        image.version == *version,
                        "cannot have multiple versions of the same kit ({name}-{left_version}@{vendor} != {name}-{version}@{vendor}",
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
                let locked_image = LockedImage::new(vendor, image);
                let kit = Self::find_kit(vendor, &locked_image).await?;
                locked.push(locked_image);
                sdk_set.insert(kit.sdk);
                for dep in kit.kits {
                    remaining.push(dep);
                }
            }
        }
        ensure!(
            sdk_set.len() <= 1,
            "cannot use multiple sdks (found sdk: {})",
            sdk_set
                .iter()
                .map(|x| format!("{}-{}@{}", x.name, x.version, x.vendor))
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

        Ok(Self {
            schema_version: project.schema_version(),
            release_version: project.release_version().to_string(),
            digest: project.digest()?,
            sdk: LockedImage::new(vendor, sdk),
            kit: locked,
        })
    }

    async fn resolve_kit(
        vendors: &HashMap<String, Vendor>,
        image: &Image,
    ) -> Result<ImageMetadata> {
        let vendor = vendors.get(&image.vendor).context(format!(
            "no vendor '{}' specified in Twoliter.toml",
            image.vendor
        ))?;
        let locked_image = LockedImage {
            name: image.name.clone(),
            version: image.version.clone(),
            vendor: image.vendor.clone(),
            source: format!("{}/{}:v{}", vendor.registry, image.name, image.version),
        };
        Self::find_kit(vendor, &locked_image).await
    }

    async fn find_kit(vendor: &Vendor, image: &LockedImage) -> Result<ImageMetadata> {
        // Now inspect the manifest list
        let manifest_bytes = docker!(
            ["manifest", "inspect", image.source.as_str()],
            format!("failed to find a kit {}", image.to_string())
        );
        let manifest_list: ManifestListView = serde_json::from_slice(manifest_bytes.as_slice())
            .context("failed to deserialize manifest list")?;

        let mut encoded_metadata: Option<String> = None;
        for manifest in manifest_list.manifests.iter() {
            let image_uri = format!("{}/{}@{}", vendor.registry, image.name, manifest.digest);

            // Now we want to fetch the metadata from the OCI image config
            let label_bytes = docker!(
                [
                    "image",
                    "inspect",
                    image_uri.as_str(),
                    "--format \"{{ json .Config.Labels }}\"",
                ],
                format!(
                    "failed to fetch kit metadata for {} with digest {}",
                    image.to_string(),
                    manifest.digest
                )
            );
            // Otherwise we should have a list of json blobs we can fetch the metadata from the label
            let labels: HashMap<String, String> = serde_json::from_slice(label_bytes.as_slice())
                .context(format!(
                    "could not deserialize labels on the image for {}",
                    image
                ))?;
            let encoded = labels
                .get("dev.bottlerocket.kit.v1")
                .context("no metadata stored on image, this image appears to not be a kit")?;
            if let Some(metadata) = encoded_metadata.as_ref() {
                ensure!(
                    encoded == metadata,
                    "metadata does match between images in manifest list"
                );
            } else {
                encoded_metadata = Some(encoded.clone());
            }
        }
        let encoded =
            encoded_metadata.context(format!("could not find metadata for kit {}", image))?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded.as_str())
            .context("malformed kit metadata detected")?;

        serde_json::from_slice(decoded.as_slice()).context("malformed kit metadata json")
    }
}
