use super::build_clean::BuildClean;
use crate::cargo_make::CargoMake;
use crate::common::fs;
use crate::project::{self, Locked};
use crate::tools::install_tools;
use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use tempfile::TempDir;

#[derive(Debug, Parser)]
pub(crate) enum BuildCommand {
    Clean(BuildClean),
    Kit(BuildKit),
    Variant(BuildVariant),
}

impl BuildCommand {
    pub(crate) async fn run(self) -> Result<()> {
        match self {
            BuildCommand::Clean(command) => command.run().await,
            BuildCommand::Kit(command) => command.run().await,
            BuildCommand::Variant(command) => command.run().await,
        }
    }
}

/// Build a Bottlerocket variant image.
#[derive(Debug, Parser)]
pub(crate) struct BuildKit {
    /// Path to Twoliter.toml. Will search for Twoliter.toml when absent.
    #[clap(long = "project-path")]
    pub(crate) project_path: Option<PathBuf>,

    /// The architecture to build for.
    #[clap(long = "arch", default_value = "x86_64")]
    pub(crate) arch: String,

    /// The name of the kit to build.
    pub(crate) kit: String,

    /// The URL to the lookaside cache where sources are stored to avoid pulling them from upstream.
    /// Defaults to https://cache.bottlerocket.aws
    pub(crate) lookaside_cache: Option<String>,

    /// If sources are not found in the lookaside cache, this flag will cause buildsys to pull them
    /// from the upstream URL found in a package's `Cargo.toml`.
    #[clap(long = "upstream-source-fallback")]
    pub(crate) upstream_source_fallback: bool,
}

impl BuildKit {
    pub(super) async fn run(&self) -> Result<()> {
        let project = project::load_or_find_project(self.project_path.clone()).await?;
        let project = project.load_lock::<Locked>().await?;
        let toolsdir = project.project_dir().join("build/tools");
        install_tools(&toolsdir).await?;
        let makefile_path = toolsdir.join("Makefile.toml");

        let mut optional_envs = Vec::new();

        if let Some(lookaside_cache) = &self.lookaside_cache {
            optional_envs.push(("BUILDSYS_LOOKASIDE_CACHE", lookaside_cache))
        }

        CargoMake::new(&project.sdk_image().project_image_uri().to_string())?
            .env("TWOLITER_TOOLS_DIR", toolsdir.display().to_string())
            .env("BUILDSYS_ARCH", &self.arch)
            .env("BUILDSYS_KIT", &self.kit)
            .env("BUILDSYS_VERSION_IMAGE", project.release_version())
            .env("GO_MODULES", project.find_go_modules().await?.join(" "))
            .env(
                "BUILDSYS_UPSTREAM_SOURCE_FALLBACK",
                self.upstream_source_fallback.to_string(),
            )
            .envs(optional_envs.into_iter())
            .makefile(makefile_path)
            .project_dir(project.project_dir())
            .exec("build-kit")
            .await
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

    /// Path to the Infra.toml file
    #[clap(long)]
    infra_toml: Option<PathBuf>,
}

impl BuildVariant {
    pub(super) async fn run(&self) -> Result<()> {
        let project = project::load_or_find_project(self.project_path.clone()).await?;
        let project = project.load_lock::<Locked>().await?;
        let toolsdir = project.project_dir().join("build/tools");
        install_tools(&toolsdir).await?;
        let makefile_path = toolsdir.join("Makefile.toml");
        // A temporary directory in the `build` directory
        let build_temp_dir = TempDir::new_in(project.project_dir())
            .context("Unable to create a tempdir for Twoliter's build")?;
        let packages_dir = build_temp_dir.path().join("sdk_rpms");
        fs::create_dir_all(&packages_dir).await?;

        let mut optional_envs = Vec::new();

        if let Some(lookaside_cache) = &self.lookaside_cache {
            optional_envs.push(("BUILDSYS_LOOKASIDE_CACHE", lookaside_cache.to_string()))
        }

        if let Some(infra_toml) = &self.infra_toml {
            optional_envs.push((
                "PUBLISH_INFRA_CONFIG_PATH",
                infra_toml.display().to_string(),
            ))
        }

        CargoMake::new(&project.sdk_image().project_image_uri().to_string())?
            .env("TWOLITER_TOOLS_DIR", toolsdir.display().to_string())
            .env("BUILDSYS_ARCH", &self.arch)
            .env("BUILDSYS_VARIANT", &self.variant)
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
            .await
    }
}
