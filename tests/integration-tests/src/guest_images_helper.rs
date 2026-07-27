use crate::run_command;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Path to the `guest-images-helper` shell file embedded by twoliter and sourced by `rpm2img`.
fn helper_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("twoliter/embedded/guest-images-helper")
}

/// Build a synthetic guest image directory mimicking the layout produced by a real variant
/// build. Includes:
///   - the bootable image artifacts that should be copied (standard + EIF)
///   - the build-metadata files that must NOT be copied (the bug we're guarding against)
///   - a stable-name symlink (`os_image.img.lz4` -> `bottlerocket-…img.lz4`)
fn populate_synthetic_guest_dir(dir: &Path) {
    // Bootable artifacts (these are the ones that SHOULD be copied).
    for f in [
        "bottlerocket-inner-x86_64-1.0.0-0.img.lz4",
        "bottlerocket-inner-x86_64-1.0.0-0-data.img.lz4",
        "bottlerocket-inner-x86_64-1.0.0-0-boot.ext4.lz4",
        "bottlerocket-inner-x86_64-1.0.0-0-root.ext4.lz4",
        "bottlerocket-inner-x86_64-1.0.0-0-root.verity.lz4",
        "bottlerocket-inner-x86_64-1.0.0-0.qcow2",
        "bottlerocket-inner-x86_64-1.0.0-0.vmdk",
    ] {
        std::fs::write(dir.join(f), b"image-bytes").unwrap();
    }

    // EIF artifacts produced by rpm2eif (hyphen-separated names).
    for f in [
        "bottlerocket-inner-x86_64-1.0.0-0.eif",
        "bottlerocket-inner-x86_64-1.0.0-0-disk.img",
        "bottlerocket-inner-x86_64-1.0.0-0-kernel",
    ] {
        std::fs::write(dir.join(f), b"eif-bytes").unwrap();
    }

    // Stable-name symlinks.
    std::os::unix::fs::symlink(
        "bottlerocket-inner-x86_64-1.0.0-0.img.lz4",
        dir.join("os_image.img.lz4"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        "bottlerocket-inner-x86_64-1.0.0-0.eif",
        dir.join("latest.eif"),
    )
    .unwrap();

    // Build metadata + SBOMs (these MUST NOT be copied).
    for f in [
        "application-inventory.json",
        "artifact-metadata.json",
        "pcr-predictions.json",
        "bottlerocket-inner-x86_64-1.0.0-0-sbom-os.spdx.json",
        "bottlerocket-inner-x86_64-1.0.0-0-sbom-os.cdx.json",
    ] {
        std::fs::write(dir.join(f), b"metadata-bytes").unwrap();
    }
    // A stray non-image binary (e.g. left-over from a tool) — must not be copied either.
    std::fs::write(dir.join("README.txt"), b"text").unwrap();
}

/// Source the helper, run `copy_guest_image_artifacts`, and return the destination directory
/// for inspection.
fn run_copy(src: &Path, dst: &Path) -> std::process::Output {
    let helper = helper_path();
    // `guest-images-helper` requires `IMAGE_ARTIFACT_SUFFIXES` and `IMAGE_ARTIFACT_GLOBS`
    // to be pre-set (normally by sourcing `imghelper`, which is not safe to source here
    // because it hard-fails on missing build-env vars). Declare both arrays directly to
    // exercise just the helper.
    let script = format!(
        r#"
        set -euo pipefail
        IMAGE_ARTIFACT_SUFFIXES=(img.lz4 qcow2 vmdk ova ext4.lz4 verity.lz4 eif)
        IMAGE_ARTIFACT_GLOBS=("*-disk.img" "*-kernel")
        source "{helper}"
        copy_guest_image_artifacts "{src}" "{dst}"
        "#,
        helper = helper.display(),
        src = src.display(),
        dst = dst.display(),
    );
    run_command("bash", ["-c", script.as_str()], [])
}

fn entries_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// All bootable image artifacts (and the stable-name symlink) end up at the destination, and
/// none of the JSON / SBOM / README files do. This is the regression test for the original
/// `cp -a` finding that copied the entire guest image directory verbatim.
#[test]
fn test_copy_guest_image_artifacts_filters_metadata_and_sboms() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dst = temp.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    populate_synthetic_guest_dir(&src);

    let output = run_copy(&src, &dst);
    assert!(
        output.status.success(),
        "copy_guest_image_artifacts failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let copied = entries_in(&dst);

    // Required: every bootable artifact + EIF artifacts + symlinks. Order independent.
    for required in [
        "bottlerocket-inner-x86_64-1.0.0-0.img.lz4",
        "bottlerocket-inner-x86_64-1.0.0-0-data.img.lz4",
        "bottlerocket-inner-x86_64-1.0.0-0-boot.ext4.lz4",
        "bottlerocket-inner-x86_64-1.0.0-0-root.ext4.lz4",
        "bottlerocket-inner-x86_64-1.0.0-0-root.verity.lz4",
        "bottlerocket-inner-x86_64-1.0.0-0.qcow2",
        "bottlerocket-inner-x86_64-1.0.0-0.vmdk",
        "bottlerocket-inner-x86_64-1.0.0-0.eif",
        "bottlerocket-inner-x86_64-1.0.0-0-disk.img",
        "bottlerocket-inner-x86_64-1.0.0-0-kernel",
        "os_image.img.lz4",
        "latest.eif",
    ] {
        assert!(
            copied.iter().any(|n| n == required),
            "expected {required:?} in dst, got {copied:?}"
        );
    }

    // Forbidden: no JSON, no SBOM, no README. These would leak guest build metadata into
    // the host rootfs and are exactly what the `cp -a` regression copied.
    for forbidden in copied.iter() {
        assert!(
            !forbidden.ends_with(".json"),
            "metadata file {forbidden:?} must not be copied (came from src JSON)"
        );
        assert!(
            !forbidden.contains("-sbom-"),
            "SBOM file {forbidden:?} must not be copied"
        );
        assert!(
            forbidden != "README.txt",
            "non-image file {forbidden:?} must not be copied"
        );
    }

    // The symlink must remain a symlink (so its target name is preserved verbatim) and
    // must point at one of the real artifacts in the same directory.
    let symlink_meta = std::fs::symlink_metadata(dst.join("os_image.img.lz4")).unwrap();
    assert!(
        symlink_meta.file_type().is_symlink(),
        "os_image.img.lz4 should remain a symlink at the destination"
    );
    let target = std::fs::read_link(dst.join("os_image.img.lz4")).unwrap();
    assert_eq!(
        target.to_string_lossy(),
        "bottlerocket-inner-x86_64-1.0.0-0.img.lz4",
        "symlink target should be preserved verbatim"
    );
}

/// If a guest directory holds no recognizable image artifacts, the helper returns non-zero so
/// the caller can fail the build instead of silently producing an empty install path.
#[test]
fn test_copy_guest_image_artifacts_fails_on_empty_input() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dst = temp.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    // Only metadata files: nothing the helper considers an artifact.
    std::fs::write(src.join("application-inventory.json"), b"{}").unwrap();
    std::fs::write(src.join("README.txt"), b"text").unwrap();

    let output = run_copy(&src, &dst);
    assert!(
        !output.status.success(),
        "copy_guest_image_artifacts should fail when no artifacts match"
    );
    assert!(
        entries_in(&dst).is_empty(),
        "no files should have been copied; got {:?}",
        entries_in(&dst)
    );
}

/// Filenames containing spaces must round-trip correctly through the find/while pipeline.
/// This guards against accidentally re-introducing word-splitting bugs in the copy logic.
#[test]
fn test_copy_guest_image_artifacts_handles_spaces_in_names() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dst = temp.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    let weird = "name with spaces.img.lz4";
    std::fs::write(src.join(weird), b"image").unwrap();

    let output = run_copy(&src, &dst);
    assert!(
        output.status.success(),
        "copy_guest_image_artifacts failed on space-in-name: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        dst.join(weird).is_file(),
        "expected {weird:?} to be copied; dst contents: {:?}",
        entries_in(&dst)
    );
}
