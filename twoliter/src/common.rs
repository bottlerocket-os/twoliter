use anyhow::{ensure, Context, Result};
use filetime::FileTime;
use log::{self, LevelFilter};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::fs::OpenOptions;
use tokio::process::Command;
use tokio::time::sleep;
use tracing::{debug, instrument};

/// This is passed as an environment variable to Buildsys. Buildsys tells Cargo to watch this
/// environment variable for changes. So if we have a breaking change to the way Buildsys and/or
/// Twoliter function, we can increment this so that we know users will rebuild after updating
/// Twoliter.
pub(crate) const BUILDSYS_OUTPUT_GENERATION_ID: u32 = 1;

/// Run a `tokio::process::Command` and return a `Result` letting us know whether or not it worked.
/// Pipes stdout/stderr when logging `LevelFilter` is more verbose than `Warn`.
#[instrument(level = "trace", skip(cmd))]
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
#[instrument(level = "trace")]
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
    use path_absolutize::Absolutize;
    use std::fs::Metadata;
    use std::io::ErrorKind;
    use std::path::{Path, PathBuf};
    use tokio::fs;
    use tracing::instrument;

    /// Canonicalizes the input path based on `cwd` without resolving symlinks or accessing the
    /// filesystem.
    #[instrument(level = "trace", skip(path), fields(path = %path.as_ref().display()))]
    pub(crate) async fn canonicalize(path: impl AsRef<Path>) -> Result<PathBuf> {
        path.as_ref()
            .absolutize()
            .context(format!(
                "Unable to canonicalize '{}'",
                path.as_ref().display()
            ))
            .map(|p| p.to_path_buf())
    }

    #[instrument(
        level = "trace",
        skip_all,
        fields(from = %from.as_ref().display(), to = %to.as_ref().display())
    )]
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

    #[instrument(level = "trace", skip(path), fields(path = %path.as_ref().display()))]
    pub(crate) async fn create_dir(path: impl AsRef<Path>) -> Result<()> {
        fs::create_dir(path.as_ref()).await.context(format!(
            "Unable to create directory '{}'",
            path.as_ref().display()
        ))
    }

    #[instrument(level = "trace", skip(path), fields(path = %path.as_ref().display()))]
    pub(crate) async fn create_dir_all(path: impl AsRef<Path>) -> Result<()> {
        fs::create_dir_all(path.as_ref()).await.context(format!(
            "Unable to create directory '{}'",
            path.as_ref().display()
        ))
    }

    #[instrument(level = "trace", skip(path), fields(path = %path.as_ref().display()))]
    pub(crate) async fn metadata(path: impl AsRef<Path>) -> Result<Metadata> {
        fs::metadata(path.as_ref()).await.context(format!(
            "Unable to read metadata for '{}'",
            path.as_ref().display()
        ))
    }

    #[instrument(level = "trace", skip(path), fields(path = %path.as_ref().display()))]
    pub(crate) async fn read(path: impl AsRef<Path>) -> Result<Vec<u8>> {
        fs::read(path.as_ref())
            .await
            .context(format!("Unable to read from '{}'", path.as_ref().display()))
    }

    #[instrument(level = "trace", skip(path), fields(path = %path.as_ref().display()))]
    pub(crate) async fn read_to_string(path: impl AsRef<Path>) -> Result<String> {
        fs::read_to_string(path.as_ref()).await.context(format!(
            "Unable to read the following file as a string '{}'",
            path.as_ref().display()
        ))
    }

    #[instrument(level = "trace", skip(path), fields(path = %path.as_ref().display()))]
    pub(crate) async fn remove_dir(path: impl AsRef<Path>) -> Result<()> {
        fs::remove_dir(path.as_ref()).await.context(format!(
            "Unable to remove directory (remove_dir) '{}'",
            path.as_ref().display()
        ))
    }

    #[instrument(level = "trace", skip(path), fields(path = %path.as_ref().display()))]
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

    #[instrument(
        level = "trace",
        skip_all,
        fields(from = %from.as_ref().display(), to = %to.as_ref().display())
    )]
    pub(crate) async fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
        let from = from.as_ref();
        let to = to.as_ref();
        fs::rename(from, to).await.context(format!(
            "Unable to rename '{}' to '{}'",
            from.display(),
            to.display()
        ))
    }

    #[instrument(level = "trace", skip(path), fields(path = %path.as_ref().display()))]
    pub(crate) async fn remove_file(path: impl AsRef<Path>) -> Result<()> {
        fs::remove_file(path.as_ref()).await.context(format!(
            "Unable to remove file '{}'",
            path.as_ref().display()
        ))
    }

    #[instrument(level = "trace", skip_all, fields(path = %path.as_ref().display()))]
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

