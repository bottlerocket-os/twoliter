use anyhow::{ensure, Context, Result};
use log::{self, debug, LevelFilter};
use tokio::process::Command;

/// Run a `tokio::process::Command` and return a `Result` letting us know whether or not it worked.
/// Pipes stdout/stderr when logging `LevelFilter` is more verbose than `Warn`.
pub(crate) async fn exec_log(cmd: &mut Command) -> Result<()> {
    let quiet = matches!(
        log::max_level(),
        LevelFilter::Off | LevelFilter::Error | LevelFilter::Warn
    );
    exec(cmd, quiet).await?;
    Ok(())
}

/// Run a `tokio::process::Command` and return a `Result` letting us know whether or not it worked.
/// `quiet` determines whether or not the command output will be piped to `stdout/stderr`. When
/// `quiet=true`, no output will be shown and will be returned instead.
pub(crate) async fn exec(cmd: &mut Command, quiet: bool) -> Result<Option<String>> {
    debug!("Running: {:?}", cmd);
    Ok(if quiet {
        // For quiet levels of logging we capture stdout and stderr
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

        Some(
            String::from_utf8(output.stdout)
                .context("Unable to convert command output to `String`")?,
        )
    } else {
        // For less quiet log levels we stream to stdout and stderr.
        let status = cmd
            .status()
            .await
            .context(format!("Unable to start command"))?;

        ensure!(
            status.success(),
            "Command was unsuccessful, exit code {}",
            status.code().unwrap_or(1),
        );

        None
    })
}
