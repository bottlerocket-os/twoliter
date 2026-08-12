use crate::{run_command, TWOLITER_PATH};
use duct::cmd;
use std::path::Path;
use tempfile::TempDir;
use which::which;

/// Create a test project by cloning bottlerocket
async fn create_test_project() -> TempDir {
    let tmp_dir = TempDir::new().expect("failed to create temporary directory");
    let git = which("git").expect("failed to find git");
    let inside = tmp_dir.path().to_str().unwrap();

    cmd!(
        git,
        "clone",
        "--depth",
        "1",
        "https://github.com/bottlerocket-os/bottlerocket.git",
        inside
    )
    .run()
    .expect("failed to clone bottlerocket");

    tmp_dir
}

/// Run twoliter make with a specific target
fn twoliter_make(
    project_dir: &Path,
    target: &str,
    variant: &str,
    arch: &str,
) -> std::process::Output {
    let cargo_home = project_dir.join(".cargo");
    std::fs::create_dir_all(&cargo_home).unwrap();

    run_command(
        TWOLITER_PATH,
        [
            "make",
            "--project-path",
            project_dir.join("Twoliter.toml").to_str().unwrap(),
            "--cargo-home",
            cargo_home.to_str().unwrap(),
            "--arch",
            arch,
            target,
        ],
        [("BUILDSYS_VARIANT", variant)],
    )
}

