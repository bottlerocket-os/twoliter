use crate::common::fs;
use crate::tools::install_tools;
use anyhow::Result;
use clap::Parser;
use std::env;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Parser)]
pub(crate) struct Debug {
    #[clap(subcommand)]
    debug_action: DebugAction,
}

#[derive(Debug, Clone, Parser)]
pub(crate) enum DebugAction {
    CheckTools(CheckToolArgs),
}

impl DebugAction {
    pub(crate) async fn run(&self) -> Result<()> {
        match self {
            DebugAction::CheckTools(c) => c.run().await,
        }
    }
}

/// Installs the tools into a directory and leaves them there for further inspection. This is useful
/// for troubleshooting a problem with the tools because during normal execution flow the tools are
/// cleaned up before Twoliter exits.
#[derive(Debug, Default, Clone, Parser)]
pub(crate) struct CheckToolArgs {
    /// The directory where the tools will be installed (and left behind for your further
    /// inspection). If not specified, a directory in the tempdir will be used. The directory will
    /// be created if it does not exist. Outputs the name of the directory to stdout.
    #[clap(long)]
    install_dir: Option<PathBuf>,
}

fn unique_name() -> String {
    let uuid = format!("{}", Uuid::new_v4());
    let slug = &uuid[0..8];
    format!("twoliter-tools-{}", slug)
}

impl CheckToolArgs {
    pub(crate) async fn run(&self) -> Result<()> {
        let dir = self
            .install_dir
            .clone()
            .unwrap_or_else(|| env::temp_dir().join(unique_name()));
        fs::create_dir_all(&dir).await?;
        install_tools(&dir).await?;
        println!("{}", dir.display());
        Ok(())
    }
}
