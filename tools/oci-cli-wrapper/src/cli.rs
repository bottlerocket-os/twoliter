use snafu::{ensure, ResultExt};
use std::path::PathBuf;
use tokio::process::Command;

use crate::{error, Result};

#[derive(Debug)]
pub(crate) struct CommandLine {
    pub(crate) path: PathBuf,
}

impl CommandLine {
    pub(crate) async fn output(&self, args: &[&str], error_msg: String) -> Result<Vec<u8>> {
        let debug_cmd = [
            vec![format!("{}", self.path.display())],
            args.iter()
                .map(|arg| format!("'{arg}'"))
                .collect::<Vec<_>>(),
        ]
        .concat()
        .join(", ");

        log::debug!("Executing [{debug_cmd}]",);
        let output = Command::new(&self.path)
            .args(args)
            .output()
            .await
            .context(error::CommandFailedSnafu { message: error_msg })?;

        ensure!(
            output.status.success(),
            error::OperationFailedSnafu {
                message: format!(
                    "[{debug_cmd}]: status: {} stderr: {} stdout: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&output.stdout)
                ),
                program: self.path.clone(),
                args: args.iter().map(|x| x.to_string()).collect::<Vec<_>>()
            }
        );

        log::debug!(
            "[{debug_cmd}] stdout: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        log::debug!(
            "[{debug_cmd}] stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        Ok(output.stdout)
    }

    pub(crate) async fn spawn(&self, args: &[&str], error_msg: String) -> Result<()> {
        log::debug!(
            "Executing '{}' with args [{}]",
            self.path.display(),
            args.iter()
                .map(|arg| format!("'{arg}'"))
                .collect::<Vec<_>>()
                .join(", ")
        );
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
