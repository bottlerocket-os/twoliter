use crate::common::fs;
use anyhow::{Context, Result};
use filetime::{set_file_handle_times, set_file_mtime, FileTime};
use flate2::read::ZlibDecoder;
use futures::future::try_join_all;
use std::fs::OpenOptions;
use std::io::{Read, Write as _};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use tar::Archive;
use tracing::debug;

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
    let metadata = fs::metadata(dir.join("build.Dockerfile"))
        .await
        .context("Unable to get Dockerfile metadata")?;
    let mtime = FileTime::from_last_modification_time(&metadata);

    let write_tasks = vec![
        write_bin(
            "advisory-checker",
            twoliter_tool_advisory_checker::ADVISORY_CHECKER.reader(),
            &dir,
            mtime,
        ),
        write_bin(
            "buildsys",
            twoliter_tool_buildsys::BUILDSYS.reader(),
            &dir,
            mtime,
        ),
        write_bin("pcrsys", twoliter_tool_pcrsys::PCRSYS.reader(), &dir, mtime),
        write_bin(
            "pipesys",
            twoliter_tool_pipesys::PIPESYS.reader(),
            &dir,
            mtime,
        ),
        write_bin(
            "tuftool",
            twoliter_tool_tuftool::TUFTOOL.reader(),
            &dir,
            mtime,
        ),
        write_bin("ukisys", twoliter_tool_ukisys::UKISYS.reader(), &dir, mtime),
        write_bin(
            "eif-builder",
            twoliter_tool_eif_builder::EIF_BUILD_BIN.reader(),
            &dir,
            mtime,
        ),
        write_bin("unplug", twoliter_tool_unplug::UNPLUG.reader(), &dir, mtime),
        write_bin("krane", krane_bundle::KRANE_BIN.reader(), &dir, mtime),
        write_bin(
            "pubsys-setup",
            twoliter_tool_pubsys_setup::PUBSYS_SETUP.reader(),
            &dir,
            mtime,
        ),
        write_bin("pubsys", twoliter_tool_pubsys::PUBSYS.reader(), &dir, mtime),
    ];

    try_join_all(write_tasks).await?;

    // Apply the mtime to the directory now that the writes are done.
    set_file_mtime(dir, mtime).context(format!("Unable to set mtime for '{}'", dir.display()))?;

    debug!("Finished installing tools");

    Ok(())
}

async fn write_bin(
    name: &str,
    mut data: Box<dyn Read + Send + Sync + 'static>,
    dir: impl AsRef<Path>,
    mtime: FileTime,
) -> Result<()> {
    let path = dir.as_ref().join(name);

    let name = name.to_string();
    tokio::task::spawn_blocking(move || {
        let mut f = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(false)
            .write(true)
            .mode(0o755)
            .open(&path)
            .context(format!("Unable to create file '{}'", path.display()))?;

        {
            let mut writer = std::io::BufWriter::new(&mut f);
            std::io::copy(&mut data, &mut writer).context(format!(
                "Failed to decompress `{name}` to `{}`",
                path.display()
            ))?;
        }

        f.flush()
            .context(format!("Unable to finalize '{}'", path.display()))?;

        set_file_handle_times(&f, None, Some(mtime))
            .context(format!("Unable to set mtime for '{}'", path.display()))?;
        Ok(())
    })
    .await?
}

async fn unpack_tarball(tools_dir: impl AsRef<Path>) -> Result<()> {
    let tools_dir = tools_dir.as_ref();
    let tar = ZlibDecoder::new(twoliter_tool_embedded_bundle::EMBEDDED_BUNDLE);
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
    assert!(toolsdir.join("Makefile.toml").is_file());
    assert!(toolsdir.join("build.Dockerfile").is_file());
    assert!(toolsdir.join("build.Dockerfile.dockerignore").is_file());
    assert!(toolsdir.join("docker-go").is_file());
    assert!(toolsdir.join("eif-sign-helper").is_file());
    assert!(toolsdir.join("guest-images-helper").is_file());
    assert!(toolsdir.join("img2img").is_file());
    assert!(toolsdir.join("imghelper").is_file());
    assert!(toolsdir.join("metadata.spec").is_file());
    assert!(toolsdir.join("builder-group.spec").is_file());
    assert!(toolsdir.join("partyplanner").is_file());
    assert!(toolsdir.join("rpm2img").is_file());
    assert!(toolsdir.join("rpm2kit").is_file());
    assert!(toolsdir.join("rpm2kmodkit").is_file());
    assert!(toolsdir.join("rpm2migrations").is_file());
    assert!(toolsdir.join("rpm2eif").is_file());

    // Check that binaries were copied.
    assert!(toolsdir.join("advisory-checker").is_file());
    assert!(toolsdir.join("buildsys").is_file());
    assert!(toolsdir.join("pcrsys").is_file());
    assert!(toolsdir.join("pipesys").is_file());
    assert!(toolsdir.join("eif-builder").is_file());
    assert!(toolsdir.join("pubsys").is_file());
    assert!(toolsdir.join("pubsys-setup").is_file());
    assert!(toolsdir.join("tuftool").is_file());
    assert!(toolsdir.join("ukisys").is_file());
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
