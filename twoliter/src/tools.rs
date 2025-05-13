use crate::common::{self, content, fs};
use anyhow::{Context, Result};
use filetime::{set_file_handle_times, set_file_mtime, FileTime};
use flate2::read::ZlibDecoder;
use krane_bundle::KRANE;
use std::path::Path;
use tar::Archive;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::runtime::Handle;
use tracing::{debug, info};

const TAR_GZ_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tools.tar.gz"));
const BUILDSYS: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_BUILDSYS"));
const PIPESYS: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_PIPESYS"));
#[cfg(feature = "pubsys")]
const PUBSYS: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_PUBSYS"));
const PUBSYS_SETUP: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_PUBSYS_SETUP"));
const TESTSYS: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_TESTSYS"));
const TUFTOOL: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_TUFTOOL"));
const UNPLUG: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_UNPLUG"));

/// Install tools into the given `tools_dir`. Uses a temporary directory and file locking
/// to prevent race conditions when multiple processes install tools concurrently.
pub(crate) async fn install_tools(tools_dir: impl AsRef<Path>) -> Result<()> {
    let dir = tools_dir.as_ref().to_path_buf();
    debug!("Installing tools to '{}'", dir.display());

    // Create parent directory and prepare temporary directory
    let parent_dir = dir.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent_dir)
        .await
        .context("Unable to create parent directory")?;

    let temp_dir = parent_dir.join(format!(
        "{}.tmp.{}",
        dir.file_name().unwrap_or_default().to_string_lossy(),
        uuid::Uuid::new_v4()
    ));
    let lock_path = parent_dir.join(format!(
        ".{}.lock",
        dir.file_name().unwrap_or_default().to_string_lossy()
    ));

    // Clean up and recreate temporary directory
    let _ = fs::remove_dir_all(&temp_dir).await;
    fs::create_dir_all(&temp_dir)
        .await
        .context("Unable to create temporary directory for tools")?;

    // Write out the embedded tools and scripts.
    unpack_tarball(&temp_dir)
        .await
        .context("Unable to install tools")?;

    // Pick one of the embedded files for use as the canonical mtime.
    let metadata = fs::metadata(temp_dir.join("build.Dockerfile"))
        .await
        .context("Unable to get Dockerfile metadata")?;
    let mtime = FileTime::from_last_modification_time(&metadata);

    // Write all binaries to the temporary directory
    write_bin("buildsys", BUILDSYS, &temp_dir, mtime).await?;
    write_bin("pipesys", PIPESYS, &temp_dir, mtime).await?;
    #[cfg(feature = "pubsys")]
    write_bin("pubsys", PUBSYS, &temp_dir, mtime).await?;
    write_bin("pubsys-setup", PUBSYS_SETUP, &temp_dir, mtime).await?;
    write_bin("testsys", TESTSYS, &temp_dir, mtime).await?;
    write_bin("tuftool", TUFTOOL, &temp_dir, mtime).await?;
    write_bin("unplug", UNPLUG, &temp_dir, mtime).await?;
    fs::copy(KRANE.path(), temp_dir.join("krane")).await?;
    set_file_mtime(&temp_dir, mtime)
        .context(format!("Unable to set mtime for '{}'", temp_dir.display()))?;

    // Use the common FileLocker utility for file-based locking
    let file_locker = common::FileLocker::new(&lock_path);

    // Acquire the lock directly
    let lock = match file_locker.try_acquire().await? {
        Some(lock) => lock,
        None => {
            // Clean up temporary directory if we can't get the lock
            let _ = fs::remove_dir_all(&temp_dir).await;
            return Err(anyhow::anyhow!(
                "Failed to acquire lock for installing tools"
            ));
        }
    };

    // Perform the critical section operations while holding the lock
    let result = async {
        if dir.exists() {
            // Check if we need to update based on content hashes using common utilities
            let need_update = match content::compare_directories(&temp_dir, &dir).await {
                Ok(different) => {
                    if different {
                        debug!("Content differences detected, updating tools");
                        true
                    } else {
                        info!("Tools directory content matches, skipping update");
                        false
                    }
                }
                Err(e) => {
                    debug!("Error comparing directories, will reinstall: {}", e);
                    true
                }
            };

            if need_update {
                fs::remove_dir_all(&dir)
                    .await
                    .context("Unable to remove existing tools directory")?;

                fs::rename(&temp_dir, &dir).await.context(format!(
                    "Unable to move temp directory to '{}'",
                    dir.display()
                ))?;
                debug!("Successfully updated tools in '{}'", dir.display());
            } else {
                // If no update needed, clean up the temp directory
                let _ = fs::remove_dir_all(&temp_dir).await;
                debug!(
                    "Tools in '{}' are up to date, no changes made",
                    dir.display()
                );
            }
        } else {
            // Directory doesn't exist, just move the temp directory
            fs::rename(&temp_dir, &dir).await.context(format!(
                "Unable to move temp directory to '{}'",
                dir.display()
            ))?;
            debug!("Successfully installed tools to '{}'", dir.display());
        }
        Ok(())
    }
    .await;

    // If there was an error and the temp directory still exists, clean it up
    if result.is_err() {
        let _ = fs::remove_dir_all(&temp_dir).await;
    }

    drop(lock);

    result
}