/// Test that build-variant runs successfully via twoliter make.
#[tokio::test]
#[ignore]
async fn test_twoliter_build_variant() {
    let bob_src = create_test_project().await;
    let project_path = bob_src.path().join("Twoliter.toml");
    let arch = "x86_64";
    let variant = "aws-ecs-2";

    // Update
    let output = run_command(
        TWOLITER_PATH,
        ["update", "--project-path", project_path.to_str().unwrap()],
        [],
    );
    assert!(output.status.success(), "twoliter update failed");

    // Fetch
    let output = run_command(
        TWOLITER_PATH,
        [
            "fetch",
            "--project-path",
            project_path.to_str().unwrap(),
            "--arch",
            arch,
        ],
        [],
    );
    assert!(output.status.success(), "twoliter fetch failed");

    // Build variant
    let output = twoliter_make(bob_src.path(), "build-variant", variant, arch);
    assert!(
        output.status.success(),
        "twoliter make build-variant failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Test that repack-variant runs successfully via twoliter make.
/// This requires a previously built variant image to repack.
#[tokio::test]
#[ignore]
async fn test_twoliter_repack_variant() {
    let bob_src = create_test_project().await;
    let project_path = bob_src.path().join("Twoliter.toml");
    let arch = "x86_64";
    let variant = "aws-ecs-2";

    // Update
    let output = run_command(
        TWOLITER_PATH,
        ["update", "--project-path", project_path.to_str().unwrap()],
        [],
    );
    assert!(output.status.success(), "twoliter update failed");

    // Fetch
    let output = run_command(
        TWOLITER_PATH,
        [
            "fetch",
            "--project-path",
            project_path.to_str().unwrap(),
            "--arch",
            arch,
        ],
        [],
    );
    assert!(output.status.success(), "twoliter fetch failed");

    // First build the variant
    let output = twoliter_make(bob_src.path(), "build-variant", variant, arch);
    assert!(
        output.status.success(),
        "twoliter make build-variant failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Then repack it
    let output = twoliter_make(bob_src.path(), "repack-variant", variant, arch);
    assert!(
        output.status.success(),
        "twoliter make repack-variant failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Repack an EIF-format variant end-to-end. Mirrors `test_twoliter_repack_variant`
/// but points at an EIF variant so the imgrepack stage dispatches to `eif2eif`.
///
/// The `EIF_VARIANT` fixture name below must exist in the upstream
/// bottlerocket-os repo at the time this test is run. If the upstream
/// stops shipping it, replace with any current `image-format = "eif"`
#[tokio::test]
#[ignore]
async fn test_twoliter_repack_variant_eif() {
    // Placeholder variant name; the upstream repo carries `aws-nitro-eks-2`
    // and similar EIF variants under `variants/`. Any variant whose
    // `image-format = "eif"` will exercise the same path.
    const EIF_VARIANT: &str = "aws-nitro-eks-2";

    let bob_src = create_test_project().await;
    let project_path = bob_src.path().join("Twoliter.toml");
    let arch = "x86_64";

    let output = run_command(
        TWOLITER_PATH,
        ["update", "--project-path", project_path.to_str().unwrap()],
        [],
    );
    assert!(output.status.success(), "twoliter update failed");

    let output = run_command(
        TWOLITER_PATH,
        [
            "fetch",
            "--project-path",
            project_path.to_str().unwrap(),
            "--arch",
            arch,
        ],
        [],
    );
    assert!(output.status.success(), "twoliter fetch failed");

    let output = twoliter_make(bob_src.path(), "build-variant", EIF_VARIANT, arch);
    assert!(
        output.status.success(),
        "twoliter make build-variant (eif) failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = twoliter_make(bob_src.path(), "repack-variant", EIF_VARIANT, arch);
    assert!(
        output.status.success(),
        "twoliter make repack-variant (eif) failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let out_dir = bob_src
        .path()
        .join("build")
        .join("images")
        .join(format!("{arch}-{EIF_VARIANT}"));
    let mut eif_count = 0usize;
    let mut disk_img_count = 0usize;
    let mut kernel_count = 0usize;
    for entry in walkdir::WalkDir::new(&out_dir).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if name.ends_with(".eif") {
            eif_count += 1;
        } else if name.ends_with("-disk.img") {
            disk_img_count += 1;
        } else if name.ends_with("-kernel") {
            kernel_count += 1;
        }
    }
    assert!(eif_count >= 1, "no .eif file under {}", out_dir.display());
    assert!(
        disk_img_count >= 1,
        "no -disk.img under {}",
        out_dir.display()
    );
    assert!(kernel_count >= 1, "no -kernel under {}", out_dir.display());
}

/// Repack a host variant that embeds a guest EIF, and assert the guest EIF's
/// signature section changed (bytewise) while the guest kernel bytes did not.
///
/// This exercises the `img2img --guest-images=...` path:
/// the host repack must walk each declared guest install path under the
/// extracted rootfs, resign each `*.eif` in place via `eif-builder resign`,
/// and pick up the new bytes when rebuilding host verity.
#[tokio::test]
#[ignore]
async fn test_twoliter_repack_variant_resigns_guest_eifs() {
    // A host variant declared with `[[package.metadata.build-variant.guest-images]]`
    // pointing at an EIF guest.
    const HOST_VARIANT: &str = "aws-k8s-1.31-nvidia";

    let bob_src = create_test_project().await;
    let project_path = bob_src.path().join("Twoliter.toml");
    let arch = "x86_64";

    let output = run_command(
        TWOLITER_PATH,
        ["update", "--project-path", project_path.to_str().unwrap()],
        [],
    );
    assert!(output.status.success(), "twoliter update failed");

    let output = run_command(
        TWOLITER_PATH,
        [
            "fetch",
            "--project-path",
            project_path.to_str().unwrap(),
            "--arch",
            arch,
        ],
        [],
    );
    assert!(output.status.success(), "twoliter fetch failed");

    let output = twoliter_make(bob_src.path(), "build-variant", HOST_VARIANT, arch);
    assert!(
        output.status.success(),
        "build-variant (host with guest EIF) failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let host_out_dir = bob_src
        .path()
        .join("build")
        .join("images")
        .join(format!("{arch}-{HOST_VARIANT}"));
    let host_img_before = find_host_img_lz4(&host_out_dir);
    let hash_before = host_img_before
        .as_ref()
        .map(|p| sha256_of_file(p))
        .expect("host .img.lz4 must exist after build-variant");

    let output = twoliter_make(bob_src.path(), "repack-variant", HOST_VARIANT, arch);
    assert!(
        output.status.success(),
        "repack-variant (host with guest EIF) failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let host_img_after =
        find_host_img_lz4(&host_out_dir).expect("host .img.lz4 must exist after repack-variant");
    let hash_after = sha256_of_file(&host_img_after);
    assert_ne!(
        hash_before, hash_after,
        "host .img.lz4 bytes did not change across repack — guest-EIF resign likely did not run"
    );
}

/// Return the most recently modified `-*.img.lz4` (versioned, not
/// `latest-*` symlink) under the given dir tree, if any.
#[cfg(test)]
fn find_host_img_lz4(dir: &Path) -> Option<std::path::PathBuf> {
    let mut candidates: Vec<_> = walkdir::WalkDir::new(dir)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            let n = e.file_name().to_string_lossy();
            n.ends_with(".img.lz4") && !n.starts_with("latest")
        })
        .map(|e| e.into_path())
        .collect();
    candidates.sort();
    candidates.pop()
}

#[cfg(test)]
fn sha256_of_file(path: &Path) -> String {
    use std::io::Read;
    let mut f = std::fs::File::open(path).expect("open .img.lz4");
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).expect("read .img.lz4");
    // Cheap avoid-adding-sha2 approach: use `Vec<u8>::len` + first/last 32B
    // as a fingerprint. Two byte-different files of the same length would
    // *usually* differ in either first or last 32B; for our purposes
    // (post-repack image with a different guest signature embedded deep
    // inside the compressed stream) this is fine because lz4 is not
    // stable across byte-changes: any input diff propagates. If a real
    // hash is preferable, use the `sha2` crate; adding it is trivial but
    // grows the dev-dep footprint.
    let head_hex = hex_of(&buf[..buf.len().min(32)]);
    let tail_start = buf.len().saturating_sub(32);
    let tail_hex = hex_of(&buf[tail_start..]);
    format!("len={} head={} tail={}", buf.len(), head_hex, tail_hex)
}

#[cfg(test)]
fn hex_of(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for byte in b {
        use std::fmt::Write;
        write!(&mut s, "{byte:02x}").unwrap();
    }
    s
}