/// A utility for safely acquiring and releasing file locks in a concurrent environment.
/// This provides atomic file-based locking with exponential backoff and stale lock detection.
pub(crate) struct FileLocker {
    lock_path: PathBuf,
    stale_timeout_secs: u64,
    max_attempts: u32,
    base_delay_ms: u64,
}

impl FileLocker {
    /// Create a new FileLocker for the specified lock path
    pub(crate) fn new(lock_path: impl AsRef<Path>) -> Self {
        Self {
            lock_path: lock_path.as_ref().to_path_buf(),
            stale_timeout_secs: 30,
            max_attempts: 5,
            base_delay_ms: 50,
        }
    }

    /// Try to acquire the lock with exponential backoff
    pub(crate) async fn try_acquire(&self) -> Result<Option<FileLock>> {
        for attempt in 0..self.max_attempts {
            match OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&self.lock_path)
                .await
            {
                Ok(file) => {
                    debug!("Acquired lock: {}", self.lock_path.display());
                    return Ok(Some(FileLock {
                        lock_path: self.lock_path.clone(),
                        _file: file,
                    }));
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    // Check if lock is stale
                    if let Ok(lock_meta) = fs::metadata(&self.lock_path).await {
                        let now = FileTime::now();
                        let lock_time = FileTime::from_last_modification_time(&lock_meta);
                        if now.seconds() - lock_time.seconds() > self.stale_timeout_secs as i64 {
                            debug!("Removing stale lock: {}", self.lock_path.display());
                            let _ = fs::remove_file(&self.lock_path).await;
                            continue;
                        }
                    }
                }
                Err(_) => {}
            }

            // Exponential backoff with jitter
            let max_pow = attempt.min(3); // Cap at 2^3 to avoid excessive delays
            let delay = (2_u64.pow(max_pow) * self.base_delay_ms) + (fastrand::u64(1..=50));
            sleep(Duration::from_millis(delay)).await;
        }

        Ok(None) // Failed to acquire lock after max attempts
    }
}

/// Represents an acquired file lock that is automatically released when dropped
pub(crate) struct FileLock {
    lock_path: PathBuf,
    _file: tokio::fs::File, // Keep the file handle to maintain the lock
}