async fn write_bin(name: &str, data: &[u8], dir: impl AsRef<Path>, mtime: FileTime) -> Result<()> {
    let path = dir.as_ref().join(name);
    let mut f = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(false)
        .write(true)
        .mode(0o755)
        .open(&path)
        .await
        .context(format!("Unable to create file '{}'", path.display()))?;
    f.write_all(data)
        .await
        .context(format!("Unable to write to '{}'", path.display()))?;
    f.flush()
        .await
        .context(format!("Unable to finalize '{}'", path.display()))?;

    let f = f.into_std().await;
    let rt = Handle::current();
    rt.spawn_blocking(move || {
        set_file_handle_times(&f, None, Some(mtime))
            .context(format!("Unable to set mtime for '{}'", path.display()))
    })
    .await
    .context("Unable to run and join async task for reading handle time".to_string())?
}

async fn unpack_tarball(tools_dir: impl AsRef<Path>) -> Result<()> {
    let tools_dir = tools_dir.as_ref();
    let tar = ZlibDecoder::new(TAR_GZ_DATA);
    let mut archive = Archive::new(tar);
    archive.unpack(tools_dir).context(format!(
        "Unable to unpack tarball into directory '{}'",
        tools_dir.display()
    ))?;
    debug!("Unpacked tarball to '{}'", tools_dir.display());
    Ok(())
}

#[tokio::test]
async fn test_install_tools() {
    let tempdir = tempfile::TempDir::new().unwrap();
    let toolsdir = tempdir.path().join("tools");
    install_tools(&toolsdir).await.unwrap();

    // Assert that the expected files exist in the tools directory.

    // Check that non-binary files were copied.
    assert!(toolsdir.join("Makefile.toml").is_file());
    assert!(toolsdir.join("build.Dockerfile").is_file());
    assert!(toolsdir.join("build.Dockerfile.dockerignore").is_file());
    assert!(toolsdir.join("docker-go").is_file());
    assert!(toolsdir.join("img2img").is_file());
    assert!(toolsdir.join("imghelper").is_file());
    assert!(toolsdir.join("metadata.spec").is_file());
    assert!(toolsdir.join("partyplanner").is_file());
    assert!(toolsdir.join("rpm2img").is_file());
    assert!(toolsdir.join("rpm2kit").is_file());
    assert!(toolsdir.join("rpm2kmodkit").is_file());
    assert!(toolsdir.join("rpm2migrations").is_file());

    // Check that binaries were copied.
    assert!(toolsdir.join("buildsys").is_file());
    assert!(toolsdir.join("pipesys").is_file());
    assert!(toolsdir.join("pubsys").is_file());
    assert!(toolsdir.join("pubsys-setup").is_file());
    assert!(toolsdir.join("testsys").is_file());
    assert!(toolsdir.join("tuftool").is_file());
    assert!(toolsdir.join("unplug").is_file());

    // Check that the mtimes match.
    let dockerfile_metadata = fs::metadata(toolsdir.join("build.Dockerfile"))
        .await
        .unwrap();
    let buildsys_metadata = fs::metadata(toolsdir.join("buildsys")).await.unwrap();
    let dockerfile_mtime = FileTime::from_last_modification_time(&dockerfile_metadata);
    let buildsys_mtime = FileTime::from_last_modification_time(&buildsys_metadata);

    assert_eq!(dockerfile_mtime, buildsys_mtime);
}

