use crate::lock::Lock;
use crate::project;
use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
pub(crate) struct Fetch {
    /// Path to Twoliter.toml. Will search for Twoliter.toml when absent
    #[clap(long = "project-path")]
    project_path: Option<PathBuf>,

    #[clap(long = "arch", default_value = "x86_64")]
    arch: String,
}

impl Fetch {
    pub(super) async fn run(&self) -> Result<()> {
        let project = project::load_or_find_project(self.project_path.clone()).await?;
        let lock_file = Lock::load(&project).await?;
        lock_file.fetch(&project, self.arch.as_str()).await?;
        Ok(())
    }
}
