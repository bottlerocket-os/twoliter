use anyhow::{ensure, Context, Result};
use log::{self, debug, LevelFilter};
use tokio::process::Command;

/// Run a `tokio::process::Command` and return a `Result` letting us know whether or not it worked.
pub(crate) async fn exec(cmd: &mut Command) -> Result<()> {
    debug!("Running: {:?}", cmd);

    match log::max_level() {
        // For quiet levels of logging we capture stdout and stderr
        LevelFilter::Off | LevelFilter::Error | LevelFilter::Warn => {
            let output = cmd
                .output()
                .await
                .context(format!("Unable to start command"))?;
            ensure!(
                output.status.success(),
                "Command was unsuccessful, exit code {}:\n{}\n{}",
                output.status.code().unwrap_or(1),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // For less quiet log levels we stream to stdout and stderr.
        LevelFilter::Info | LevelFilter::Debug | LevelFilter::Trace => {
            let status = cmd
                .status()
                .await
                .context(format!("Unable to start command"))?;

            ensure!(
                status.success(),
                "Command was unsuccessful, exit code {}",
                status.code().unwrap_or(1),
            );
        }
    }
    Ok(())
}

// TODO mod fs