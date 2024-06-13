use crate::common::fs::{create_dir_all, read, remove_dir_all, remove_file, write};
use crate::project::{Image, Project, ValidIdentifier, Vendor};
use crate::schema_version::SchemaVersion;
use anyhow::{ensure, Context, Result};
use base64::Engine;
use buildsys_config::DockerArchitecture;
use olpc_cjson::CanonicalFormatter as CanonicalJsonFormatter;
use semver::Version;
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize};
use sha2::Digest;
use std::cmp::PartialEq;
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::mem::take;
use std::path::{Path, PathBuf};
use tar::Archive as TarArchive;
use tempfile::TempDir;
use tokio::fs::read_to_string;
use tokio::process::Command;

const TWOLITER_LOCK: &str = "Twoliter.lock";

macro_rules! docker {
    ($arg: expr, $error_msg: expr) => {{
        let output = Command::new("docker")
            .args($arg)
            .output()
            .await
            .context($error_msg)?;
        ensure!(
            output.status.success(),
            "docker failed to run operation: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }};
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
    /// The digest of the image
    pub digest: String,
    #[serde(skip)]
    pub(crate) manifest: Vec<u8>,
}

impl LockedImage {
    pub async fn new(vendor: &Vendor, image: &Image) -> Result<Self> {
        let source = format!("{}/{}:v{}", vendor.registry, image.name, image.version);
        let manifest_bytes = docker!(
            ["manifest", "inspect", source.as_str()],
            format!("failed to inspect manifest of resource at {}", source)
        );

        // We calculate a 'digest' of the manifest to use as our unique id
        let digest = sha2::Sha256::digest(manifest_bytes.as_slice());
        let digest = base64::engine::general_purpose::STANDARD.encode(digest.as_slice());
        Ok(Self {
            name: image.name.to_string(),
            version: image.version.clone(),
            vendor: image.vendor.to_string(),
            source,
            digest,
            manifest: manifest_bytes,
        })
    }

    pub fn digest_uri(&self, digest: &str) -> String {
        self.source.replace(
            format!(":v{}", self.version).as_str(),
            format!("@{}", digest).as_str(),
        )
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

#[derive(Deserialize, Debug, Clone)]
struct ManifestView {
    digest: String,
    platform: Option<Platform>,
}

#[derive(Deserialize, Debug, Clone)]
struct Platform {
    architecture: DockerArchitecture,
}

#[derive(Deserialize, Debug)]
struct IndexView {
    manifests: Vec<ManifestView>,
}

#[derive(Deserialize, Debug)]
struct ManifestLayoutView {
    layers: Vec<Layer>,
}

#[derive(Deserialize, Debug)]
struct Layer {
    digest: ContainerDigest,
}

#[derive(Debug)]
struct ContainerDigest(String);

impl<'de> Deserialize<'de> for ContainerDigest {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let digest = String::deserialize(deserializer)?;
        if !digest.starts_with("sha256:") {
            return Err(D::Error::custom(format!(
                "invalid digest detected in layer: {}",
                digest
            )));
        };
        Ok(Self(digest))
    }
}

impl Display for ContainerDigest {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.as_str())
    }
}

#[derive(Serialize, Debug)]
struct ExternalKitMetadata {
    sdk: LockedImage,
    #[serde(rename = "kit")]
    kits: Vec<LockedImage>,
}

#[derive(Debug)]
struct OCIArchive {
    image: LockedImage,
    digest: String,
    cache_dir: PathBuf,
}

impl OCIArchive {
    fn new<P>(image: &LockedImage, digest: &str, cache_dir: P) -> Self
    where
        P: AsRef<Path>,
    {
        Self {
            image: image.clone(),
            digest: digest.into(),
            cache_dir: cache_dir.as_ref().to_path_buf(),
        }
    }

    fn archive_path(&self) -> PathBuf {
        self.cache_dir.join(self.digest.replace(':', "-"))
    }

    async fn pull_image(&self) -> Result<()> {
        let digest_uri = self.image.digest_uri(self.digest.as_str());
        let oci_archive_path = self.archive_path();
        if !oci_archive_path.exists() {
            let oci_archive_str = oci_archive_path.to_string_lossy();
            docker!(
                ["save", digest_uri.as_str(), "-o", oci_archive_str.as_ref()],
                format!(
                    "failed to fetch kit and save to disk from {} to {}",
                    digest_uri, oci_archive_str
                )
            );
        }
        Ok(())
    }

    async fn unpack_layers<P>(&self, out_dir: P) -> Result<()>
    where
        P: AsRef<Path>,
    {
        let path = out_dir.as_ref();
        let digest_file = path.join("digest");
        if digest_file.exists() {
            let digest = read_to_string(&digest_file).await.context(format!(
                "failed to read digest file at {}",
                digest_file.display()
            ))?;
            if digest == self.digest {
                return Ok(());
            }
        }

        remove_dir_all(path).await?;
        create_dir_all(path).await?;
        let oci_archive_path = self.archive_path();
        let mut oci_file = File::open(&oci_archive_path).context(format!(
            "failed to open oci archive at {}",
            oci_archive_path.display()
        ))?;
        let mut oci_archive = TarArchive::new(&mut oci_file);
        let temp_dir = TempDir::new_in(path).context(format!(
            "failed to create temporary directory inside {}",
            path.display()
        ))?;
        oci_archive
            .unpack(temp_dir.path())
            .context("failed to unpack oci image")?;
        let index_bytes = read(temp_dir.path().join("index.json")).await?;
        let index: IndexView = serde_json::from_slice(index_bytes.as_slice())
            .context("failed to deserialize oci image index")?;

        // Read the manifest so we can get the layer digests
        let digest = index
            .manifests
            .first()
            .context("empty oci image")?
            .digest
            .replace(':', "/");
        let manifest_bytes = read(temp_dir.path().join(format!("blobs/{digest}")))
            .await
            .context("failed to read manifest blob")?;
        let manifest_layout: ManifestLayoutView = serde_json::from_slice(manifest_bytes.as_slice())
            .context("failed to deserialize oci manifest")?;

        // Extract each layer into the target directory
        for layer in manifest_layout.layers {
            let digest = layer.digest.to_string().replace(':', "/");
            let mut layer_blob = File::open(temp_dir.path().join(format!("blobs/{digest}")))
                .context("failed to read layer of oci image")?;
            let decompressed = flate2::read::GzDecoder::new(&mut layer_blob);
            let mut layer_archive = TarArchive::new(decompressed);
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

    fn external_kit_metadata(&self) -> ExternalKitMetadata {
        ExternalKitMetadata {
            sdk: self.sdk.clone(),
            kits: self.kit.clone(),
        }
    }

    /// Fetches all external kits defined in a Twoliter.lock to the build directory
    pub(crate) async fn fetch(&self, project: &Project, arch: &str) -> Result<()> {
        let target_dir = project.external_kits_dir();
        create_dir_all(&target_dir).await.context(format!(
            "failed to create external-kits directory at {}",
            target_dir.display()
        ))?;
        for image in self.kit.iter() {
            self.extract_kit(&project.external_kits_dir(), image, arch)
                .await?;
        }
        let mut kit_list = Vec::new();
        let mut ser =
            serde_json::Serializer::with_formatter(&mut kit_list, CanonicalJsonFormatter::new());
        self.external_kit_metadata()
            .serialize(&mut ser)
            .context("failed to serialize external kit metadata")?;
        write(project.external_kits_metadata(), kit_list.as_slice())
            .await
            .context(format!(
                "failed to write external kit metadata: {}",
                project.external_kits_metadata().display()
            ))?;

        Ok(())
    }

    async fn get_manifest(&self, image: &LockedImage, arch: &str) -> Result<ManifestView> {
        let manifest_bytes = docker!(
            ["manifest", "inspect", image.source.as_str()],
            format!("failed to find a kit {}", image.to_string())
        );
        let manifest_list: ManifestListView = serde_json::from_slice(manifest_bytes.as_slice())
            .context("failed to deserialize manifest list")?;
        let docker_arch = DockerArchitecture::try_from(arch)?;
        manifest_list
            .manifests
            .iter()
            .find(|x| x.platform.as_ref().unwrap().architecture == docker_arch)
            .cloned()
            .context(format!(
                "could not find kit image for architecture '{}' at {}",
                docker_arch, image.source
            ))
    }

    async fn extract_kit<P>(&self, path: P, image: &LockedImage, arch: &str) -> Result<()>
    where
        P: AsRef<Path>,
    {
        let vendor = image.vendor.clone();
        let name = image.name.clone();
        let target_path = path.as_ref().join(format!("{vendor}/{name}/{arch}"));
        let cache_path = path.as_ref().join("cache");
        create_dir_all(&target_path).await?;
        create_dir_all(&cache_path).await?;

        // First get the manifest for the specific requested architecture
        let manifest = self.get_manifest(image, arch).await?;
        let oci_archive = OCIArchive::new(image, manifest.digest.as_str(), &cache_path);

        // Checks for the saved image locally, or else pulls and saves it
        oci_archive.pull_image().await?;

        // Checks if this archive has already been extracted by checking a digest file
        // otherwise cleans up the path and unpacks the archive
        oci_archive.unpack_layers(&target_path).await?;

        Ok(())
    }

    async fn resolve(project: &Project) -> Result<Self> {
        let vendor_table = project.vendor();
        let mut known: HashMap<(ValidIdentifier, ValidIdentifier), Version> = HashMap::new();
        let mut locked: Vec<LockedImage> = Vec::new();

        let mut remaining: Vec<Image> = project.kits();
        let mut sdk_set: HashSet<Image> = HashSet::new();
        if let Some(sdk) = project.sdk_image() {
            // We don't scan over the sdk images as they are not kit images and there is no kit metadata to fetch
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
                let locked_image = LockedImage::new(vendor, image).await?;
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
            sdk: LockedImage::new(vendor, sdk).await?,
            kit: locked,
        })
    }

    async fn find_kit(vendor: &Vendor, image: &LockedImage) -> Result<ImageMetadata> {
        let manifest_list: ManifestListView = serde_json::from_slice(image.manifest.as_slice())
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
                    "--format",
                    "\"{{ json .Config.Labels }}\"",
                ],
                format!(
                    "failed to fetch kit metadata for {} with digest {}",
                    image.to_string(),
                    manifest.digest
                )
            );
            let label_str = String::from_utf8_lossy(label_bytes.as_slice()).to_string();
            let label_str = label_str.trim().trim_matches('"');
            let labels: HashMap<String, String> = serde_json::from_str(label_str).context(
                format!("could not deserialize labels on the image for {}", image),
            )?;
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