#[tokio::test]
async fn test_content_based_installation() {
    let tempdir = tempfile::TempDir::new().unwrap();
    let toolsdir = tempdir.path().join("tools");

    // First installation
    install_tools(&toolsdir).await.unwrap();

    // Get modification time of a file to check later
    let test_file = toolsdir.join("Makefile.toml");
    let original_metadata = test_file.metadata().unwrap();
    let original_modified = original_metadata.modified().unwrap();

    // Sleep to ensure potential timestamp difference would be detectable
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Second installation - should skip updates due to identical content
    install_tools(&toolsdir).await.unwrap();

    // Check that file wasn't modified (timestamp should be the same)
    let new_metadata = test_file.metadata().unwrap();
    let new_modified = new_metadata.modified().unwrap();

    assert_eq!(
        original_modified, new_modified,
        "File was replaced despite identical content"
    );
}

#[tokio::test]
async fn test_content_update_when_different() {
    let tempdir = tempfile::TempDir::new().unwrap();
    let toolsdir = tempdir.path().join("tools");

    // First installation
    install_tools(&toolsdir).await.unwrap();

    // Get modification time of a file
    let test_file = toolsdir.join("Makefile.toml");
    let _original_content = tokio::fs::read_to_string(&test_file).await.unwrap(); // Keep for test clarity
    let original_metadata = test_file.metadata().unwrap();
    let original_modified = original_metadata.modified().unwrap();

    // Sleep to ensure potential timestamp difference would be detectable
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Modify a file to force update on next installation
    let modified_content = format!(
        "modified content {}",
        std::time::SystemTime::now().elapsed().unwrap().as_millis()
    );
    tokio::fs::write(&test_file, &modified_content)
        .await
        .unwrap();

    // Verify the file was actually modified
    let intermediate_content = tokio::fs::read_to_string(&test_file).await.unwrap();
    assert_eq!(
        intermediate_content, modified_content,
        "Failed to modify file content for test"
    );

    // Sleep again to ensure timestamps would differ
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Second installation - should update due to content difference
    install_tools(&toolsdir).await.unwrap();

    // Check that file content was restored
    let final_content = tokio::fs::read_to_string(&test_file).await.unwrap();
    assert_ne!(
        final_content, modified_content,
        "File content wasn't updated by reinstallation"
    );

    // Get the new metadata to check modification time
    let new_metadata = test_file.metadata().unwrap();
    let new_modified = new_metadata.modified().unwrap();

    // The file should have been replaced, so timestamps should differ
    let timestamp_changed = original_modified != new_modified;
    let content_changed = final_content != modified_content;

    // At least one of these conditions should be true
    assert!(
        timestamp_changed || content_changed,
        "Neither file timestamp nor content was updated despite modified content"
    );
}

#[tokio::test]
async fn test_concurrent_installation() {
    let tempdir = tempfile::TempDir::new().unwrap();
    let toolsdir = tempdir.path().join("tools");

    // Launch two concurrent installations
    let install1 = install_tools(&toolsdir);
    let install2 = install_tools(&toolsdir);

    // Both should complete without errors
    let (result1, result2) = tokio::join!(install1, install2);
    assert!(result1.is_ok(), "First installation failed");
    assert!(result2.is_ok(), "Second installation failed");

    // Verify tools directory has expected files
    assert!(toolsdir.join("Makefile.toml").is_file());
    assert!(toolsdir.join("buildsys").is_file());
}
