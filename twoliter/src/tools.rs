use anyhow::{Context, Result};
use flate2::read::ZlibDecoder;
use log::debug;
use std::path::Path;
use tar::Archive;
use tempfile::TempDir;
use tokio::fs;
use tokio::io::AsyncWriteExt;

const TAR_GZ_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tools.tar.gz"));
const BOTTLEROCKET_VARIANT: &[u8] =
    include_bytes!(env!("CARGO_BIN_FILE_BUILDSYS_bottlerocket-variant"));
const BUILDSYS: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_BUILDSYS"));
const PUBSYS: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_PUBSYS"));
const PUBSYS_SETUP: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_PUBSYS_SETUP"));
const TESTSYS: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_TESTSYS"));
const TUFTOOL: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_TUFTOOL"));

/// Create a `TempDir` object and provide a tools-centric error message if it fails. Make sure you
/// hang on to the `TempDir` for as long as you need it. It will be deleted when it goes out of
/// scope.
pub(crate) fn tools_tempdir() -> Result<TempDir> {
    TempDir::new().context("Unable to create a tempdir for Twoliter's tools")
}

/// Install tools into the given `tools_dir`. If you use a `TempDir` object, make sure to pass it by
/// reference and hold on to it until you no longer need the tools to still be installed (it will
/// auto delete when it goes out of scope).
pub(crate) async fn install_tools(tools_dir: impl AsRef<Path>) -> Result<()> {
    let dir = tools_dir.as_ref();
    debug!("Installing tools to '{}'", dir.display());

    write_bin("bottlerocket-variant", BOTTLEROCKET_VARIANT, &dir).await?;
    write_bin("buildsys", BUILDSYS, &dir).await?;
    write_bin("pubsys", PUBSYS, &dir).await?;
    write_bin("pubsys-setup", PUBSYS_SETUP, &dir).await?;
    write_bin("testsys", TESTSYS, &dir).await?;
    write_bin("tuftool", TUFTOOL, &dir).await?;

    // Write out the embedded tools and scripts.
    unpack_tarball(&dir)
        .await
        .context("Unable to install tools")?;

    Ok(())
}

async fn write_bin(name: &str, data: &[u8], dir: impl AsRef<Path>) -> Result<()> {
    let path = dir.as_ref().join(name);
    let mut f = fs::OpenOptions::new()
        .create(true)
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
        .context(format!("Unable to finalize '{}'", path.display()))
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
    let tempdir = tools_tempdir().unwrap();
    install_tools(&tempdir).await.unwrap();

    // Assert that the expected files exist in the tools directory.

    // Check that non-binary files were copied.
    assert!(tempdir.path().join("Dockerfile").is_file());
    assert!(tempdir.path().join("Makefile.toml").is_file());
    assert!(tempdir.path().join("docker-go").is_file());
    assert!(tempdir.path().join("partyplanner").is_file());
    assert!(tempdir.path().join("rpm2img").is_file());
    assert!(tempdir.path().join("rpm2kmodkit").is_file());
    assert!(tempdir.path().join("rpm2migrations").is_file());

    // Check that binaries were copied.
    assert!(tempdir.path().join("bottlerocket-variant").is_file());
    assert!(tempdir.path().join("buildsys").is_file());
    assert!(tempdir.path().join("pubsys").is_file());
    assert!(tempdir.path().join("pubsys-setup").is_file());
    assert!(tempdir.path().join("testsys").is_file());
    assert!(tempdir.path().join("tuftool").is_file());
}
