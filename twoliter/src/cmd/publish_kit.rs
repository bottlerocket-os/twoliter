use crate::cargo_make::CargoMake;
use crate::project::{self, Locked};
use crate::tools::install_tools;
use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

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

    /// Publish kit image to a different repository than the kit's name
    kit_repo: Option<String>,
}

impl PublishKit {
    pub(super) async fn run(&self) -> Result<()> {
        let project = project::load_or_find_project(self.project_path.clone()).await?;
        let project = project.load_lock::<Locked>().await?;
        let toolsdir = project.project_dir().join("build/tools");
        install_tools(&toolsdir).await?;
        let makefile_path = toolsdir.join("Makefile.toml");

        let publish_kit_repo = match &self.kit_repo {
            Some(kit_repo) => kit_repo,
            None => &self.kit_name,
        };
        CargoMake::new(project.sdk_image().project_image_uri().to_string().as_str())?
            .env("TWOLITER_TOOLS_DIR", toolsdir.display().to_string())
            .env("BUILDSYS_KIT", &self.kit_name)
            .env("BUILDSYS_VERSION_IMAGE", project.release_version())
            .env("PUBLISH_VENDOR", &self.vendor)
            .env("PUBLISH_KIT_REPO", publish_kit_repo)
            .makefile(makefile_path)
            .project_dir(project.project_dir())
            .exec("publish-kit")
            .await
    }
}
