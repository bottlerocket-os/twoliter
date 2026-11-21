use super::{
    check_twoliter_version, checkout, clean_up, compare_and_report, git, prepare, run_build,
};
use crate::common::fs;
use crate::project::lock::image::{EncodedKitMetadata, ImageMetadata, LockedImage};
use crate::project::lock::views::{ManifestListView, ManifestView};
use anyhow::{ensure, Context, Result};
use base64::Engine;
use clap::Parser;
use oci_cli_wrapper::{ConfigView, ImageTool};
use path_absolutize::Absolutize;
use semver::Version;
use serde::Deserialize;
use serde_json::Value;
use sha2::Sha256;
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tracing::{info, warn};

const DEFAULT_REGISTRY: &str = "public.ecr.aws/bottlerocket";

/// Kits are cloned from `<DEFAULT_GITHUB_ORG>/<kit>` unless --repo is given.
const DEFAULT_GITHUB_ORG: &str = "https://github.com/bottlerocket-os";

/// Architectures kits are built for, with the name the OCI image index uses for each.
const ARCHES: &[(&str, &str)] = &[("x86_64", "amd64"), ("aarch64", "arm64")];

/// The architecture of the SDK Bottlerocket kits are built with, for every target architecture.
const DEFAULT_SDK_ARCH: &str = "x86_64";

/// The first twoliter release whose kit builds are expected to reproduce: the first whose rpm2kit
/// writes reproducible repository metadata and orders the layers by name.
const MIN_REPRODUCIBLE_KIT_TWOLITER: Version = Version::new(0, 28, 0);

/// Rebuild a kit from source and compare it against the published kit image.
///
/// Needs Docker, git and make. The kit repository must build the kit with `make build`, as the
/// Bottlerocket kit repositories do, so that it is rebuilt with the twoliter version it pins.
#[derive(Debug, Parser)]
pub(crate) struct VerifyKit {
    /// The kit to verify, such as bottlerocket-core-kit. This is the image repository name the kit
    /// is published under.
    pub(crate) kit: String,

    /// The version of the kit to verify, such as 17.3.0.
    pub(crate) version: String,

    /// The container registry the kit is published to. The kit image is
    /// `<registry>/<kit>:v<version>`.
    #[clap(long, default_value = DEFAULT_REGISTRY)]
    pub(crate) registry: String,

    /// The git repository to clone the source from. Defaults to
    /// `https://github.com/bottlerocket-os/<kit>`. Conflicts with --source-dir.
    #[clap(long, conflicts_with = "source_dir")]
    pub(crate) repo: Option<String>,

    /// An existing clean clone of the source repository to build from. The kit's commit is
    /// checked out in it.
    #[clap(long = "source-dir")]
    pub(crate) source_dir: Option<PathBuf>,

    /// The commit the kit was built from. Defaults to the commit tagged `v<version>`.
    #[clap(long)]
    pub(crate) commit: Option<String>,

    /// An architecture to verify, x86_64 or aarch64. Can be given more than once. Defaults to
    /// every architecture the kit is published for.
    #[clap(long = "arch", value_parser = ["x86_64", "aarch64"])]
    pub(crate) arches: Vec<String>,

    /// Architecture of the SDK to build with; must match the host that built the published kit.
    /// Other architectures run under QEMU emulation.
    #[clap(long = "sdk-arch", value_parser = ["x86_64", "aarch64"], default_value = DEFAULT_SDK_ARCH)]
    pub(crate) sdk_arch: String,

    /// Directory for the source checkout, kit images and report. Defaults to
    /// `./twoliter-verify-<kit>-v<version>`.
    #[clap(long = "work-dir")]
    pub(crate) work_dir: Option<PathBuf>,

    /// Keep the source checkout and kit images in the work directory when done. The report is
    /// always kept.
    #[clap(long)]
    pub(crate) keep: bool,

    /// Show the build output instead of writing it to `build-<arch>.log` in the work directory,
    /// and show full hashes in the summary.
    #[clap(long)]
    pub(crate) verbose: bool,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    config: ManifestView,
    layers: Vec<ManifestView>,
}

/// The part of Twoliter.lock that pins the SDK.
#[derive(Debug, Deserialize)]
struct Lock {
    sdk: LockedImage,
}

