use crate::cargo_make::CargoMake;
use crate::project::{self, Locked};
use crate::tools;
use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
pub(crate) struct BuildClean {
    /// Path to Twoliter.toml. Will search for Twoliter.toml when absent.
    #[clap(long = "project-path")]
    project_path: Option<PathBuf>,
}

impl BuildClean {
    pub(super) async fn run(&self) -> Result<()> {
        let project = project::load_or_find_project(self.project_path.clone()).await?;
        let project = project.load_lock::<Locked>().await?;
        let toolsdir = project.project_dir().join("build/tools");
        tools::install_tools(&toolsdir).await?;
        let makefile_path = toolsdir.join("Makefile.toml");

        CargoMake::new(&project.sdk_image().project_image_uri().to_string())?
            .env("TWOLITER_TOOLS_DIR", toolsdir.display().to_string())
            .makefile(makefile_path)
            .project_dir(project.project_dir())
            .exec("clean")
            .await?;

        Ok(())
    }
}
