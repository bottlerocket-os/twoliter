use crate::common::exec;
use anyhow::Result;
use log::{debug, log, Level};
use std::path::Path;
use tokio::process::Command;

pub(crate) struct DockerContainer {
    name: String,
}

impl DockerContainer {
    /// Create a docker image with the given name from the image by using `docker create`.
    pub(crate) async fn new<S1, S2>(container_name: S1, image: S2) -> Result<Self>
    where
        S1: Into<String>,
        S2: Into<String>,
    {
        let name = container_name.into();
        let image = image.into();

        // Make sure previous versions of this container are stopped deleted.
        cleanup_container(&name, Level::Trace).await;

        debug!("Creating docker container '{name}' from image '{image}'");

        // Create the new container.
        let args = vec![
            "create".to_string(),
            "--rm".to_string(),
            "--name".to_string(),
            name.to_string(),
            image.to_string(),
        ];

        exec(Command::new("docker").args(args), true).await?;
        Ok(Self { name })
    }

    /// Copy the data from this container to a local destination.
    pub(crate) async fn cp_out<P1, P2>(&self, src: P1, dest: P2) -> Result<()>
    where
        P1: AsRef<Path>,
        P2: AsRef<Path>,
    {
        debug!(
            "Copying '{}' from '{}' to '{}'",
            src.as_ref().display(),
            self.name,
            dest.as_ref().display()
        );
        let mut args = vec!["cp".to_string()];
        args.push(format!("{}:{}", self.name, src.as_ref().display()));
        args.push(dest.as_ref().display().to_string());
        exec(Command::new("docker").args(args), true).await?;
        Ok(())
    }
}

impl Drop for DockerContainer {
    fn drop(&mut self) {
        let name = self.name.clone();
        tokio::task::spawn(async move { cleanup_container(&name, Level::Error).await });
    }
}

async fn cleanup_container(name: &str, log_level: Level) {
    let args = vec!["stop".to_string(), name.to_string()];
    if let Err(e) = exec(Command::new("docker").args(args), true).await {
        log!(log_level, "Unable to stop container '{}': {e}", name)
    }
    let args = vec!["rm".to_string(), name.to_string()];
    if let Err(e) = exec(Command::new("docker").args(args), true).await {
        log!(log_level, "Unable to remove container '{}': {e}", name)
    }
}
