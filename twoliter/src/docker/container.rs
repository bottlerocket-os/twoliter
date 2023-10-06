use std::path::PathBuf;

use crate::common::exec;
use anyhow::Result;
use log::warn;
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
        let args = vec!["stop".to_string(), name.to_string()];
        if let Err(e) = exec(Command::new("docker").args(args)).await {
            warn!("Unable to stop container '{name}': {e}")
        }
        let args = vec!["rm".to_string(), name.to_string()];
        if let Err(e) = exec(Command::new("docker").args(args)).await {
            warn!("Unable to remove container '{name}': {e}")
        }

        // Create the new container.
        let args = vec![
            "create".to_string(),
            "--rm".to_string(),
            "--name".to_string(),
            name.to_string(),
            image.to_string(),
        ];

        exec(Command::new("docker").args(args)).await?;
        Ok(Self { name })
    }

    /// Copy the data from this container to a local destination.
    pub(crate) async fn cp(&self, src: &PathBuf, dest: &PathBuf) -> Result<()> {
        let mut args = vec!["cp".to_string()];
        args.push(format!("{}:{}", self.name, src.display()));
        args.push(dest.display().to_string());
        exec(Command::new("docker").args(args)).await
    }
}
