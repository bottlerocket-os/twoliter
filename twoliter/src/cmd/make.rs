use crate::common::exec;
use crate::project;
use crate::tools::{install_tools, tools_tempdir};
use anyhow::Result;
use clap::Parser;
use log::trace;
use std::path::PathBuf;
use tokio::process::Command;

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
}

impl Make {
    pub(super) async fn run(&self) -> Result<()> {
        let project = project::load_or_find_project(self.project_path.clone()).await?;
        let tempdir = tools_tempdir()?;
        install_tools(&tempdir).await?;
        let makefile_path = tempdir.path().join("Makefile.toml");

        let mut args = vec![
            "make".to_string(),
            "--disable-check-for-updates".to_string(),
            "--makefile".to_string(),
            makefile_path.display().to_string(),
            "--cwd".to_string(),
            project.project_dir().display().to_string(),
        ];

        for (key, val) in std::env::vars() {
            if is_build_system_env(key.as_str()) {
                trace!("Passing env var {} to cargo make", key);
                args.push("-e".to_string());
                args.push(format!("{}={}", key, val));
            }
        }

        args.push(format!("-e=CARGO_HOME={}", self.cargo_home.display()));
        args.push(format!(
            "-e=TWOLITER_TOOLS_DIR={}",
            tempdir.path().display()
        ));

        args.push(self.makefile_task.clone());

        for cargo_make_arg in &self.additional_args {
            args.push(cargo_make_arg.clone());
        }

        exec(Command::new("cargo").args(args)).await
    }
}

/// A list of environment variables that don't conform to naming conventions but need to be passed
/// through to the `cargo make` invocation.
const ENV_VARS: [&str; 12] = [
    "ALLOW_MISSING_KEY",
    "AMI_DATA_FILE_SUFFIX",
    "CARGO_MAKE_CARGO_ARGS",
    "CARGO_MAKE_CARGO_LIMIT_JOBS",
    "CARGO_MAKE_DEFAULT_TESTSYS_KUBECONFIG_PATH",
    "CARGO_MAKE_TESTSYS_ARGS",
    "CARGO_MAKE_TESTSYS_KUBECONFIG_ARG",
    "MARK_OVA_AS_TEMPLATE",
    "RELEASE_START_TIME",
    "SSM_DATA_FILE_SUFFIX",
    "VMWARE_IMPORT_SPEC_PATH",
    "VMWARE_VM_NAME_DEFAULT",
];

/// Returns `true` if `key` is an environment variable that needs to be passed to `cargo make`.
fn is_build_system_env(key: impl AsRef<str>) -> bool {
    let key = key.as_ref();
    key.starts_with("BUILDSYS_")
        || key.starts_with("PUBLISH_")
        || key.starts_with("REPO_")
        || key.starts_with("TESTSYS_")
        || key.starts_with("BOOT_CONFIG")
        || key.starts_with("AWS_")
        || ENV_VARS.contains(&key)
}

#[test]
fn test_is_build_system_env() {
    assert!(is_build_system_env(
        "CARGO_MAKE_DEFAULT_TESTSYS_KUBECONFIG_PATH"
    ));
    assert!(is_build_system_env("BUILDSYS_PRETTY_NAME"));
    assert!(is_build_system_env("PUBLISH_FOO_BAR"));
    assert!(is_build_system_env("TESTSYS_!"));
    assert!(is_build_system_env("BOOT_CONFIG!"));
    assert!(is_build_system_env("BOOT_CONFIG_INPUT"));
    assert!(is_build_system_env("AWS_REGION"));
    assert!(!is_build_system_env("PATH"));
    assert!(!is_build_system_env("HOME"));
    assert!(!is_build_system_env("COLORTERM"));
}
