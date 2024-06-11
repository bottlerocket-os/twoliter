use crate::lock::Lock;
use crate::project;
use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
pub(crate) struct Update {
    /// Path to Twoliter.toml. Will search for Twoliter.toml when absent
    #[clap(long = "project-path")]
    pub(crate) project_path: Option<PathBuf>,
}

impl Update {
    pub(super) async fn run(&self) -> Result<()> {
        let project = project::load_or_find_project(self.project_path.clone()).await?;
        Lock::create(&project).await?;
        Ok(())
    }
}
