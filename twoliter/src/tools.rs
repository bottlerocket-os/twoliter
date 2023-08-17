use anyhow::{Context, Result};
use flate2::read::ZlibDecoder;
use log::debug;
use std::path::Path;
use tar::Archive;
use tempfile::TempDir;

const TAR_GZ_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tools.tar.gz"));

pub(crate) async fn install_tools() -> Result<TempDir> {
    let tempdir = TempDir::new().context("Unable to create a tempdir for Twoliter's tools")?;
    let tools_dir = tempdir.path();
    debug!("Installing tools to '{}'", tools_dir.display());

    // Write out the embedded tools and scripts.
    unpack_tarball(&tools_dir)
        .await
        .context("Unable to install tools")?;

    Ok(tempdir)
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
    let tempdir = install_tools().await.unwrap();

    // Assert that the expected files exist in the tools directory.
    assert!(tempdir.path().join("Makefile.toml").is_file());
}
