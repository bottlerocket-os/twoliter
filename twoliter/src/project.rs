use crate::common::fs;
use crate::docker::ImageUri;
use crate::schema_version::SchemaVersion;
use anyhow::{ensure, Context, Result};
use async_recursion::async_recursion;
use async_walkdir::WalkDir;
use futures::stream::StreamExt;
use log::{debug, info, trace, warn};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use toml::Table;

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
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Serialize)]
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
    sdk: Option<Image>,

    /// Set of vendors
    vendor: BTreeMap<String, Vendor>,

    /// Set of kit dependencies
    kit: Vec<Image>,
}

impl Project {
    /// Load a `Twoliter.toml` file from the given file path (it can have any filename).
    pub(crate) async fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let data = fs::read_to_string(&path)
            .await
            .context(format!("Unable to read project file '{}'", path.display()))?;
        let unvalidated: UnvalidatedProject = toml::from_str(&data).context(format!(
            "Unable to deserialize project file '{}'",
            path.display()
        ))?;
        unvalidated.validate(path).await
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

    pub(crate) fn release_version(&self) -> &str {
        self.release_version.as_str()
    }

    pub(crate) fn sdk(&self) -> Result<Option<ImageUri>> {
        if let Some(sdk) = self.sdk.as_ref() {
            let vendor = self.vendor.get(&sdk.vendor).context(format!(
                "vendor '{}' was not specified in Twoliter.toml",
                sdk.vendor
            ))?;
            Ok(Some(ImageUri::new(
                Some(vendor.registry.clone()),
                sdk.name.clone(),
                format!("v{}", sdk.version),
            )))
        } else {
            Ok(None)
        }
    }

    #[allow(unused)]
    pub(crate) fn kit(&self, name: &str) -> Result<Option<ImageUri>> {
        if let Some(kit) = self.kit.iter().find(|y| y.name == name) {
            let vendor = self.vendor.get(&kit.vendor).context(format!(
                "vendor '{}' was not specified in Twoliter.toml",
                kit.vendor
            ))?;
            Ok(Some(ImageUri::new(
                Some(vendor.registry.clone()),
                kit.name.clone(),
                format!("v{}", kit.version),
            )))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn token(&self) -> String {
        let mut d = Sha512::new();
        d.update(self.filepath().display().to_string());
        let digest = hex::encode(d.finalize());
        (digest[..12]).to_string()
    }

    /// Returns a list of the names of Go modules by searching the `sources` directory for `go.mod`
    /// files.
    pub(crate) async fn find_go_modules(&self) -> Result<Vec<String>> {
        let root = self.project_dir.join("sources");
        let mut entries = WalkDir::new(&root);
        let mut modules = Vec::new();
        loop {
            match entries.next().await {
                Some(Ok(entry)) => {
                    if let Some(filename) = entry.path().file_name() {
                        if filename == OsStr::new("go.mod") {
                            let parent_dir = entry
                                .path()
                                .parent()
                                .context(format!(
                                    "Expected the path '{}' to have a parent when searching for \
                                 go modules",
                                    entry.path().display()
                                ))?
                                .to_path_buf();

                            let module_name = parent_dir
                                .file_name()
                                .context(format!(
                                    "Expected to find a module name in path '{}'",
                                    parent_dir.display()
                                ))?
                                .to_str()
                                .context(format!(
                                    "Found non-UTF-8 character in file path '{}'",
                                    parent_dir.display(),
                                ))?
                                .to_string();
                            modules.push(module_name)
                        }
                    }
                }
                Some(Err(e)) => break Err(e).context("Error while searching for go modules"),
                None => break Ok(()),
            }
        }?;
        // Provide a predictable ordering.
        modules.sort();
        Ok(modules)
    }
}

/// This represents a container registry vendor that is used in resolving the kits and also
/// now the bottlerocket sdk
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Ord, PartialOrd)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Vendor {
    pub registry: String,
}

/// This represents a dependency on a container, primarily used for kits
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Ord, PartialOrd)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Image {
    pub name: String,
    pub version: Version,
    pub vendor: String,
}

