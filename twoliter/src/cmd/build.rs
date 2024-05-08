use super::build_clean::BuildClean;
use crate::cargo_make::CargoMake;
use crate::common::fs;
use crate::docker::DockerContainer;
use crate::project;
use crate::tools::install_tools;
use anyhow::{Context, Result};
use clap::Parser;
use log::debug;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

#[derive(Debug, Parser)]
pub(crate) enum BuildCommand {
    Clean(BuildClean),
    Variant(BuildVariant),
}

impl BuildCommand {
    pub(crate) async fn run(self) -> Result<()> {
        match self {
            BuildCommand::Clean(command) => command.run().await,
            BuildCommand::Variant(command) => command.run().await,
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

    /// The URL to the lookaside cache where sources are stored to avoid pulling them from upstream.
    /// Defaults to https://cache.bottlerocket.aws
    lookaside_cache: Option<String>,

    /// If sources are not found in the lookaside cache, this flag will cause buildsys to pull them
    /// from the upstream URL found in a package's `Cargo.toml`.
    #[clap(long = "upstream-source-fallback")]
    upstream_source_fallback: bool,
}

impl BuildVariant {
    pub(super) async fn run(&self) -> Result<()> {
        let project = project::load_or_find_project(self.project_path.clone()).await?;
        let token = project.token();
        let toolsdir = project.project_dir().join("build/tools");
        install_tools(&toolsdir).await?;
        let makefile_path = toolsdir.join("Makefile.toml");
        // A temporary directory in the `build` directory
        let build_temp_dir = TempDir::new_in("/tmp")
            .context("Unable to create a tempdir for Twoliter's build")?;
        let packages_dir = build_temp_dir.path().join("sdk_rpms");
        fs::create_dir_all(&packages_dir).await?;

        let sdk_container = DockerContainer::new(
            format!("sdk-{}", token),
            project
                .sdk()
                .context(format!(
                    "No SDK defined in {}",
                    project.filepath().display(),
                ))?
                .uri(),
        )
        .await?;
        sdk_container
            .cp_out(Path::new("twoliter/alpha/build/rpms"), &packages_dir)
            .await?;

        let rpms_dir = project.project_dir().join("build").join("rpms");
        fs::create_dir_all(&rpms_dir).await?;
        debug!("Moving rpms to build dir");
        let rpms = packages_dir.join("rpms");
        let mut read_dir = tokio::fs::read_dir(&rpms)
            .await
            .context(format!("Unable to read dir '{}'", rpms.display()))?;
        while let Some(entry) = read_dir.next_entry().await.context(format!(
            "Error while reading entries in dir '{}'",
            rpms.display()
        ))? {
            debug!("Moving '{}'", entry.path().display());
            fs::rename(entry.path(), rpms_dir.join(entry.file_name())).await?;
        }

        let mut created_files = Vec::new();

        let sbkeys_dir = project.project_dir().join("sbkeys");
        if !sbkeys_dir.is_dir() {
            // Create a sbkeys directory in the main project
            debug!("sbkeys dir not found. Creating a temporary directory");
            fs::create_dir_all(&sbkeys_dir).await?;
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
                .await
                .context("Unable to create models source directory")?;
            created_files.push(models_dir)
        }

        let mut optional_envs = Vec::new();

        if let Some(lookaside_cache) = &self.lookaside_cache {
            optional_envs.push(("BUILDSYS_LOOKASIDE_CACHE", lookaside_cache))
        }

        // Hold the result of the cargo make call so we can clean up the project directory first.
        let res = CargoMake::new(&project)?
            .env("TWOLITER_TOOLS_DIR", toolsdir.display().to_string())
            .env("BUILDSYS_ARCH", &self.arch)
            .env("BUILDSYS_VARIANT", &self.variant)
            .env("BUILDSYS_SBKEYS_DIR", sbkeys_dir.display().to_string())
            .env("BUILDSYS_VERSION_IMAGE", project.release_version())
            .env("GO_MODULES", project.find_go_modules().await?.join(" "))
            .env(
                "BUILDSYS_UPSTREAM_SOURCE_FALLBACK",
                self.upstream_source_fallback.to_string(),
            )
            .envs(optional_envs.into_iter())
            .makefile(makefile_path)
            .project_dir(project.project_dir())
            .exec("build")
            .await;

        // Clean up all of the files we created
        for file_name in created_files {
            let added = Path::new(&file_name);
            if added.is_file() {
                fs::remove_file(added).await?;
            } else if added.is_dir() {
                fs::remove_dir_all(added).await?;
            }
        }

        res
    }
}
