use crate::cargo_make::CargoMake;
use crate::project::{self};
use crate::tools::install_tools;
use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

/// Run a cargo make command in Twoliter's build environment. Known Makefile.toml environment
/// variables will be passed-through to the cargo make invocation.
#[derive(Debug, Parser)]
#[clap(trailing_var_arg = true)]
pub(crate) struct Make {
    /// Path to the project file. Will search for Twoliter.toml when absent.
    #[clap(long)]
    project_path: Option<PathBuf>,

    /// Twoliter does not read this from the CARGO_HOME environment variable to avoid any possible
    /// confusion between a CARGO_HOME set on the system, and the path intended for the Bottlerocket
    /// build.
    #[clap(long)]
    cargo_home: PathBuf,

    /// This can be passed by environment variable. We require it as part of the command arguments
    /// because we need it to pull the right SDK target architecture.
    #[clap(long, env = "BUILDSYS_ARCH")]
    arch: String,

    /// Cargo make task. E.g. the word "build" if we want to execute `cargo make build`.
    makefile_task: String,

    /// Uninspected arguments to be passed to cargo make after the target name. For example, --foo
    /// in the following command : cargo make test --foo.
    additional_args: Vec<String>,
}

impl Make {
    pub(super) async fn run(&self) -> Result<()> {
        let project = project::load_or_find_project(self.project_path.clone()).await?;
        let toolsdir = project.project_dir().join("build/tools");
        install_tools(&toolsdir).await?;
        let makefile_path = toolsdir.join("Makefile.toml");
        CargoMake::new(&project, &self.arch)?
            .env("CARGO_HOME", self.cargo_home.display().to_string())
            .env("TWOLITER_TOOLS_DIR", toolsdir.display().to_string())
            .env("BUILDSYS_VERSION_IMAGE", project.release_version())
            .makefile(makefile_path)
            .project_dir(project.project_dir())
            .exec_with_args(&self.makefile_task, self.additional_args.clone())
            .await
    }
}

#[test]
fn test_trailing_args_1() {
    let args = Make::try_parse_from(&[
        "make",
        "--cargo-home",
        "/tmp/foo",
        "--arch",
        "x86_64",
        "testsys",
        "--",
        "add",
        "secret",
        "map",
        "--name",
        "foo",
        "something=bar",
        "something-else=baz",
    ])
    .unwrap();

    assert_eq!(args.makefile_task, "testsys");
    assert_eq!(args.additional_args[0], "add");
    assert_eq!(args.additional_args[1], "secret");
    assert_eq!(args.additional_args[2], "map");
    assert_eq!(args.additional_args[3], "--name");
    assert_eq!(args.additional_args[4], "foo");
    assert_eq!(args.additional_args[5], "something=bar");
    assert_eq!(args.additional_args[6], "something-else=baz");
}

#[test]
fn test_trailing_args_2() {
    let args = Make::try_parse_from(&[
        "make",
        "--cargo-home",
        "/tmp/foo",
        "--arch",
        "x86_64",
        "testsys",
        "add",
        "secret",
        "map",
        "--name",
        "foo",
        "something=bar",
        "something-else=baz",
    ])
    .unwrap();

    assert_eq!(args.makefile_task, "testsys");
    assert_eq!(args.additional_args[0], "add");
    assert_eq!(args.additional_args[1], "secret");
    assert_eq!(args.additional_args[2], "map");
    assert_eq!(args.additional_args[3], "--name");
    assert_eq!(args.additional_args[4], "foo");
    assert_eq!(args.additional_args[5], "something=bar");
    assert_eq!(args.additional_args[6], "something-else=baz");
}

#[test]
fn test_trailing_args_3() {
    let args = Make::try_parse_from(&[
        "make",
        "--cargo-home",
        "/tmp/foo",
        "--arch",
        "x86_64",
        "testsys",
        "--",
        "add",
        "secret",
        "map",
        "--",
        "--name",
        "foo",
        "something=bar",
        "something-else=baz",
        "--",
    ])
    .unwrap();

    assert_eq!(args.makefile_task, "testsys");
    assert_eq!(args.additional_args[0], "add");
    assert_eq!(args.additional_args[1], "secret");
    assert_eq!(args.additional_args[2], "map");
    // The first instance of `--`, between `testsys` and `add`, is not passed through to the
    // varargs. After that, instances of `--` are passed through the varargs.
    assert_eq!(args.additional_args[3], "--");
    assert_eq!(args.additional_args[4], "--name");
    assert_eq!(args.additional_args[5], "foo");
    assert_eq!(args.additional_args[6], "something=bar");
    assert_eq!(args.additional_args[7], "something-else=baz");
    assert_eq!(args.additional_args[8], "--");
}