/// This is used to `Deserialize` a project, then run validation code before returning a valid
/// [`Project`]. This is necessary both because there is no post-deserialization serde hook for
/// validation and, even if there was, we need to know the project directory path in order to check
/// some things.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct UnvalidatedProject {
    schema_version: SchemaVersion<1>,
    release_version: String,
    sdk: Option<Image>,
    vendor: Option<BTreeMap<String, Vendor>>,
    kit: Option<Vec<Image>>,
}

impl UnvalidatedProject {
    /// Constructs a [`Project`] from an [`UnvalidatedProject`] after validating fields.
    async fn validate(self, path: impl AsRef<Path>) -> Result<Project> {
        let filepath: PathBuf = path.as_ref().into();
        let project_dir = filepath
            .parent()
            .context(format!(
                "Unable to find the parent directory of '{}'",
                filepath.display(),
            ))?
            .to_path_buf();

        self.check_vendor_availability().await?;
        self.check_release_toml(&project_dir).await?;

        Ok(Project {
            filepath,
            project_dir,
            schema_version: self.schema_version,
            release_version: self.release_version,
            sdk: self.sdk,
            vendor: self.vendor.unwrap_or_default(),
            kit: self.kit.unwrap_or_default(),
        })
    }

    /// Errors if the user has defined a sdk and/or kit dependency without specifying the associated
    /// vendor
    async fn check_vendor_availability(&self) -> Result<()> {
        let mut dependency_list = self.kit.clone().unwrap_or_default();
        if let Some(sdk) = self.sdk.as_ref() {
            dependency_list.push(sdk.clone());
        }
        for dependency in dependency_list.iter() {
            ensure!(
                self.vendor.is_some()
                    && self
                        .vendor
                        .as_ref()
                        .unwrap()
                        .contains_key(&dependency.vendor),
                "cannot define a dependency on a vendor that is not specified in Twoliter.toml"
            );
        }
        Ok(())
    }