impl VerifyKit {
    pub(super) async fn run(&self) -> Result<()> {
        let mut cleanup = Vec::new();
        let result = self.verify(&mut cleanup).await;
        clean_up(self.keep, &cleanup).await;
        result
    }

    /// Verify the kit, adding the directories to delete when done to `cleanup`.
    async fn verify(&self, cleanup: &mut Vec<PathBuf>) -> Result<()> {
        let version = self.version.trim_start_matches('v');
        let parsed_version = Version::parse(version)
            .with_context(|| format!("'{}' is not a valid kit version", self.version))?;
        validate_kit_name(&self.kit)?;
        let (work_dir, tools) = prepare(
            &["git", "make", "docker"],
            self.work_dir.as_deref(),
            format!("twoliter-verify-{}-v{version}", self.kit),
        )
        .await?;

        let image_tool = ImageTool::from_builtin_krane();
        let repository = format!("{}/{}", self.registry, self.kit);
        let image = format!("{repository}:v{version}");
        let index_bytes = image_tool
            .get_manifest(&image)
            .await
            .with_context(|| format!("Unable to fetch the image index of '{image}'"))?;
        let index: ManifestListView = serde_json::from_slice(&index_bytes)
            .with_context(|| format!("'{image}' is not a multi-platform kit image"))?;
        let published = select_arches(&index, &self.arches).with_context(|| {
            format!("Unable to select the architectures of '{image}' to verify")
        })?;
        // Download the published images first: their metadata names the kit to build.
        let images_dir = work_dir.join("images");
        cleanup.push(images_dir.clone());
        let mut images = Vec::new();
        for (arch, digest) in &published {
            let dir = images_dir.join(arch).join("published");
            info!("Downloading the published {arch} kit {repository}@{digest}");
            if dir.exists() {
                fs::remove_dir_all(&dir).await?;
            }
            fs::create_dir_all(&dir).await?;
            // krane checks the manifest against the digest it is pulled by.
            image_tool
                .pull_oci_image(&dir, &format!("{repository}@{digest}"))
                .await
                .with_context(|| format!("Unable to download the published {arch} kit"))?;
            images.push((arch, dir, digest));
        }

        let (_, dir, digest) = &images[0];
        let manifest: Manifest = read_json(&blob(dir, digest)?).await?;
        let config: Value = read_json(&blob(dir, &manifest.config.digest)?).await?;
        let metadata = kit_metadata(&config)?;
        ensure!(
            metadata.version == parsed_version,
            "'{image}' holds version {} of kit {}, not {version}",
            metadata.version,
            metadata.name
        );
        ensure!(
            metadata.name == self.kit,
            "'{image}' holds kit {}, not {}",
            metadata.name,
            self.kit
        );

        let (source_dir, cloned) = match &self.source_dir {
            Some(dir) => (dir.absolutize()?.into_owned(), false),
            None => (work_dir.join("source"), true),
        };
        let default_repo = format!("{DEFAULT_GITHUB_ORG}/{}", self.kit);
        let repo = self.repo.as_deref().unwrap_or(&default_repo);
        if cloned {
            cleanup.push(source_dir.clone());
        }
        let commit = checkout(
            &source_dir,
            cloned.then_some(repo),
            self.commit.as_deref(),
            version,
            "kit",
        )
        .await?;
        let kit_dir = source_dir.join("kits").join(&metadata.name);
        ensure!(
            source_dir.join("Makefile").is_file() && kit_dir.join("Cargo.toml").is_file(),
            "'{}' needs a Makefile and kit {} in '{}'; kits are rebuilt with `make build`",
            source_dir.display(),
            metadata.name,
            kit_dir.display()
        );
        check_twoliter_version(&source_dir, &MIN_REPRODUCIBLE_KIT_TWOLITER).await?;
        let commit_time = git(&source_dir, &["show", "-s", "--format=%ct", &commit]).await?;

        let mut build_logs = Vec::new();
        let build = async {
            let sdk = Sdk::pull(&source_dir, &self.sdk_arch).await?;
            for (arch, _, _) in &images {
                let log = (!self.verbose).then(|| work_dir.join(format!("build-{arch}.log")));
                let mut cmd = Command::new("make");
                cmd.arg("build")
                    .arg(format!("KIT={}", metadata.name))
                    .arg(format!("ARCH={arch}"))
                    .env("DOCKER_DEFAULT_PLATFORM", &sdk.platform);
                let what = format!("{} {arch}", metadata.name);
                run_build(cmd, &source_dir, log.as_deref(), &what).await?;
                build_logs.extend(log);
                let archive = find_archive(&source_dir, &metadata.name, version, &commit, arch)?;
                unpack(&archive, &images_dir.join(arch).join("rebuilt")).await?;
            }
            for (arch, dir, digest) in &images {
                let regenerated = images_dir.join(arch).join("regenerated");
                info!("Regenerating the repository metadata of the published {arch} packages");
                sdk.createrepo(dir, digest, &regenerated, &commit_time)
                    .await
                    .with_context(|| {
                        format!("Unable to regenerate the published {arch} repository metadata")
                    })?;
            }
            Ok::<_, anyhow::Error>(())
        };
        // Stop the pull and build on Ctrl-C rather than exiting, so that the SDK tag is restored.
        let built = tokio::select! {
            built = build => built,
            _ = tokio::signal::ctrl_c() => Err(anyhow::anyhow!("Interrupted")),
        };
        built?;

        let report_path = work_dir.join("report.json");
        let mut compare = Command::new(tools.path().join("reprosys"));
        compare
            .arg("compare-kit")
            .arg("--images-dir")
            .arg(&images_dir)
            .arg("--report")
            .arg(&report_path);
        for (arch, _, digest) in &images {
            let regenerated = images_dir.join(arch).join("regenerated/repodata");
            compare.arg("--arch").arg(format!("{arch}={digest}"));
            compare
                .arg("--regenerated")
                .arg(format!("{arch}={}", regenerated.display()));
        }
        if self.verbose {
            compare.arg("--verbose");
        }
        let mut files = vec![("Report:", report_path.as_path())];
        files.extend(build_logs.iter().map(|log| ("Build log:", log.as_path())));
        let verified: Vec<String> = images
            .iter()
            .map(|(arch, _, digest)| format!("{arch}@{digest}"))
            .collect();
        let what = format!("Kit '{repository}' ({})", verified.join(", "));
        compare_and_report(compare, &files, &what, &commit).await
    }
}

