use flate2::read::GzDecoder;
use std::fs::File;
use std::path::PathBuf;
use std::process::Command;
use std::{env, fs};
use tar::Archive;

/// Version of go-containerregistry to bundle. The prebuilt `krane` binary is
/// downloaded from that release on GitHub:
/// https://github.com/google/go-containerregistry/releases/tag/v0.21.8
const CRANE_VERSION: &str = "0.21.8";

fn main() {
    let script_dir = env::current_dir().unwrap();
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    println!("cargo::rerun-if-changed=../build-cache-fetch");
    println!("cargo::rerun-if-changed=hashes");

    // Pick the correct prebuilt archive for the current target. The upstream
    // release asset filenames do NOT include a version, so archives fetched
    // for different `CRANE_VERSION`s share the same on-disk name.
    let goreleaser_platform = goreleaser_platform();
    let archive_name = format!("go-containerregistry_{goreleaser_platform}.tar.gz");
    let hash_file = script_dir.join("hashes").join(&goreleaser_platform);

    if !hash_file.exists() {
        panic!(
            "No hash file for platform '{}' at '{}'. Add a hash entry to support this target.",
            goreleaser_platform,
            hash_file.display(),
        );
    }

    // Fetch and checksum-verify the release archive. See the README for how
    // the cache lookup and `UPSTREAM_SOURCE_FALLBACK` escape hatch work.
    env::set_current_dir(&out_dir).expect("Failed to set current directory");
    let fetch_status = Command::new(script_dir.join("../build-cache-fetch"))
        .arg(&hash_file)
        .status()
        .expect("Failed to execute build-cache-fetch");

    if !fetch_status.success() {
        panic!(
            "Failed to fetch krane release archive: build-cache-fetch exited with status {fetch_status}"
        );
    }

    // Extract the archive. The upstream release archives contain a flat layout
    // with the binaries (crane, gcrane, krane) at the top level alongside
    // LICENSE and README.md.
    let crane_archive = out_dir.join(&archive_name);
    let crane_tgz = File::open(&crane_archive).expect("Failed to open krane release archive");
    let extract_dir = out_dir.join(format!("go-containerregistry-{CRANE_VERSION}"));
    // Clean out any previous extraction so re-runs work reliably.
    if extract_dir.exists() {
        fs::remove_dir_all(&extract_dir).expect("Failed to clean previous extraction");
    }
    fs::create_dir_all(&extract_dir).expect("Failed to create extraction directory");
    let mut tar_archive = Archive::new(GzDecoder::new(crane_tgz));
    tar_archive
        .unpack(&extract_dir)
        .expect("Failed to extract krane release archive");

    let krane_binary = extract_dir.join("krane");
    if !krane_binary.exists() {
        panic!(
            "krane binary not found at '{}' after extracting release archive",
            krane_binary.display(),
        );
    }

    println!("cargo::rustc-env=KRANE_PATH={}", krane_binary.display());
}

/// Map the Cargo target to the platform string used in goreleaser's archive
/// names (e.g. `Linux_x86_64`, `Linux_arm64`).
fn goreleaser_platform() -> String {
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("Failed to read CARGO_CFG_TARGET_OS");
    let target_arch =
        env::var("CARGO_CFG_TARGET_ARCH").expect("Failed to read CARGO_CFG_TARGET_ARCH");

    let os = match target_os.as_str() {
        "linux" => "Linux",
        other => panic!("Unsupported target OS for prebuilt krane: {other}"),
    };
    let arch = match target_arch.as_str() {
        "x86_64" => "x86_64",
        "aarch64" => "arm64",
        other => panic!("Unsupported target architecture for prebuilt krane: {other}"),
    };
    format!("{os}_{arch}")
}
