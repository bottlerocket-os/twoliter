use crate::cargo_make::CargoMake;
use crate::project;
use crate::tools::{install_tools, tools_tempdir};
use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

/// Run a cargo make command in Twoliter's build environment. Known Makefile.toml environment
/// variables will be passed-through to the cargo make invocation.
#[derive(Debug, Parser)]
pub(crate) struct Make {
    /// Path to the project file. Will search for Twoliter.toml when absent.
    #[clap(long)]
    project_path: Option<PathBuf>,

    /// Twoliter does not read this from the CARGO_HOME environment variable to avoid any possible
    /// confusion between a CARGO_HOME set on the system, and the path intended for the Bottlerocket
    /// build.
    #[clap(long)]
    cargo_home: PathBuf,

    /// Cargo make task. E.g. the word "build" if we want to execute `cargo make build`.
    makefile_task: String,

    /// Uninspected arguments to be passed to cargo make after the target name. For example, --foo
    /// in the following command : cargo make test --foo.
    additional_args: Vec<String>,

    #[clap(env = "BUILDSYS_ARCH")]
    arch: String,
}

impl Make {
    pub(super) async fn run(&self) -> Result<()> {
        let project = project::load_or_find_project(self.project_path.clone()).await?;
        let tempdir = tools_tempdir()?;
        install_tools(&tempdir).await?;
        let makefile_path = tempdir.path().join("Makefile.toml");
        CargoMake::new(&project, &self.arch)?
            .env("CARGO_HOME", self.cargo_home.display().to_string())
            .env("TWOLITER_TOOLS_DIR", tempdir.path().display().to_string())
            .env("BUILDSYS_VERSION_IMAGE", project.release_version())
            .makefile(makefile_path)
            .project_dir(project.project_dir())
            .exec_with_args(&self.makefile_task, self.additional_args.clone())
            .await
    }
}