/// The SDK image tagged for --sdk-arch under its Twoliter.lock name for the build; restores the
/// previous tag on drop.
///
/// The build uses whatever image Docker has under the SDK name, and pulls one for the Docker
/// host's platform only when there is none. Setting `DOCKER_DEFAULT_PLATFORM` for the build makes
/// it use the tagged image whatever the host's architecture.
struct Sdk {
    /// The name the build uses, from Twoliter.lock.
    name: String,
    /// The Docker platform of the image, such as `linux/amd64`.
    platform: String,
    /// The ID of the image Docker had under `name` before, if any.
    previous: Option<String>,
}

impl Sdk {
    /// Pull the SDK pinned in Twoliter.lock for `sdk_arch` and tag it with the name the build uses.
    async fn pull(dir: &Path, sdk_arch: &str) -> Result<Self> {
        let lock: Lock = toml::from_str(&fs::read_to_string(dir.join("Twoliter.lock")).await?)
            .context("Unable to parse Twoliter.lock")?;
        let name = lock.sdk.source;
        // The lock records the base64 SHA-256 of the SDK's image index.
        let digest = base64::engine::general_purpose::STANDARD
            .decode(&lock.sdk.digest)
            .ok()
            .filter(|digest| digest.len() == 32)
            .with_context(|| {
                format!(
                    "Twoliter.lock pins SDK digest '{}', which is not a base64 SHA-256",
                    lock.sdk.digest
                )
            })?;
        // The source is always tagged `v<version>`.
        let tag = format!(":v{}", lock.sdk.version);
        let repository = name
            .strip_suffix(&tag)
            .with_context(|| format!("Twoliter.lock SDK source '{name}' is not tagged '{tag}'"))?;
        let pinned = format!(
            "{repository}@sha256:{:x}",
            sha2::digest::Output::<Sha256>::from_slice(&digest)
        );
        let (_, docker_arch) = ARCHES
            .iter()
            .find(|(arch, _)| *arch == sdk_arch)
            .with_context(|| format!("Unknown SDK architecture '{sdk_arch}'"))?;
        let platform = format!("linux/{docker_arch}");
        let host_arch = docker(&["version", "--format", "{{.Server.Arch}}"]).await?;
        if host_arch != *docker_arch {
            info!(
                "The Docker host is {host_arch}, so the {sdk_arch} SDK runs under emulation, which \
                 needs QEMU user-mode emulation registered with binfmt_misc"
            );
        }
        let previous = docker(&["image", "inspect", "--format", "{{.Id}}", &name])
            .await
            .ok();
        info!("Pulling the {platform} SDK {pinned}");
        docker(&["pull", "--quiet", "--platform", &platform, &pinned])
            .await
            .with_context(|| {
                format!("Unable to pull the SDK {pinned} pinned in Twoliter.lock for {platform}")
            })?;
        // From here on, dropping restores the previous tag.
        let sdk = Self {
            name,
            platform,
            previous,
        };
        docker(&["tag", &pinned, &sdk.name]).await?;
        Ok(sdk)
    }
}

