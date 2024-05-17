use anyhow::{ensure, Context, Result};
use log::{self, debug, LevelFilter};
use tokio::process::Command;

/// This is passed as an environment variable to Buildsys. Buildsys tells Cargo to watch this
/// environment variable for changes. So if we have a breaking change to the way Buildsys and/or
/// Twoliter function, we can increment this so that we know users will rebuild after updating
/// Twoliter.
pub(crate) const BUILDSYS_OUTPUT_GENERATION_ID: u32 = 1;

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
            .context("Unable to start command".to_string())?;
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
            .context("Unable to start command".to_string())?;

        ensure!(
            status.success(),
            "Command was unsuccessful, exit code {}",
            status.code().unwrap_or(1),
        );

        None
    })
}

/// These are thin wrappers for `tokio::fs` functions which provide more useful error messages. For
/// example, tokio will provide an unhelpful `std` error message such as `Error: No such file or
/// directory (os error 2)` and we want to augment this with the filepath that was not found.
///
/// We allow `dead_code` here because it is inconvenient to delete and replace these simple helper
/// functions as we change calling code. The compiler will strip dead code in release builds anyway,
/// so there is no real issue having these unused here.
#[allow(dead_code)]
pub(crate) mod fs {
    use anyhow::{Context, Result};
    use std::fs::Metadata;
    use std::io::ErrorKind;
    use std::path::{Path, PathBuf};
    use tokio::fs;

    pub(crate) async fn canonicalize(path: impl AsRef<Path>) -> Result<PathBuf> {
        fs::canonicalize(path.as_ref()).await.context(format!(
            "Unable to canonicalize '{}'",
            path.as_ref().display()
        ))
    }

    pub(crate) async fn copy<P1, P2>(from: P1, to: P2) -> Result<u64>
    where
        P1: AsRef<Path>,
        P2: AsRef<Path>,
    {
        let from = from.as_ref();
        let to = to.as_ref();
        fs::copy(from, to).await.context(format!(
            "Unable to copy '{}' to '{}'",
            from.display(),
            to.display()
        ))
    }

    pub(crate) async fn create_dir(path: impl AsRef<Path>) -> Result<()> {
        fs::create_dir(path.as_ref()).await.context(format!(
            "Unable to create directory '{}'",
            path.as_ref().display()
        ))
    }

    pub(crate) async fn create_dir_all(path: impl AsRef<Path>) -> Result<()> {
        fs::create_dir_all(path.as_ref()).await.context(format!(
            "Unable to create directory '{}'",
            path.as_ref().display()
        ))
    }

    pub(crate) async fn metadata(path: impl AsRef<Path>) -> Result<Metadata> {
        fs::metadata(path.as_ref()).await.context(format!(
            "Unable to read metadata for '{}'",
            path.as_ref().display()
        ))
    }

    pub(crate) async fn read(path: impl AsRef<Path>) -> Result<Vec<u8>> {
        fs::read(path.as_ref())
            .await
            .context(format!("Unable to read from '{}'", path.as_ref().display()))
    }

    pub(crate) async fn read_to_string(path: impl AsRef<Path>) -> Result<String> {
        fs::read_to_string(path.as_ref()).await.context(format!(
            "Unable to read the following file as a string '{}'",
            path.as_ref().display()
        ))
    }

    pub(crate) async fn remove_dir(path: impl AsRef<Path>) -> Result<()> {
        fs::remove_dir(path.as_ref()).await.context(format!(
            "Unable to remove directory (remove_dir) '{}'",
            path.as_ref().display()
        ))
    }

    pub(crate) async fn remove_dir_all(path: impl AsRef<Path>) -> Result<()> {
        match fs::remove_dir_all(path.as_ref()).await {
            Ok(_) => Ok(()),
            Err(e) => match e.kind() {
                ErrorKind::NotFound => {
                    // not a problem, the directory isn't there
                    Ok(())
                }
                _ => Err(e).context(format!(
                    "Unable to remove directory (remove_dir_all) '{}'",
                    path.as_ref().display()
                )),
            },
        }
    }

    pub(crate) async fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
        let from = from.as_ref();
        let to = to.as_ref();
        fs::rename(from, to).await.context(format!(
            "Unable to rename '{}' to '{}'",
            from.display(),
            to.display()
        ))
    }

    pub(crate) async fn remove_file(path: impl AsRef<Path>) -> Result<()> {
        fs::remove_file(path.as_ref()).await.context(format!(
            "Unable to remove file '{}'",
            path.as_ref().display()
        ))
    }

    pub(crate) async fn write<P, C>(path: P, contents: C) -> Result<()>
    where
        P: AsRef<Path>,
        C: AsRef<[u8]>,
    {
        fs::write(path.as_ref(), contents)
            .await
            .context(format!("Unable to write to '{}'", path.as_ref().display()))
    }
}

#[tokio::test]
async fn test_remove_dir_all_no_dir() {
    use crate::common::fs;
    use tempfile::TempDir;

    let tempdir = TempDir::new().unwrap();
    let does_not_exist = tempdir.path().join("nope");

    // This should not error even though the directory is not present.
    fs::remove_dir_all(does_not_exist).await.unwrap();
}

#[tokio::test]
async fn test_create_and_remove_dir() {
    use crate::common::fs;
    use tempfile::TempDir;

    let tempdir = TempDir::new().unwrap();
    let path = tempdir.path().join("yep").join("ok");

    fs::create_dir_all(&path).await.unwrap();
    assert!(
        path.is_dir(),
        "Expected a directory to be created at '{}'",
        path.display()
    );

    fs::remove_dir_all(&path).await.unwrap();
    assert!(
        !path.exists(),
        "Expected directories to be removed from this path '{}'",
        path.display()
    )
}