    /// Issues a warning if `Release.toml` is found and, if so, ensures that it contains the same
    /// version (i.e. `release-version`) as the `Twoliter.toml` project file.
    async fn check_release_toml(&self, project_dir: &Path) -> Result<()> {
        let path = project_dir.join("Release.toml");
        if !path.exists() || !path.is_file() {
            // There is no Release.toml file. This is a good thing!
            trace!("This project does not have a Release.toml file (this is not a problem)");
            return Ok(());
        }
        warn!(
            "A Release.toml file was found. Release.toml is deprecated. Please remove it from \
             your project."
        );
        let content = fs::read_to_string(&path).await.context(format!(
            "Error while checking Release.toml file at '{}'",
            path.display()
        ))?;
        let toml: Table = match toml::from_str(&content) {
            Ok(toml) => toml,
            Err(e) => {
                warn!(
                    "Unable to parse Release.toml to ensure that its version matches the \
                     release-version in Twoliter.toml: {e}",
                );
                return Ok(());
            }
        };
        let version = match toml.get("version") {
            Some(version) => version,
            None => {
                info!("Release.toml does not contain a version key. Ignoring it.");
                return Ok(());
            }
        }
        .as_str()
        .context("The version in Release.toml is not a string")?;
        ensure!(
            version == self.release_version,
            "The version found in Release.toml, '{version}', does not match the release-version \
            found in Twoliter.toml '{}'",
            self.release_version
        );
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::common::fs;
    use crate::test::{data_dir, projects_dir};
    use tempfile::TempDir;

    /// Ensure that `Twoliter.toml` can be deserialized.
    #[tokio::test]
    async fn deserialize_twoliter_1_toml() {
        let path = data_dir().join("Twoliter-1.toml");
        let deserialized = Project::load(path).await.unwrap();

        // Add checks here as desired to validate deserialization.
        assert_eq!(SchemaVersion::<1>, deserialized.schema_version);
        assert_eq!(1, deserialized.vendor.len());
        assert!(deserialized.vendor.contains_key("my-vendor"));
        assert_eq!(
            "a.com/b",
            deserialized.vendor.get("my-vendor").unwrap().registry
        );

        let sdk = deserialized.sdk.unwrap();
        assert_eq!("my-bottlerocket-sdk", sdk.name.as_str());
        assert_eq!(Version::new(1, 2, 3), sdk.version);
        assert_eq!("my-vendor", sdk.vendor.as_str());

        assert_eq!(1, deserialized.kit.len());
        assert_eq!("my-core-kit", deserialized.kit[0].name.as_str());
        assert_eq!(Version::new(1, 2, 3), deserialized.kit[0].version);
        assert_eq!("my-vendor", deserialized.kit[0].vendor.as_str());
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
    fn test_sdk_uri() {
        let project = Project {
            filepath: Default::default(),
            project_dir: Default::default(),
            schema_version: Default::default(),
            release_version: String::from("1.0.0"),
            sdk: Some(Image {
                name: "foo-abc".into(),
                version: Version::new(1, 2, 3),
                vendor: "example".into(),
            }),
            vendor: BTreeMap::from([(
                "example".into(),
                Vendor {
                    registry: "example.com".into(),
                },
            )]),
            kit: Vec::new(),
        };

        assert_eq!(
            "example.com/foo-abc:v1.2.3",
            project.sdk().unwrap().unwrap().to_string()
        );
    }

    #[tokio::test]
    async fn test_release_toml_check_error() {
        let tempdir = TempDir::new().unwrap();
        let p = tempdir.path();
        let from = data_dir();
        let twoliter_toml_from = from.join("Twoliter-1.toml");
        let twoliter_toml_to = p.join("Twoliter.toml");
        let release_toml_from = from.join("Release-2.toml");
        let release_toml_to = p.join("Release.toml");
        fs::copy(&twoliter_toml_from, &twoliter_toml_to)
            .await
            .unwrap();
        fs::copy(&release_toml_from, &release_toml_to)
            .await
            .unwrap();
        let result = Project::find_and_load(p).await;
        assert!(
            result.is_err(),
            "Expected the loading of the project to fail because of a mismatched version in \
            Release.toml, but the project loaded without an error."
        );
    }

    #[tokio::test]
    async fn test_vendor_specifications() {
        let project = UnvalidatedProject {
            schema_version: SchemaVersion::default(),
            release_version: "1.0.0".into(),
            sdk: Some(Image {
                name: "bottlerocket-sdk".into(),
                version: Version::new(1, 41, 1),
                vendor: "bottlerocket".into(),
            }),
            vendor: Some(BTreeMap::from([(
                "not-bottlerocket".into(),
                Vendor {
                    registry: "public.ecr.aws/not-bottlerocket".into(),
                },
            )])),
            kit: Some(vec![Image {
                name: "bottlerocket-core-kit".into(),
                version: Version::new(1, 20, 0),
                vendor: "not-bottlerocket".into(),
            }]),
        };
        assert!(project.check_vendor_availability().await.is_err());
    }

    #[tokio::test]
    async fn test_release_toml_check_ok() {
        let tempdir = TempDir::new().unwrap();
        let p = tempdir.path();
        let from = data_dir();
        let twoliter_toml_from = from.join("Twoliter-1.toml");
        let twoliter_toml_to = p.join("Twoliter.toml");
        let release_toml_from = from.join("Release-1.toml");
        let release_toml_to = p.join("Release.toml");
        fs::copy(&twoliter_toml_from, &twoliter_toml_to)
            .await
            .unwrap();
        fs::copy(&release_toml_from, &release_toml_to)
            .await
            .unwrap();

        // The project should load because Release.toml and Twoliter.toml versions match.
        Project::find_and_load(p).await.unwrap();
    }

    #[tokio::test]
    async fn find_go_modules() {
        let twoliter_toml_path = projects_dir().join("project1").join("Twoliter.toml");
        let project = Project::load(twoliter_toml_path).await.unwrap();
        let go_modules = project.find_go_modules().await.unwrap();
        assert_eq!(go_modules.len(), 1, "Expected to find 1 go module");
        assert_eq!(go_modules.first().unwrap(), "hello-go");
    }
}