impl Sdk {
    /// Regenerate the repository metadata of the published kit image with manifest `digest` in
    /// the OCI layout `layout`, as rpm2kit generates it: unpack its package layers into `dir`
    /// and run the SDK's createrepo_c there, with the commit time as the metadata's revision and
    /// timestamps. The metadata is written to `dir/repodata`.
    async fn createrepo(
        &self,
        layout: &Path,
        digest: &str,
        dir: &Path,
        commit_time: &str,
    ) -> Result<()> {
        if dir.exists() {
            fs::remove_dir_all(dir).await?;
        }
        fs::create_dir_all(dir).await?;
        let manifest: Manifest = read_json(&blob(layout, digest)?).await?;
        let layers = manifest
            .layers
            .iter()
            .map(|layer| blob(layout, &layer.digest))
            .collect::<Result<Vec<_>>>()?;
        let out = dir.to_path_buf();
        tokio::task::spawn_blocking(move || {
            for layer in layers {
                let file = std::fs::File::open(&layer)
                    .with_context(|| format!("Unable to open '{}'", layer.display()))?;
                let mut archive = tar::Archive::new(file);
                archive.set_preserve_mtime(true);
                archive
                    .unpack(&out)
                    .with_context(|| format!("Unable to unpack '{}'", layer.display()))?;
            }
            // The published metadata is what the regenerated metadata is compared against.
            let repodata = out.join("repodata");
            if repodata.exists() {
                std::fs::remove_dir_all(&repodata)
                    .with_context(|| format!("Unable to remove '{}'", repodata.display()))?;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("Unable to join the unpack task")??;

        // Run as the owner of `dir`, so that the metadata can be removed when done.
        let owner = std::fs::metadata(dir)?;
        let user = format!("{}:{}", owner.uid(), owner.gid());
        let volume = format!("{}:/kit:z", dir.display());
        docker(&[
            "run",
            "--rm",
            "--network",
            "none",
            "--platform",
            &self.platform,
            "--user",
            &user,
            "-v",
            &volume,
            &self.name,
            "createrepo_c",
            "--revision",
            commit_time,
            "--set-timestamp-to-revision",
            "/kit",
        ])
        .await?;
        Ok(())
    }
}

impl Drop for Sdk {
    fn drop(&mut self) {
        let args = match &self.previous {
            Some(id) => vec!["tag", id, &self.name],
            None => vec!["rmi", &self.name],
        };
        let restored = std::process::Command::new("docker")
            .args(&args)
            .stdout(Stdio::null())
            .status();
        if !restored.is_ok_and(|status| status.success()) {
            warn!("Unable to restore the SDK image '{}'", self.name);
        }
    }
}

async fn docker(args: &[&str]) -> Result<String> {
    let output = Command::new("docker")
        .args(args)
        .stderr(Stdio::null())
        .output()
        .await
        .context("Unable to run docker")?;
    ensure!(
        output.status.success(),
        "'docker {}' failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Select the architectures to verify, `requested` or every one the kit image index publishes,
/// and return the digest of each one's manifest.
///
/// Without `requested`, every manifest in the index is verified, so none may be for another
/// platform. Consumers use the first manifest for an architecture, so verifying another one would
/// not cover what they get: the index may list only one per architecture.
fn select_arches(
    index: &ManifestListView,
    requested: &[String],
) -> Result<BTreeMap<String, String>> {
    let mut published = BTreeMap::new();
    for manifest in &index.manifests {
        let docker = manifest
            .platform
            .as_ref()
            .map(|platform| platform.architecture.to_string());
        let Some((arch, _)) = docker
            .as_deref()
            .and_then(|docker| ARCHES.iter().find(|(_, d)| *d == docker))
        else {
            ensure!(
                !requested.is_empty(),
                "The index lists manifest {} for {}, which cannot be verified; pass --arch to \
                 verify a subset of its architectures",
                manifest.digest,
                docker.map_or("no platform".to_string(), |d| format!("architecture {d}"))
            );
            continue;
        };
        let duplicate = published
            .insert(arch.to_string(), manifest.digest.clone())
            .is_some();
        ensure!(
            !duplicate,
            "The index lists more than one manifest for {arch}"
        );
    }
    if requested.is_empty() {
        ensure!(!published.is_empty(), "The index lists no kit images");
        return Ok(published);
    }
    requested
        .iter()
        .map(|arch| {
            let digest = published.get(arch).with_context(|| {
                format!(
                    "The kit is not published for {arch}; it is for {:?}",
                    published.keys()
                )
            })?;
            Ok((arch.clone(), digest.clone()))
        })
        .collect()
}

/// Read the kit metadata from the label of an image config.
fn kit_metadata(config: &Value) -> Result<ImageMetadata> {
    let config: ConfigView = config
        .get("config")
        .and_then(|config| serde_json::from_value(config.clone()).ok())
        .context("The image config has no labels; it does not appear to be a kit")?;
    let encoded = EncodedKitMetadata::extract_encoded_kit_metadata(&config)?;
    let metadata = ImageMetadata::try_from(EncodedKitMetadata(encoded.trim().to_string()))?;
    validate_kit_name(&metadata.name).context("The kit metadata does not name a valid kit")?;
    Ok(metadata)
}

/// Fail unless `name` is a valid kit name. It is used in paths and make arguments.
fn validate_kit_name(name: &str) -> Result<()> {
    ensure!(
        name.starts_with(|c: char| c.is_ascii_alphanumeric())
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
        "'{name}' is not a valid kit name"
    );
    Ok(())
}

/// The path of a blob in an OCI image layout.
fn blob(dir: &Path, digest: &str) -> Result<PathBuf> {
    let hex = digest.strip_prefix("sha256:").unwrap_or_default();
    ensure!(
        hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid digest '{digest}'"
    );
    Ok(dir.join("blobs/sha256").join(hex))
}

/// Find the kit archive buildsys names `<kit>-v<version>-<build id>-<arch>.tar`, where the build
/// ID is the abbreviated commit.
fn find_archive(
    source_dir: &Path,
    kit: &str,
    version: &str,
    commit: &str,
    arch: &str,
) -> Result<PathBuf> {
    let dir = source_dir.join("build/kits").join(kit);
    let prefix = format!("{kit}-v{version}-{}", &commit[..8]);
    let suffix = format!("-{arch}.tar");
    std::fs::read_dir(&dir)
        .with_context(|| format!("Unable to read '{}'", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(&suffix))
        })
        .with_context(|| {
            format!(
                "The build did not produce '{}/{prefix}*{suffix}'",
                dir.display()
            )
        })
}

/// Unpack the OCI layout in a kit archive into `dir`.
async fn unpack(archive: &Path, dir: &Path) -> Result<()> {
    if dir.exists() {
        fs::remove_dir_all(dir).await?;
    }
    fs::create_dir_all(dir).await?;
    let (archive, dir) = (archive.to_path_buf(), dir.to_path_buf());
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&archive)
            .with_context(|| format!("Unable to open '{}'", archive.display()))?;
        tar::Archive::new(file)
            .unpack(&dir)
            .with_context(|| format!("Unable to unpack '{}'", archive.display()))
    })
    .await
    .context("Unable to join the unpack task")?
}

async fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_slice(&fs::read(path).await?)
        .with_context(|| format!("Unable to parse '{}'", path.display()))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn checks_kit_names() {
        let config = |name: &str| {
            let metadata = format!(
                r#"{{"name":"{name}","version":"1.0.0","sdk":{{"name":"sdk","version":"1.0.0","vendor":"v"}},"kit":[]}}"#
            );
            let label = base64::engine::general_purpose::STANDARD.encode(metadata);
            serde_json::json!({ "config": { "Labels": { "dev.bottlerocket.kit.v4": label } } })
        };
        for name in ["../kit", "kit;rm", "-kit", ""] {
            assert!(kit_metadata(&config(name)).is_err(), "{name}");
            assert!(validate_kit_name(name).is_err(), "{name}");
        }
        assert_eq!(kit_metadata(&config("kit")).unwrap().name, "kit");
        assert!(validate_kit_name("bottlerocket-core-kit").is_ok());
        assert!(blob(Path::new("/l"), "sha256:../../x").is_err());
    }

