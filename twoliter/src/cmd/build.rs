use crate::cargo_make::CargoMake;
use crate::docker::DockerContainer;
use crate::project;
use crate::tools::{install_tools, tools_tempdir};
use anyhow::{Context, Result};
use clap::Parser;
use log::{debug, trace};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tokio::fs::{create_dir, remove_dir_all, remove_file};

const LICENSE_FILES: &[&str] = &["LICENSE-APACHE", "LICENSE-MIT", "COPYRIGHT"];
const ALPHA_INFRA: &[&str] = &[
    "sbkeys/generate-local-sbkeys",
    "sbkeys/generate-aws-sbkeys",
    "sources/logdog",
    "sources/models",
];

#[derive(Debug, Parser)]
pub(crate) enum BuildCommand {
    Variant(BuildVariant),
}

impl BuildCommand {
    pub(crate) async fn run(self) -> Result<()> {
        match self {
            BuildCommand::Variant(build_variant) => build_variant.run().await,
        }
    }
}

/// Build a Bottlerocket variant image.
#[derive(Debug, Parser)]
pub(crate) struct BuildVariant {
    /// Path to Twoliter.toml. Will search for Twoliter.toml when absent.
    #[clap(long = "project-path")]
    project_path: Option<PathBuf>,

    /// The architecture to build for.
    #[clap(long = "arch", env = "BUILDSYS_ARCH", default_value = "x86_64")]
    arch: String,

    /// The variant to build.
    #[clap(env = "BUILDSYS_VARIANT")]
    variant: String,

    /// The go modules that should be build
    #[clap(long = "go-modules", env = "GO_MODULES", default_value = "")]
    go_modules: String,
}

impl BuildVariant {
    pub(super) async fn run(&self) -> Result<()> {
        let project = project::load_or_find_project(self.project_path.clone()).await?;
        let tempdir = tools_tempdir()?;
        install_tools(&tempdir).await?;
        let makefile_path = tempdir.path().join("Makefile.toml");
        let packages_dir =
            TempDir::new().context("Unable to create a tempdir for Twoliter's packages")?;

        let sdk_container = DockerContainer::new(
            "sdk",
            project
                .sdk(&self.arch)
                .context("The project was missing an sdk")?
                .uri(),
        )
        .await?;
        sdk_container
            .cp(
                &"twoliter/alpha/build/rpms".into(),
                &packages_dir.path().into(),
            )
            .await?;

        let rpms_dir = project.project_dir().join("build").join("rpms");
        fs::create_dir_all(&rpms_dir)?;
        
        for maybe_file in fs::read_dir(packages_dir.path().join("rpms"))? {
            let file = maybe_file?;
            if !file.file_type()?.is_file() {
                debug!("Skipping '{}'", file.path().display());
            }
            debug!("Copying '{}'", file.path().display());
            fs::copy(file.path(), rpms_dir.join(file.file_name()))?;
        }

        // Create the sbkeys directory
        if !Path::new("sbkeys").is_dir() {
            create_dir("sbkeys").await?;
        }

        let mut created_files = cp_licenses(&sdk_container).await?;
        created_files.extend(cp_alpha_files(&sdk_container).await?);

        let res = CargoMake::new(&project, &self.arch)?
            .env("TWOLITER_TOOLS_DIR", tempdir.path().display().to_string())
            .env("GO_MODULES", &self.go_modules)
            .makefile(makefile_path)
            .project_dir(project.project_dir())
            .exec("build")
            .await;

        // Clean up all of the files we created
        for file_name in created_files {
            let added = Path::new(&file_name);
            if added.is_file() {
                remove_file(added).await?;
            } else if added.is_dir() {
                remove_dir_all(added).await?;
            }
        }

        res
    }
}

// Make sure the build system has all required licenses
async fn cp_licenses(sdk_container: &DockerContainer) -> Result<Vec<String>> {
    let mut created_files = Vec::new();
    for license in LICENSE_FILES {
        if Path::new(license).is_file() {
            trace!("License file '{license}' already exists. Skipping");
            continue;
        }
        sdk_container
            .cp(
                &Path::new("twoliter/alpha/licenses").join(license).into(),
                &license.into(),
            )
            .await?;
        created_files.push(license.to_string())
    }
    Ok(created_files)
}

// Make sure the alpha build system has all required files
async fn cp_alpha_files(sdk_container: &DockerContainer) -> Result<Vec<String>> {
    let mut created_files = Vec::new();
    for file_name in ALPHA_INFRA {
        if Path::new(file_name).is_file() {
            trace!("Alpha file '{file_name}' already exists. Skipping");
            continue;
        }
        if Path::new(file_name).is_dir() {
            trace!("Alpha directory '{file_name}' already exists. Skipping");
            continue;
        }
        sdk_container
            .cp(
                &Path::new("twoliter/alpha/").join(file_name).into(),
                &file_name.into(),
            )
            .await?;
        created_files.push(file_name.to_string())
    }
    Ok(created_files)
}
