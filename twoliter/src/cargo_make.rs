use crate::common::exec_log;
use crate::docker::ImageArchUri;
use crate::project::Project;
use anyhow::{bail, Result};
use log::trace;
use std::path::PathBuf;
use tokio::process::Command;

/// A struct used to invoke `cargo make` tasks with `twoliter`'s `Makefile.toml`.
/// ```rust
/// # use crate::project::Project;
/// # use crate::test::data_dir;
/// # use self::CargoMake;
/// # let project_path = data_dir().join("Twoliter-1.toml");
/// # let makefile_path = data_dir().join("Makefile.toml");
/// # let project_dir = data_dir();
///
/// // First create a twoliter project.
/// let project = Project::load(project_path).await.unwrap();
/// // Add the architecture that cargo make will be invoked for.
/// // This is required so that the correct sdk and toolchain are selected.
/// let arch = "x86_64";
///
/// // Create the `cargo make` command.
/// let cargo_make_command = CargoMake::new(&project, arch)
///     .unwrap()
///     // Specify path to the `Makefile.toml` (Default: `Makefile.toml`)
///     .makefile(makefile_path)
///     // Specify the project directory (Default: `.`)
///     .project_dir(project_dir)
///     // Add environment variable to the command
///     .env("FOO", "bar")
///     // Add cargo make arguments such as `-q` (quiet)
///     ._arg("-q");
///
/// // Run the `cargo make` task
/// cargo_make_command.clone()
///     ._exec("verify-twoliter-env")
///     .await
///     .unwrap();
///
/// // Run the `cargo make` task with args
/// cargo_make_command
///     .exec_with_args("verify-env-set-with-arg", ["FOO"])
///     .await
///     .unwrap();
/// ```
#[derive(Debug, Clone, Default)]
pub struct CargoMake {
    makefile_path: Option<PathBuf>,
    project_dir: Option<PathBuf>,
    args: Vec<String>,
}

impl CargoMake {
    /// Create a new `cargo make` command. The sdk and toolchain environment variables will be set
    /// based on the definitions in `Twoliter.toml` and `arch`.
    pub(crate) fn new<S>(project: &Project, arch: S) -> Result<Self>
    where
        S: Into<String>,
    {
        let (sdk, toolchain) = require_sdk(project, &arch.into())?;
        Ok(Self::default()
            .env("TLPRIVATE_SDK_IMAGE", sdk)
            .env("TLPRIVATE_TOOLCHAIN", toolchain))
    }

    /// Specify the path to the `Makefile.toml` for the `cargo make` command
    pub(crate) fn makefile<P>(mut self, makefile_path: P) -> Self
    where
        P: Into<PathBuf>,
    {
        self.makefile_path = Some(makefile_path.into());
        self
    }

    /// Specify the project directory for the `cargo make` command
    pub(crate) fn project_dir<P>(mut self, project_dir: P) -> Self
    where
        P: Into<PathBuf>,
    {
        self.project_dir = Some(project_dir.into());
        self
    }

    /// Specify environment variables that should be applied for this comand
    pub(crate) fn env<S1, S2>(mut self, key: S1, value: S2) -> Self
    where
        S1: Into<String>,
        S2: Into<String>,
    {
        self.args
            .push(format!("-e={}={}", key.into(), value.into()));
        self
    }

    /// Execute the `cargo make` task
    pub(crate) async fn exec<S>(&self, task: S) -> Result<()>
    where
        S: Into<String>,
    {
        self.exec_with_args(task, Vec::<String>::new()).await
    }

    /// Execute the `cargo make` task with arguments provided
    pub(crate) async fn exec_with_args<S1, S2, I>(&self, task: S1, args: I) -> Result<()>
    where
        S1: Into<String>,
        S2: Into<String>,
        I: IntoIterator<Item = S2>,
    {
        exec_log(
            Command::new("cargo")
                .arg("make")
                .arg("--disable-check-for-updates")
                .args(
                    self.makefile_path.iter().flat_map(|path| {
                        vec!["--makefile".to_string(), path.display().to_string()]
                    }),
                )
                .args(
                    self.project_dir
                        .iter()
                        .flat_map(|path| vec!["--cwd".to_string(), path.display().to_string()]),
                )
                .args(build_system_env_vars()?)
                .args(&self.args)
                .arg(task.into())
                .args(args.into_iter().map(Into::into)),
        )
        .await
    }
}

fn require_sdk(project: &Project, arch: &str) -> Result<(ImageArchUri, ImageArchUri)> {
    match (project.sdk(arch), project.toolchain(arch)) {
        (Some(s), Some(t)) => Ok((s, t)),
        _ => bail!(
            "When using twoliter make, it is required that the SDK and toolchain be specified in \
            Twoliter.toml"
        ),
    }
}

fn build_system_env_vars() -> Result<Vec<String>> {
    let mut args = Vec::new();
    for (key, val) in std::env::vars() {
        if is_build_system_env(key.as_str()) {
            trace!("Passing env var {} to cargo make", key);
            args.push("-e".to_string());
            args.push(format!("{}={}", key, val));
        }

        // To avoid confusion, environment variables whose values have been moved to
        // Twoliter.toml are expressly disallowed here.
        check_for_disallowed_var(&key)?;
    }
    Ok(args)
}

/// A list of environment variables that don't conform to naming conventions but need to be passed
/// through to the `cargo make` invocation.
const ENV_VARS: [&str; 13] = [
    "ALLOW_MISSING_KEY",
    "AMI_DATA_FILE_SUFFIX",
    "CARGO_MAKE_CARGO_ARGS",
    "CARGO_MAKE_CARGO_LIMIT_JOBS",
    "CARGO_MAKE_DEFAULT_TESTSYS_KUBECONFIG_PATH",
    "CARGO_MAKE_TESTSYS_ARGS",
    "CARGO_MAKE_TESTSYS_KUBECONFIG_ARG",
    "GO_MODULES",
    "MARK_OVA_AS_TEMPLATE",
    "RELEASE_START_TIME",
    "SSM_DATA_FILE_SUFFIX",
    "VMWARE_IMPORT_SPEC_PATH",
    "VMWARE_VM_NAME_DEFAULT",
];

const DISALLOWED_SDK_VARS: [&str; 4] = [
    "BUILDSYS_SDK_NAME",
    "BUILDSYS_SDK_VERSION",
    "BUILDSYS_REGISTRY",
    "BUILDSYS_TOOLCHAIN",
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

fn check_for_disallowed_var(key: &str) -> Result<()> {
    if DISALLOWED_SDK_VARS.contains(&key) {
        bail!(
            "The environment variable '{}' can no longer be used. Specify the SDK in Twoliter.toml",
            key
        )
    }
    Ok(())
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
    assert!(is_build_system_env("GO_MODULES"));
    assert!(is_build_system_env("AWS_REGION"));
    assert!(!is_build_system_env("PATH"));
    assert!(!is_build_system_env("HOME"));
    assert!(!is_build_system_env("COLORTERM"));
}

#[test]
fn test_check_for_disallowed_var() {
    assert!(check_for_disallowed_var("BUILDSYS_REGISTRY").is_err());
    assert!(check_for_disallowed_var("BUILDSYS_PRETTY_NAME").is_ok());
}
