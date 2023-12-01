use crate::docker::ImageArchUri;
use crate::schema_version::SchemaVersion;
use anyhow::{ensure, Context, Result};
use async_recursion::async_recursion;
use log::{debug, trace};
use non_empty_string::NonEmptyString;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use std::path::{Path, PathBuf};
use tokio::fs;

/// Common functionality in commands, if the user gave a path to the `Twoliter.toml` file,
/// we use it, otherwise we search for the file. Returns the `Project` and the path at which it was
/// found (this is the same as `user_path` if provided).
pub(crate) async fn load_or_find_project(user_path: Option<PathBuf>) -> Result<Project> {
    let project = match user_path {
        None => Project::find_and_load(".").await?,
        Some(p) => Project::load(&p).await?,
    };
    debug!(
        "Project file loaded from '{}'",
        project.filepath().display()
    );
    Ok(project)
}

/// Represents the structure of a `Twoliter.toml` project file.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Project {
    #[serde(skip)]
    filepath: PathBuf,
    #[serde(skip)]
    project_dir: PathBuf,

    /// The version of this schema struct.
    schema_version: SchemaVersion<1>,

    /// The version that will be given to released artifacts such as kits and variants.
    release_version: String,

    /// The Bottlerocket SDK container image.
    sdk: Option<ImageName>,

    /// The Bottlerocket Toolchain container image.
    toolchain: Option<ImageName>,
}

impl Project {
    /// Load a `Twoliter.toml` file from the given file path (it can have any filename).
    pub(crate) async fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let data = fs::read_to_string(&path)
            .await
            .context(format!("Unable to read project file '{}'", path.display()))?;
        let mut project: Self = toml::from_str(&data).context(format!(
            "Unable to deserialize project file '{}'",
            path.display()
        ))?;
        project.filepath = path.into();
        project.project_dir = project
            .filepath
            .parent()
            .context(format!(
                "Unable to find the parent directory of '{}'",
                project.filepath.display(),
            ))?
            .into();
        Ok(project)
    }

    /// Recursively search for a file named `Twoliter.toml` starting in `dir`. If it is not found,
    /// move up (i.e. `cd ..`) until it is found. Return an error if there is no parent directory.
    #[async_recursion]
    pub(crate) async fn find_and_load<P: AsRef<Path> + Send>(dir: P) -> Result<Self> {
        let dir = dir.as_ref();
        trace!("Looking for Twoliter.toml in '{}'", dir.display());
        ensure!(
            dir.is_dir(),
            "Unable to locate Twoliter.toml in '{}': not a directory",
            dir.display()
        );
        let dir = dir
            .canonicalize()
            .context(format!("Unable to canonicalize '{}'", dir.display()))?;
        let filepath = dir.join("Twoliter.toml");
        if filepath.is_file() {
            return Self::load(&filepath).await;
        }
        // Move up a level and recurse.
        let parent = dir
            .parent()
            .context("Unable to find Twoliter.toml file")?
            .to_owned();
        Self::find_and_load(parent).await
    }

    pub(crate) fn filepath(&self) -> PathBuf {
        self.filepath.clone()
    }

    pub(crate) fn project_dir(&self) -> PathBuf {
        self.project_dir.clone()
    }

    pub(crate) fn _release_version(&self) -> &str {
        self.release_version.as_str()
    }

    pub(crate) fn sdk_name(&self) -> Option<&ImageName> {
        self.sdk.as_ref()
    }

    pub(crate) fn toolchain_name(&self) -> Option<&ImageName> {
        self.toolchain.as_ref()
    }

    pub(crate) fn sdk(&self, arch: &str) -> Option<ImageArchUri> {
        self.sdk_name().map(|s| s.uri(arch))
    }

    pub(crate) fn toolchain(&self, arch: &str) -> Option<ImageArchUri> {
        self.toolchain_name().map(|s| s.uri(arch))
    }

    pub(crate) fn token(&self) -> String {
        let mut d = Sha512::new();
        d.update(self.filepath().display().to_string());
        let digest = hex::encode(d.finalize());
        (digest[..12]).to_string()
    }
}

