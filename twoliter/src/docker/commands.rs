use crate::common::exec;
use crate::docker::ImageUri;
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::process::Command;

/// Can execute a `docker build` command. This follows the builder pattern, for example:
///
/// ```
/// let build = DockerBuild.dockerfile("./Dockerfile").context(".").execute().await?;
/// ```
pub(crate) struct DockerBuild {
    dockerfile: Option<PathBuf>,
    context_dir: PathBuf,
    tag: Option<ImageUri>,
    build_args: HashMap<String, String>,
}

impl Default for DockerBuild {
    fn default() -> Self {
        Self {
            dockerfile: None,
            context_dir: PathBuf::from("."),
            tag: None,
            build_args: Default::default(),
        }
    }
}

impl DockerBuild {
    /// Add a value for the `--file` argument.
    pub(crate) fn dockerfile<P: Into<PathBuf>>(mut self, dockerfile: P) -> Self {
        self.dockerfile = Some(dockerfile.into());
        self
    }

    /// Required: the directory to be passed to the build as the context.
    pub(crate) fn context_dir<P: Into<PathBuf>>(mut self, context_dir: P) -> Self {
        self.context_dir = context_dir.into();
        self
    }

    /// Add a value for the `--tag` argument.
    pub(crate) fn tag<T: Into<ImageUri>>(mut self, tag: T) -> Self {
        self.tag = Some(tag.into());
        self
    }

    /// Add a build arg, where `("KEY", value)` becomes `--build-arg=KEY=value`.
    pub(crate) fn build_arg<S1, S2>(mut self, key: S1, value: S2) -> Self
    where
        S1: Into<String>,
        S2: Into<String>,
    {
        self.build_args.insert(key.into(), value.into());
        self
    }

    /// Add multiple build args, where `("KEY", value)` becomes `--build-arg=KEY=value`.
    pub(crate) fn _build_args<I: IntoIterator<Item = (String, String)>>(
        mut self,
        build_args: I,
    ) -> Self {
        self.build_args.extend(build_args.into_iter());
        self
    }

    /// Run the `docker build` command.
    pub(crate) async fn execute(self) -> Result<()> {
        let mut args = vec!["build".to_string()];
        if let Some(dockerfile) = self.dockerfile.as_ref() {
            args.push("--file".to_string());
            args.push(dockerfile.display().to_string());
        }
        if let Some(tag) = self.tag.as_ref() {
            args.push("--tag".to_string());
            args.push(tag.uri());
        }
        args.extend(
            self.build_args
                .iter()
                .map(|(k, v)| format!("--build-arg={}={}", k, v)),
        );
        args.push(self.context_dir.display().to_string());
        exec(
            Command::new("docker")
                .args(args.into_iter())
                .env("DOCKER_BUILDKIT", "1"),
        )
        .await
    }
}
