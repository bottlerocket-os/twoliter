use std::{env, path::Path};

use duct::cmd;
use reqwest::Url;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufReader, Cursor};
use std::path::PathBuf;
use tar::Archive;
use tempfile::TempDir;
use which::which;

use crate::{run_command, twoliter_build::copy_project_to_temp_dir, TWOLITER_PATH};

const EXPECTED_INVENTORY_PATH: &str =
    "build/images/x86_64-aws-ecs-2/latest/application-inventory.json";

// Last Bottlerocket release that doesn't include source packages in application inventory.
// For older releases, we check that reference inventory is a subset of current.
const SOURCE_PACKAGE_INVENTORY_ANCHOR_VERSION: &str = "1.56.0";

#[derive(Serialize, Deserialize)]
pub struct GithubRelease {
    tag_name: String,
}

async fn find_latest_version(repository: &str) -> String {
    // We make a get request to fetch the github releases of twoliter
    let url = Url::parse(&format!(
        "https://api.github.com/repos/bottlerocket-os/{repository}/releases/latest"
    ))
    .expect("invalid url");
    let client = reqwest::Client::new();
    let mut request = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        // Pretend to be curl
        .header("User-Agent", "twoliter-ci");
    // Check if we have a GITHUB_TOKEN
    if let Some(token) = env::var("GITHUB_TOKEN").ok() {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .await
        .expect("failed to query github api for release list");
    let response_body = response
        .bytes()
        .await
        .expect("failed to get response from github api for release list");
    let release: GithubRelease =
        serde_json::from_slice(&response_body).expect("malformed data returned from github api");
    let tag_name = release.tag_name;
    let tag_name = tag_name
        .split_once(" ")
        .map(|x| x.0)
        .unwrap_or(tag_name.as_str());

    tag_name
        .strip_prefix("refs/tags/")
        .unwrap_or(tag_name)
        .to_string()
}

fn parse_version(release: &str) -> Version {
    let version = release.strip_prefix("v").unwrap_or(release);
    Version::parse(version).expect("failed to parse")
}

async fn create_bob(version: &str) -> TempDir {
    let tmp_dir = TempDir::new().expect("failed to create temporary directory");
    let git = which("git").expect("failed to find git");
    let inside = tmp_dir.path().to_str().unwrap();
    cmd!(
        git,
        "clone",
        "-b",
        version,
        "https://github.com/bottlerocket-os/bottlerocket.git",
        inside
    )
    .run()
    .expect("failed to clone bob");
    tmp_dir
}

// Installs twoliter from the latest non-rc release on github
async fn install_twoliter_from_release() -> (TempDir, PathBuf) {
    let tmp_dir = TempDir::new().expect("failed to create temporary directory");
    let twoliter_bin = tmp_dir.path().join("twoliter");
    let latest = find_latest_version("twoliter").await;
    let arch = env::consts::ARCH;
    let url = Url::parse(&format!(
        "https://github.com/bottlerocket-os/twoliter/releases/download/{latest}/twoliter-{arch}-unknown-linux-musl.tar.xz"
    )).expect("failed to parse twoliter url");
    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .header("User-Agent", "twoliter-ci")
        .send()
        .await
        .expect("failed to download twoliter release archive");
    let mut archive_bytes = BufReader::new(Cursor::new(
        response
            .bytes()
            .await
            .expect("failed to fetch twoliter release archive"),
    ));
    let mut decompressed_bytes: Vec<u8> = Vec::new();
    lzma_rs::xz_decompress(&mut archive_bytes, &mut decompressed_bytes)
        .expect("failed to decompress archive");
    let mut archive = Archive::new(Cursor::new(decompressed_bytes));
    let mut twoliter = File::create(&twoliter_bin).expect("failed to create twoliter binary");
    // Now iterate through the archive till we find the twoliter binary
    let mut entries = archive.entries().expect("failed to get archive iterator");
    while let Some(Ok(mut entry)) = entries.next() {
        if entry.path().unwrap().ends_with("twoliter") {
            std::io::copy(&mut entry, &mut twoliter).expect("failed to write twoliter executable");
        }
    }
    // Make the file executable
    cmd!("chmod", "+x", &twoliter_bin)
        .run()
        .expect("failed to make twoliter executable");
    (tmp_dir, twoliter_bin)
}

#[derive(Deserialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "PascalCase")]
struct ContentView {
    name: String,
    architecture: String,
    version: String,
    release: String,
    publisher: String,
    epoch: String,
}

#[derive(Deserialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "PascalCase")]
struct InventoryView {
    content: Vec<ContentView>,
}

#[tokio::test]
#[ignore]
async fn test_twoliter_application_inventory() {
    // We clone the latest release bob
    let bob_version = find_latest_version("bottlerocket").await;
    let bob_src = create_bob(bob_version.as_str()).await;

    // Now we want to first get the last released twoliter so we can build our reference inventory file
    let (_tmp_dir, latest_twoliter) = install_twoliter_from_release().await;
    // Build and get the application inventory file contents with this twoliter
    let reference_aif = build_and_fetch_application_inventory(&latest_twoliter, bob_src.path());
    let reference_set: InventoryView = serde_json::from_str(reference_aif.as_str())
        .expect("failed to deserialize reference inventory file");
    // Now we want to build with the twoliter we built for this test (head)
    let current_aif = build_and_fetch_application_inventory(TWOLITER_PATH, bob_src.path());
    let current_set: InventoryView = serde_json::from_str(current_aif.as_str())
        .expect("failed to deserialize current inventory file");

    // Last Bottlerocket release that doesn't include source packages in application inventory.
    // For older releases, we check that reference inventory is a subset of current.
    let anchor = parse_version(SOURCE_PACKAGE_INVENTORY_ANCHOR_VERSION);
    let bob_ver = parse_version(&bob_version);

    // For BOB versions before source package support, check that reference is a subset of current
    // current may have additional source package entries
    if bob_ver <= anchor {
        for item in &reference_set.content {
            assert!(
                current_set.content.contains(item),
                "current inventory missing package from reference: {:?}",
                item
            );
        }
    } else {
        assert_eq!(
            reference_set, current_set,
            "twoliter generated different application inventory than last released twoliter"
        );
    }
}

fn build_and_fetch_application_inventory(
    twoliter: impl AsRef<Path>,
    project: impl AsRef<Path>,
) -> String {
    let tmp_dir = copy_project_to_temp_dir(project);
    let twoliter_path = twoliter.as_ref().to_str().unwrap();
    let project_path = tmp_dir.path().join("Twoliter.toml");
    let project_path = project_path.to_str().unwrap();
    let cmd_env = [];
    // Twoliter update
    let output = run_command(
        twoliter_path,
        ["update", "--project-path", project_path],
        cmd_env,
    );
    assert!(output.status.success(), "failed to run twoliter update");
    // Twoliter fetch
    let output = run_command(
        twoliter_path,
        ["fetch", "--project-path", project_path, "--arch", "x86_64"],
        cmd_env,
    );
    assert!(output.status.success(), "failed to run twoliter fetch");
    // Build the variant
    let output = run_command(
        twoliter_path,
        [
            "build",
            "variant",
            "aws-ecs-2",
            "--project-path",
            project_path,
            "--arch",
            "x86_64",
        ],
        cmd_env,
    );
    assert!(
        output.status.success(),
        "failed to run twoliter build variant"
    );
    // Now we should have the application inventory file
    let aif = tmp_dir.path().join(EXPECTED_INVENTORY_PATH);
    assert!(
        aif.exists(),
        "twoliter did not generate application-inventory.json"
    );
    std::fs::read_to_string(&aif).expect("failed to read application-inventory.json file")
}
