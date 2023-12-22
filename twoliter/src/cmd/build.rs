use crate::cargo_make::CargoMake;
use crate::docker::DockerContainer;
use crate::project;
use crate::tools::install_tools;
use anyhow::{Context, Result};
use clap::Parser;
use log::debug;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tokio::fs::{remove_dir_all, remove_file};

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
    #[clap(long = "arch", default_value = "x86_64")]
    arch: String,

    /// The variant to build.
    variant: String,
}

impl BuildVariant {
    pub(super) async fn run(&self) -> Result<()> {
        let project = project::load_or_find_project(self.project_path.clone()).await?;
        let token = project.token();
        let toolsdir = project.project_dir().join("build/tools");
        tokio::fs::remove_dir_all(&toolsdir).await?;
        tokio::fs::create_dir_all(&toolsdir).await?;
        install_tools(&toolsdir).await?;
        let makefile_path = toolsdir.join("Makefile.toml");
        // A temporary directory in the `build` directory
        let build_temp_dir = TempDir::new_in(project.project_dir())
            .context("Unable to create a tempdir for Twoliter's build")?;
        let packages_dir = build_temp_dir.path().join("sdk_rpms");
        fs::create_dir_all(&packages_dir)?;

        let sdk_container = DockerContainer::new(
            format!("sdk-{}", token),
            project
                .sdk(&self.arch)
                .context(format!(
                    "No SDK defined in {} for {}",
                    project.filepath().display(),
                    &self.arch
                ))?
                .uri(),
        )
        .await?;
        sdk_container
            .cp_out(Path::new("twoliter/alpha/build/rpms"), &packages_dir)
            .await?;

        let rpms_dir = project.project_dir().join("build").join("rpms");
        fs::create_dir_all(&rpms_dir)?;
        debug!("Moving rpms to build dir");
        for maybe_file in fs::read_dir(packages_dir.join("rpms"))? {
            let file = maybe_file?;
            debug!("Moving '{}'", file.path().display());
            fs::rename(file.path(), rpms_dir.join(file.file_name()))?;
        }

        let mut created_files = Vec::new();

        let sbkeys_dir = project.project_dir().join("sbkeys");
        if !sbkeys_dir.is_dir() {
            // Create a sbkeys directory in the main project
            debug!("sbkeys dir not found. Creating a temporary directory");
            fs::create_dir_all(&sbkeys_dir)?;
            sdk_container
                .cp_out(
                    Path::new("twoliter/alpha/sbkeys/generate-local-sbkeys"),
                    &sbkeys_dir,
                )
                .await?;
        };

        // TODO: Remove once models is no longer conditionally compiled.
        // Create the models directory for the sdk to mount
        let models_dir = project.project_dir().join("sources/models");
        if !models_dir.is_dir() {
            debug!("models source dir not found. Creating a temporary directory");
            fs::create_dir_all(&models_dir.join("src/variant"))
                .context("Unable to create models source directory")?;
            created_files.push(models_dir)
        }

        // Hold the result of the cargo make call so we can clean up the project directory first.
        let res = CargoMake::new(&project, &self.arch)?
            .env("TWOLITER_TOOLS_DIR", toolsdir.display().to_string())
            .env("BUILDSYS_ARCH", &self.arch)
            .env("BUILDSYS_VARIANT", &self.variant)
            .env("BUILDSYS_SBKEYS_DIR", sbkeys_dir.display().to_string())
            .env("BUILDSYS_VERSION_IMAGE", project.release_version())
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
