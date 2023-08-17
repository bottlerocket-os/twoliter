use crate::docker::{ImageArchUri, DEFAULT_REGISTRY, DEFAULT_SDK_NAME, DEFAULT_SDK_VERSION};
use anyhow::{ensure, Context, Result};
use async_recursion::async_recursion;
use log::{debug, trace};
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
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
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Project {
    #[serde(skip)]
    filepath: PathBuf,
    #[serde(skip)]
    project_dir: PathBuf,
    pub(crate) schema_version: SchemaVersion<1>,
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
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Sdk {
    pub(crate) registry: Option<String>,
    pub(crate) name: String,
    pub(crate) version: String,
}

impl Default for Sdk {
    fn default() -> Self {
        Self {
            registry: Some(DEFAULT_REGISTRY.to_string()),
            name: DEFAULT_SDK_NAME.to_string(),
            version: DEFAULT_SDK_VERSION.to_string(),
        }
    }
}

impl Sdk {
    pub(crate) fn uri<S: Into<String>>(&self, arch: S) -> ImageArchUri {
        ImageArchUri::new(self.registry.clone(), &self.name, arch, &self.version)
    }
}

/// We need to constrain the `Project` struct to a valid version. Unfortunately `serde` does not
/// have an after-deserialization validation hook, so we have this struct to limit the version to a
/// single acceptable value.
#[derive(Default, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct SchemaVersion<const N: u32>;

impl<const N: u32> SchemaVersion<N> {
    pub(crate) fn get(&self) -> u32 {
        N
    }

    pub(crate) fn get_static() -> u32 {
        N
    }
}

impl<const N: u32> From<SchemaVersion<N>> for u32 {
    fn from(value: SchemaVersion<N>) -> Self {
        value.get()
    }
}

impl<const N: u32> fmt::Debug for SchemaVersion<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        fmt::Debug::fmt(&self.get(), f)
    }
}

impl<const N: u32> fmt::Display for SchemaVersion<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        fmt::Display::fmt(&self.get(), f)
    }
}

impl<const N: u32> Serialize for SchemaVersion<N> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u32(self.get())
    }
}

impl<'de, const N: u32> Deserialize<'de> for SchemaVersion<N> {
    fn deserialize<D>(deserializer: D) -> Result<SchemaVersion<N>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value: u32 = Deserialize::deserialize(deserializer)?;
        if value != Self::get_static() {
            Err(Error::custom(format!(
                "Incorrect project schema_version: got '{}', expected '{}'",
                value,
                Self::get_static()
            )))
        } else {
            Ok(Self)
        }
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
        assert_eq!(SchemaVersion::<1>::default(), deserialized.schema_version);
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
}
