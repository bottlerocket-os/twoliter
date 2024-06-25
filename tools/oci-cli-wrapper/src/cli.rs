use snafu::{ensure, ResultExt};
use std::path::PathBuf;
use tokio::process::Command;

use crate::{error, Result};

pub(crate) struct CommandLine {
    pub(crate) path: PathBuf,
}

impl CommandLine {
    pub(crate) async fn output(&self, args: &[&str], error_msg: String) -> Result<Vec<u8>> {
        let output = Command::new(&self.path)
            .args(args)
            .output()
            .await
            .context(error::CommandFailedSnafu { message: error_msg })?;
        ensure!(
            output.status.success(),
            error::OperationFailedSnafu {
                message: String::from_utf8_lossy(&output.stderr),
                program: self.path.clone(),
                args: args.iter().map(|x| x.to_string()).collect::<Vec<_>>()
            }
        );
        Ok(output.stdout)
    }

    pub(crate) async fn spawn(&self, args: &[&str], error_msg: String) -> Result<()> {
        let status = Command::new(&self.path)
            .args(args)
            .spawn()
            .context(error::CommandFailedSnafu {
                message: error_msg.clone(),
            })?
            .wait()
            .await
            .context(error::CommandFailedSnafu {
                message: error_msg.clone(),
            })?;
        ensure!(
            status.success(),
            error::OperationFailedSnafu {
                message: error_msg.clone(),
                program: self.path.clone(),
                args: args.iter().map(|x| x.to_string()).collect::<Vec<_>>()
            }
        );
        Ok(())
    }
}
