use crate::cargo_make::CargoMake;
use crate::project;
use crate::tools::{install_tools, tools_tempdir};
use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
pub(crate) enum PublishCommand {
    Ami(PublishAmi),
}

impl PublishCommand {
    pub(crate) async fn run(self) -> Result<()> {
        match self {
            PublishCommand::Ami(publish_ami) => publish_ami.run().await,
        }
    }
}

/// Publish a Bottlerocket variant image.
#[derive(Debug, Parser)]
pub(crate) struct PublishAmi {
    /// Path to Twoliter.toml. Will search for Twoliter.toml when absent.
    #[clap(long, env = "TWOLITER_PROJECT")]
    project_path: Option<PathBuf>,

    /// The architecture to publish.
    #[clap(long, env = "BUILDSYS_ARCH", default_value = "x86_64")]
    arch: String,

    /// The variant to publish.
    #[clap(env = "BUILDSYS_VARIANT")]
    variant: String,

    /// Path to Infra.toml
    #[clap(long, env = "PUBLISH_INFRA_CONFIG_PATH", default_value = "Infra.toml")]
    infra_config_path: String,
}

impl PublishAmi {
    pub(super) async fn run(&self) -> Result<()> {
        let project = project::load_or_find_project(self.project_path.clone()).await?;
        let tempdir = tools_tempdir()?;
        install_tools(&tempdir).await?;
        let makefile_path = tempdir.path().join("Makefile.toml");
        CargoMake::new(&project, &self.arch)?
            .env("TWOLITER_TOOLS_DIR", tempdir.path().display().to_string())
            .env("PUBLISH_INFRA_CONFIG_PATH", &self.infra_config_path)
            .makefile(makefile_path)
            .project_dir(project.project_dir())
            .exec("ami")
            .await
    }
}
