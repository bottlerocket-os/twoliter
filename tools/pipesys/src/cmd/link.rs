use super::fetch_fd;

use anyhow::{bail, Context, Result};
use clap::Parser;
use daemonize::{Daemonize, Outcome};
use futures::{Future, StreamExt};
use inotify::{Inotify, WatchMask};
use log::{error, info, trace};
use std::path::{Path, PathBuf};
use std::{env, process};
use tokio::fs;

/// Retrieve a file descriptor from an abstract socket, and set up a
/// symlink to it that provides access to subsequent processes until
/// the symlink is removed.
#[derive(Debug, Parser)]
pub(crate) struct Link {
    /// Fetch the file descriptor for a path from this abstract socket.
    #[clap(long = "fd-socket")]
    fd_socket: String,

    /// Create this target path as a symlink to the file descriptor.
    #[clap(long = "target")]
    target: PathBuf,
}

impl Link {
    /// Retrieve the file descriptor, then spawn a background process to create the link so that it
    /// survives the return of the foreground process.
    pub(crate) async fn execute(&self) -> Result<()> {
        // Remove the existing symlink, if present.
        if self.target.is_symlink() {
            fs::remove_file(&self.target).await.with_context(|| {
                format!("failed to clean up symlink for {}", self.target.display())
            })?;
        }

        // If the target still exists, do not proceed.
        if self.target.exists() {
            bail!(
                "found existing file or directory at {}",
                self.target.display()
            )
        }

        // Retrieve the path file descriptor.
        let dir_fd = fetch_fd(&self.fd_socket)?;

        // Create a log file for the background process.
        let parent_dir = parent_dir(&self.target)?;
        let log_file = parent_dir.join("pipesys-link.log");
        let (stdout, stderr) = output_streams(&log_file).await.with_context(|| {
            format!("failed to create output streams for {}", log_file.display())
        })?;

        // After we daemonize, we need to avoid returning back to the caller since the async
        // runtime and associated state are no longer valid.
        std::thread::scope(|s| {
            s.spawn(|| {
                if let Outcome::Child(res) = Daemonize::new()
                    .stdout(stdout)
                    .stderr(stderr)
                    .working_directory(parent_dir)
                    .execute()
                {
                    if let Err(e) = res {
                        error!("failed to daemonize: {e}");
                        std::process::exit(1);
                    }

                    trace!("daemonized!");
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_io()
                        .build()
                        .expect("failed to build runtime");

                    rt.block_on(async {
                        if let Err(e) = self.manage_symlink(dir_fd).await {
                            error!("failed to manage symlink: {e}");
                            std::process::exit(1);
                        }
                    });

                    info!("done");
                    std::process::exit(0);
                }
            });
        });

        self.wait_for_symlink().await
    }

    /// The parent (foreground) process waits until the symlink is created.
    async fn wait_for_symlink(&self) -> Result<()> {
        let inotify = inotify_init(&self.target, WatchMask::CREATE)?;
        inotify_wait(inotify, &self.target, &symlink_found).await
    }

    /// The child (background) process creates the symlink. Since it will be invalidated when the
    /// process exits, wait until the external caller removes it before returning.
    async fn manage_symlink(&self, dir_fd: i32) -> Result<()> {
        let inotify = inotify_init(&self.target, WatchMask::DELETE)?;

        // Create the symlink.
        let pid = process::id();
        let source = format!("/proc/{pid}/fd/{dir_fd}");
        fs::symlink(&source, &self.target)
            .await
            .with_context(|| format!("failed to symlink {source} to {}", self.target.display()))?;
        info!("symlinked {} to {source}", self.target.display());

        inotify_wait(inotify, &self.target, &symlink_not_found).await
    }
}

/// Choose a plausible parent directory. For an absolute path, the parent should be available. For
/// a relative path, assume the current process directory is intended.
fn parent_dir(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        if let Some(parent) = path.parent() {
            return Ok(parent.to_path_buf());
        }
    } else if let Ok(current) = env::current_dir() {
        return Ok(current);
    }
    bail!("failed to find parent directory for {}", path.display());
}

/// Helper function to create stdout and stderr streams with the same file storage.
async fn output_streams(path: &Path) -> Result<(std::fs::File, std::fs::File)> {
    let stdout = fs::File::create(path).await?;
    let stderr = stdout.try_clone().await?;
    Ok((stdout.into_std().await, stderr.into_std().await))
}

/// Returns true if a symlink exists at the path, and false otherwise.
async fn symlink_found(path: &Path) -> bool {
    let res = fs::symlink_metadata(path)
        .await
        .with_context(|| format!("failed to query metadata for {}", path.display()));

    if let Ok(m) = res {
        if m.is_symlink() {
            trace!("found symlink for {}", path.display());
            return true;
        }
    }

    trace!("no symlink found for {}", path.display());
    false
}

/// Returns false if a symlink exists at the path, and true otherwise.
async fn symlink_not_found(path: &Path) -> bool {
    !symlink_found(path).await
}

/// Initialize an inotify instance with the requested watch mask.
fn inotify_init(path: &Path, watch_mask: WatchMask) -> Result<Inotify> {
    let parent_dir = parent_dir(path)?;

    let inotify = Inotify::init().with_context(|| "failed to initialize inotify")?;
    inotify
        .watches()
        .add(&parent_dir, watch_mask)
        .with_context(|| format!("failed to add watch for {}", parent_dir.display()))?;

    Ok(inotify)
}

/// Wait for inotify events until the provided function returns true.
async fn inotify_wait<'a, F>(
    watcher: Inotify,
    path: &'a Path,
    check: &dyn Fn(&'a Path) -> F,
) -> Result<()>
where
    F: Future<Output = bool> + 'a,
{
    // Check whether the provided function succeeds right away, in case the watched file was
    // created or removed before the inotify watch could be started.
    if check(path).await {
        return Ok(());
    }

    let file_name = path
        .file_name()
        .with_context(|| format!("failed to find file name for {}", path.display()))?;

    let mut buf = [0; 256];
    let mut event_stream = watcher
        .into_event_stream(&mut buf)
        .with_context(|| "failed to get event stream")?;

    while let Some(event) = event_stream.next().await {
        let event = event.with_context(|| "failed to read event".to_string())?;
        if let Some(event_file_name) = event.name {
            if event_file_name == file_name {
                trace!("found event for {}", path.display());
                if check(path).await {
                    return Ok(());
                }
            }
        }
    }

    bail!("failed to observe event for {}", path.display())
}