    fn index(platforms: &[Option<&str>]) -> ManifestListView {
        let manifests: Vec<Value> = platforms
            .iter()
            .enumerate()
            .map(|(i, arch)| {
                let mut manifest = serde_json::json!({ "digest": format!("sha256:{i}") });
                if let Some(arch) = arch {
                    manifest["platform"] =
                        serde_json::json!({ "architecture": arch, "os": "linux" });
                }
                manifest
            })
            .collect();
        serde_json::from_value(serde_json::json!({ "manifests": manifests })).unwrap()
    }

    #[test]
    fn selects_arches() {
        let arches = |platforms: &[Option<&str>], requested: &[&str]| {
            let requested: Vec<String> = requested.iter().map(|a| a.to_string()).collect();
            select_arches(&index(platforms), &requested)
                .map(|arches| arches.into_iter().collect::<Vec<_>>())
                .map_err(|e| e.to_string())
        };
        let both = [Some("amd64"), Some("arm64")];
        let digests = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(a, d)| (a.to_string(), d.to_string()))
                .collect()
        };
        assert_eq!(
            arches(&both, &[]).unwrap(),
            digests(&[("aarch64", "sha256:1"), ("x86_64", "sha256:0")])
        );
        assert_eq!(
            arches(&both, &["aarch64"]).unwrap(),
            digests(&[("aarch64", "sha256:1")])
        );
        // A manifest without a known platform fails unless --arch selects a subset. The index
        // does not parse with architectures other than amd64 and arm64.
        let unknown = [Some("amd64"), None];
        assert!(arches(&unknown, &[]).unwrap_err().contains("no platform"));
        assert_eq!(
            arches(&unknown, &["x86_64"]).unwrap(),
            digests(&[("x86_64", "sha256:0")])
        );
        let duplicate = [Some("amd64"), Some("amd64")];
        assert!(arches(&duplicate, &["x86_64"])
            .unwrap_err()
            .contains("more than one"));
        assert!(arches(&[Some("amd64")], &["aarch64"])
            .unwrap_err()
            .contains("not published for aarch64"));
        assert!(arches(&[], &[]).is_err());
    }

    #[test]
    fn finds_archives() {
        let dir = tempfile::TempDir::new().unwrap();
        let kits = dir.path().join("build/kits/kit");
        std::fs::create_dir_all(&kits).unwrap();
        let commit = "0123456789abcdef0123456789abcdef01234567";
        for name in [
            "kit-v1.0.0-01234567-x86_64.tar",
            "kit-v1.0.0-01234567-aarch64.tar",
            "kit-v1.0.0-76543210-x86_64.tar",
        ] {
            std::fs::write(kits.join(name), b"").unwrap();
        }
        let found = find_archive(dir.path(), "kit", "1.0.0", commit, "x86_64").unwrap();
        assert_eq!(found, kits.join("kit-v1.0.0-01234567-x86_64.tar"));
        assert!(find_archive(dir.path(), "kit", "1.0.1", commit, "x86_64").is_err());
    }
}
