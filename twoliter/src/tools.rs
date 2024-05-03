use crate::common::fs;
use anyhow::{Context, Result};
use filetime::{set_file_handle_times, set_file_mtime, FileTime};
use flate2::read::ZlibDecoder;
use log::debug;
use std::path::Path;
use tar::Archive;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::runtime::Handle;

const TAR_GZ_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tools.tar.gz"));
const BOTTLEROCKET_VARIANT: &[u8] =
    include_bytes!(env!("CARGO_BIN_FILE_BUILDSYS_bottlerocket-variant"));
const BUILDSYS: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_BUILDSYS"));
const PUBSYS: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_PUBSYS"));
const PUBSYS_SETUP: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_PUBSYS_SETUP"));
const TESTSYS: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_TESTSYS"));
const TUFTOOL: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_TUFTOOL"));

/// Install tools into the given `tools_dir`. If you use a `TempDir` object, make sure to pass it by
/// reference and hold on to it until you no longer need the tools to still be installed (it will
/// auto delete when it goes out of scope).
pub(crate) async fn install_tools(tools_dir: impl AsRef<Path>) -> Result<()> {
    let dir = tools_dir.as_ref();
    debug!("Installing tools to '{}'", dir.display());
    fs::remove_dir_all(dir)
        .await
        .context("Unable to remove tools directory before installing")?;
    fs::create_dir_all(dir)
        .await
        .context("Unable to create directory for tools")?;

    // Write out the embedded tools and scripts.
    unpack_tarball(dir)
        .await
        .context("Unable to install tools")?;

    // Pick one of the embedded files for use as the canonical mtime.
    let metadata = fs::metadata(dir.join("Dockerfile"))
        .await
        .context("Unable to get Dockerfile metadata")?;
    let mtime = FileTime::from_last_modification_time(&metadata);

    write_bin("bottlerocket-variant", BOTTLEROCKET_VARIANT, &dir, mtime).await?;
    write_bin("buildsys", BUILDSYS, &dir, mtime).await?;
    write_bin("pubsys", PUBSYS, &dir, mtime).await?;
    write_bin("pubsys-setup", PUBSYS_SETUP, &dir, mtime).await?;
    write_bin("testsys", TESTSYS, &dir, mtime).await?;
    write_bin("tuftool", TUFTOOL, &dir, mtime).await?;

    // Apply the mtime to the directory now that the writes are done.
    set_file_mtime(dir, mtime).context(format!("Unable to set mtime for '{}'", dir.display()))?;

    Ok(())
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
    debug!("Installed tools to '{}'", tools_dir.display());
    Ok(())
}

#[tokio::test]
async fn test_install_tools() {
    let tempdir = tempfile::TempDir::new().unwrap();
    let toolsdir = tempdir.path().join("tools");
    install_tools(&toolsdir).await.unwrap();

    // Assert that the expected files exist in the tools directory.

    // Check that non-binary files were copied.
    assert!(toolsdir.join("Dockerfile").is_file());
    assert!(toolsdir.join("Makefile.toml").is_file());
    assert!(toolsdir.join("docker-go").is_file());
    assert!(toolsdir.join("partyplanner").is_file());
    assert!(toolsdir.join("rpm2img").is_file());
    assert!(toolsdir.join("rpm2kit").is_file());
    assert!(toolsdir.join("rpm2kmodkit").is_file());
    assert!(toolsdir.join("rpm2migrations").is_file());
    assert!(toolsdir.join("metadata.spec").is_file());

    // Check that binaries were copied.
    assert!(toolsdir.join("bottlerocket-variant").is_file());
    assert!(toolsdir.join("buildsys").is_file());
    assert!(toolsdir.join("pubsys").is_file());
    assert!(toolsdir.join("pubsys-setup").is_file());
    assert!(toolsdir.join("testsys").is_file());
    assert!(toolsdir.join("tuftool").is_file());

    // Check that the mtimes match.
    let dockerfile_metadata = fs::metadata(toolsdir.join("Dockerfile")).await.unwrap();
    let buildsys_metadata = fs::metadata(toolsdir.join("buildsys")).await.unwrap();
    let dockerfile_mtime = FileTime::from_last_modification_time(&dockerfile_metadata);
    let buildsys_mtime = FileTime::from_last_modification_time(&buildsys_metadata);

    assert_eq!(dockerfile_mtime, buildsys_mtime);
}
