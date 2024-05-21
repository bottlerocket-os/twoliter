use crate::project;
use crate::tools::install_tools;
use anyhow::{ensure, Context, Result};
use clap::Parser;
use std::path::PathBuf;
use tokio::process::Command;

/// Group all publish commands
#[derive(Debug, Parser)]
pub(crate) enum PublishCommand {
    Kit(PublishKit),
}

impl PublishCommand {
    pub(crate) async fn run(self) -> Result<()> {
        match self {
            PublishCommand::Kit(command) => command.run().await,
        }
    }
}

/// Publish a local kit to a container registry
#[derive(Debug, Parser)]
pub(crate) struct PublishKit {
    /// Path to Twoliter.toml. Will search for Twoliter.toml when absent
    #[clap(long = "project-path")]
    project_path: Option<PathBuf>,
    /// Kit name to build
    kit_name: String,
    /// Vendor to publish to
    vendor: String,
}

impl PublishKit {
    pub(super) async fn run(&self) -> Result<()> {
        let project = project::load_or_find_project(self.project_path.clone()).await?;
        let toolsdir = project.project_dir().join("build/tools");
        install_tools(&toolsdir).await?;
        let pubsys_path = toolsdir.join("pubsys");
        let infra_path = project.project_dir().join("Infra.toml");
        let kit_path = project
            .project_dir()
            .join("build")
            .join("rpms")
            .join(self.kit_name.as_str());

        let infra_arg = format!("--infra-config-path={}", infra_path.display());
        let kit_arg = format!("--kit-path={}", kit_path.display());
        let vendor_arg = format!("--vendor={}", self.vendor);
        // Now we want to offload this operation to pubsys
        let res = Command::new(pubsys_path)
            .args([
                infra_arg.as_str(),
                "publish-kit",
                kit_arg.as_str(),
                vendor_arg.as_str(),
            ])
            .spawn()
            .context("failed to spawn pubsys")?
            .wait()
            .await
            .context("failed to publish a kit with pubsys")?;
        ensure!(res.success(), "failed to publish a kit with pubsys");
        Ok(())
    }
}