/// A base name for an image that can be suffixed using a naming convention. For example,
/// `registry=public.ecr.aws/bottlerocket`, `name=bottlerocket`, `version=v0.50.0` can be suffixed
/// via naming convention to produce:
/// - `registry=public.ecr.aws/bottlerocket/bottlerocket-sdk-x86_64:v0.50.0`
/// - `registry=public.ecr.aws/bottlerocket/bottlerocket-toolchain-aarch64:v0.50.0`
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct ImageName {
    /// The registry, e.g. `public.ecr.aws/bottlerocket`. Optional because locally cached images may
    /// not specify a registry.
    pub(crate) registry: Option<NonEmptyString>,
    /// The base name of the image that can be suffixed. For example `bottlerocket` can become
    /// `bottlerocket-sdk` or `bottlerocket-toolchain`.
    pub(crate) name: NonEmptyString,
    /// The version tag, for example `v0.50.0`
    pub(crate) version: NonEmptyString,
}

impl ImageName {
    pub(crate) fn uri<S>(&self, arch: S) -> ImageArchUri
    where
        S: AsRef<str>,
    {
        ImageArchUri::new(
            self.registry.as_ref().map(|s| s.to_string()),
            self.name.clone(),
            arch.as_ref(),
            &self.version,
        )
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test::data_dir;
    use tempfile::TempDir;
    use tokio::fs;

    /// Ensure that `Twoliter.toml` can be deserialized.
    #[tokio::test]
    async fn deserialize_twoliter_1_toml() {
        let path = data_dir().join("Twoliter-1.toml");
        let deserialized = Project::load(path).await.unwrap();

        // Add checks here as desired to validate deserialization.
        assert_eq!(SchemaVersion::<1>, deserialized.schema_version);
        let sdk_name = deserialized.sdk_name().unwrap();
        let toolchain_name = deserialized.toolchain_name().unwrap();
        assert_eq!("a.com/b", sdk_name.registry.as_ref().unwrap().as_str());
        assert_eq!(
            "my-bottlerocket-sdk",
            deserialized.sdk_name().unwrap().name.as_str()
        );
        assert_eq!("v1.2.3", deserialized.sdk_name().unwrap().version.as_str());
        assert_eq!("c.co/d", toolchain_name.registry.as_ref().unwrap().as_str());
        assert_eq!(
            "toolchainz",
            deserialized.toolchain_name().unwrap().name.as_str()
        );
        assert_eq!(
            "v3.4.5",
            deserialized.toolchain_name().unwrap().version.as_str()
        );
    }

    /// Ensure that a `Twoliter.toml` cannot be serialized if the `schema_version` is incorrect.
    #[tokio::test]
    async fn deserialize_invalid_version() {
        let path = data_dir().join("Twoliter-invalid-version.toml");
        let result = Project::load(path).await;
        let err = result.err().unwrap();
        let caused_by = err.source().unwrap().to_string();
        assert!(
            caused_by.contains("got '4294967295'"),
            "Expected the error message to contain \"got '4294967295'\", but the error message was this: {}",
            caused_by
        );
    }

    /// Ensure the `find_and_load` function searches upward until it finds `Twoliter.toml`.
    #[tokio::test]
    async fn find_and_deserialize_twoliter_1_toml() {
        let original_path = data_dir().join("Twoliter-1.toml");
        let tempdir = TempDir::new().unwrap();
        let twoliter_toml_path = tempdir.path().join("Twoliter.toml");
        let subdir = tempdir.path().join("a").join("b").join("c");
        fs::create_dir_all(&subdir).await.unwrap();
        fs::copy(&original_path, &twoliter_toml_path).await.unwrap();
        let project = Project::find_and_load(subdir).await.unwrap();

        // Ensure that the file we loaded was the one we expected to load.
        assert_eq!(project.filepath(), twoliter_toml_path);
    }

    #[test]
    fn test_sdk_toolchain_uri() {
        let project = Project {
            filepath: Default::default(),
            project_dir: Default::default(),
            schema_version: Default::default(),
            release_version: String::from("1.0.0"),
            sdk: Some(ImageName {
                registry: Some("example.com".try_into().unwrap()),
                name: "foo-abc".try_into().unwrap(),
                version: "version1".try_into().unwrap(),
            }),
            toolchain: Some(ImageName {
                registry: Some("example.com".try_into().unwrap()),
                name: "foo-def".try_into().unwrap(),
                version: "version2".try_into().unwrap(),
            }),
        };

        assert_eq!(
            "example.com/foo-abc-x86_64:version1",
            project.sdk("x86_64").unwrap().to_string()
        );
        assert_eq!(
            "example.com/foo-def-aarch64:version2",
            project.toolchain("aarch64").unwrap().to_string()
        );
    }
}
