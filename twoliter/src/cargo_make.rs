use std::path::PathBuf;

use anyhow::{bail, Result};
use log::trace;
use tokio::process::Command;

use crate::common::exec;
use crate::docker::ImageArchUri;
use crate::project::Project;

fn require_sdk(project: &Project, arch: &str) -> Result<(ImageArchUri, ImageArchUri)> {
    match (project.sdk(arch), project.toolchain(arch)) {
        (Some(s), Some(t)) => Ok((s, t)),
        _ => bail!(
            "When using twoliter make, it is required that the SDK and toolchain be specified in \
            Twoliter.toml"
        ),
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CargoMake {
    makefile_path: Option<PathBuf>,
    project_dir: Option<PathBuf>,
    args: Vec<String>,
}

impl CargoMake {
    pub(crate) fn new<S>(project: &Project, arch: S) -> Result<Self>
    where
        S: Into<String>,
    {
        let (sdk, toolchain) = require_sdk(&project, &arch.into())?;
        Ok(Self::default()
            .env("TLPRIVATE_SDK_IMAGE", sdk)
            .env("TLPRIVATE_TOOLCHAIN", toolchain))
    }

    pub(crate) fn makefile<P>(mut self, makefile_path: P) -> Self
    where
        P: Into<PathBuf>,
    {
        self.makefile_path = Some(makefile_path.into());
        self
    }

    pub(crate) fn project_dir<P>(mut self, project_dir: P) -> Self
    where
        P: Into<PathBuf>,
    {
        self.project_dir = Some(project_dir.into());
        self
    }

    pub(crate) fn env<S1, S2>(mut self, key: S1, value: S2) -> Self
    where
        S1: Into<String>,
        S2: Into<String>,
    {
        self.args
            .push(format!("-e={}={}", key.into(), value.into()));
        self
    }

    pub(crate) fn envs<S1, S2, V>(mut self, env_vars: V) -> Self
    where
        S1: Into<String>,
        S2: Into<String>,
        V: Into<Vec<(S1, S2)>>,
    {
        for (key, value) in env_vars.into() {
            self.args
                .push(format!("-e={}={}", key.into(), value.into()));
        }
        self
    }

    pub(crate) fn _arg<S>(mut self, arg: S) -> Self
    where
        S: Into<String>,
    {
        self.args.push(arg.into());
        self
    }

    pub(crate) fn _args<V, S>(mut self, args: V) -> Self
    where
        S: Into<String>,
        V: Into<Vec<S>>,
    {
        self.args.extend(args.into().into_iter().map(Into::into));
        self
    }

    pub(crate) async fn exec<S>(&self, task: S) -> Result<()>
    where
        S: Into<String>,
    {
        self.exec_with_args(task, Vec::<String>::new()).await
    }

    pub(crate) async fn exec_with_args<S1, S2, V>(&self, task: S1, args: V) -> Result<()>
    where
        S1: Into<String>,
        S2: Into<String>,
        V: Into<Vec<S2>>,
    {
        exec(
            Command::new("cargo")
                .arg("make")
                .arg("--disable-check-for-updates")
                .args(
                    self.makefile_path
                        .iter()
                        .map(|path| vec!["--makefile".to_string(), path.display().to_string()])
                        .flatten(),
                )
                .args(
                    self.makefile_path
                        .iter()
                        .map(|path| vec!["--cwd".to_string(), path.display().to_string()])
                        .flatten(),
                )
                .args(build_system_env_vars()?)
                .args(&self.args)
                .arg(task.into())
                .args(args.into().into_iter().map(Into::into)),
        )
        .await
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