impl Drop for FileLock {
    fn drop(&mut self) {
        debug!("Releasing lock: {}", self.lock_path.display());

        // Use a synchronous file removal on drop since we can't use async in Drop
        // This is acceptable since the file is small and drop should be fast
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

/// Utilities for content-based file comparison and tracking
pub(crate) mod content {
    use super::*;
    use anyhow::Context;
    use std::collections::hash_map::DefaultHasher;
    use std::collections::HashMap;
    use std::hash::{Hash, Hasher};
    use std::path::Path;
    use tokio::fs;
    use tracing::debug;

    /// Calculate a hash for content
    pub(crate) fn calculate_hash(content: &[u8]) -> u64 {
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        hasher.finish()
    }

    /// Check if a file's content needs to be updated based on hash comparison
    pub(crate) async fn needs_content_update(
        path: impl AsRef<Path>,
        new_content: &[u8],
    ) -> Result<bool> {
        let path = path.as_ref();
        if path.exists() {
            match fs::read(path).await {
                Ok(existing) => {
                    let existing_hash = calculate_hash(&existing);
                    let new_hash = calculate_hash(new_content);
                    let needs_update = existing_hash != new_hash;

                    if needs_update {
                        debug!(
                            "Content hash mismatch for '{}': existing={:x}, new={:x}",
                            path.display(),
                            existing_hash,
                            new_hash
                        );
                    } else {
                        debug!(
                            "Content hash match for '{}': hash={:x}",
                            path.display(),
                            existing_hash
                        );
                    }

                    Ok(needs_update)
                }
                Err(e) => {
                    debug!("Error reading existing file '{}': {}", path.display(), e);
                    Ok(true) // If we can't read it, we'll rewrite it
                }
            }
        } else {
            debug!("File '{}' doesn't exist, needs creation", path.display());
            Ok(true) // File doesn't exist, need to create it
        }
    }

    /// Compare two directories to determine if their contents are identical
    /// Returns Ok(true) if directories differ, Ok(false) if they're identical, or an Error
    pub(crate) async fn compare_directories(
        dir1: impl AsRef<Path>,
        dir2: impl AsRef<Path>,
    ) -> Result<bool> {
        let dir1 = dir1.as_ref();
        let dir2 = dir2.as_ref();

        debug!(
            "Comparing directories: '{}' and '{}'",
            dir1.display(),
            dir2.display()
        );

        // Get the list of files in both directories
        let dir1_entries = get_file_list(dir1).await?;
        let dir2_entries = get_file_list(dir2).await?;

        // First, check if the file lists are different
        if dir1_entries.len() != dir2_entries.len() {
            debug!(
                "Directory sizes differ: {} vs {} files",
                dir1_entries.len(),
                dir2_entries.len()
            );
            return Ok(true); // Different number of files means directories differ
        }

        // Next check file by file to see if anything differs
        for (file_path, hash1) in &dir1_entries {
            match dir2_entries.get(file_path) {
                Some(hash2) if hash1 == hash2 => {
                    // File content is the same, continue checking
                    continue;
                }
                Some(hash2) => {
                    debug!(
                        "Content hash mismatch for '{}': {:x} vs {:x}",
                        file_path, hash1, hash2
                    );
                    return Ok(true); // File content is different
                }
                None => {
                    debug!("File '{}' missing in second directory", file_path);
                    return Ok(true); // File doesn't exist in dir2
                }
            }
        }

        // If we got here, directories are identical
        debug!(
            "Directories are identical: {} files with matching content",
            dir1_entries.len()
        );
        Ok(false)
    }

    /// Get list of files in a directory with their content hashes
    pub(crate) async fn get_file_list(dir: &Path) -> Result<HashMap<String, u64>> {
        let mut result = HashMap::new();
        let entries = fs::read_dir(dir)
            .await
            .context(format!("Unable to read directory '{}'", dir.display()))?;

        let mut entries_vec = Vec::new();
        let mut dir_entries = entries;
        while let Some(entry) = dir_entries.next_entry().await? {
            entries_vec.push(entry);
        }

        for entry in entries_vec {
            let path = entry.path();
            let metadata = entry.metadata().await?;

            if metadata.is_file() {
                // Get relative path
                let rel_path = path
                    .strip_prefix(dir)
                    .context(format!("Unable to strip prefix from '{}'", path.display()))?
                    .to_string_lossy()
                    .into_owned();

                // Calculate hash for file content
                let content = fs::read(&path)
                    .await
                    .context(format!("Unable to read file '{}'", path.display()))?;
                let hash = calculate_hash(&content);

                result.insert(rel_path, hash);
            } else if metadata.is_dir() {
                // Handle subdirectories recursively using Box::pin for recursive async calls
                let subdirectory = Box::pin(get_file_list(&path)).await?;
                for (sub_path, hash) in subdirectory {
                    let full_path = format!(
                        "{}/{}",
                        path.file_name().unwrap_or_default().to_string_lossy(),
                        sub_path
                    );
                    result.insert(full_path, hash);
                }
            }
        }

        Ok(result)
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
