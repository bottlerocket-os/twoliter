use crate::docker;
use crate::project::{Project, Sdk};
use anyhow::Result;
use clap::Parser;
use log::debug;
use std::path::PathBuf;

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
}

impl BuildVariant {
    pub(super) async fn run(&self) -> Result<()> {
        let _project = match &self.project_path {
            None => {
                let project = Project::find_and_load(".").await?;
                debug!(
                    "Project file loaded from '{}'",
                    project.filepath().display()
                );
                project
            }
            Some(p) => Project::load(p).await?,
        };
        // TODO - get smart about sdk: https://github.com/bottlerocket-os/twoliter/issues/11
        let sdk = Sdk::default();
        let _ = docker::create_twoliter_image_if_not_exists(&sdk.uri(&self.arch)).await?;
        Ok(())
    }
}
