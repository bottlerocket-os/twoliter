use crate::common::fs;
use crate::tools::install_tools;
use anyhow::{bail, ensure, Context, Result};
use clap::Parser;
use path_absolutize::Absolutize;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Instant;
use tempfile::TempDir;
use tokio::process::Command;
use tracing::{info, warn};

mod kit;

const DEFAULT_REPO: &str = "https://github.com/bottlerocket-os/bottlerocket.git";

/// The first twoliter release whose builds are expected to be reproducible.
const MIN_REPRODUCIBLE_TWOLITER: Version = Version::new(0, 26, 0);

/// The first twoliter release whose ext4 partitions do not depend on the build host: the release
/// carrying imghelper's `ext4_set_signed_dir_hash` and rpm2img's `no_copy_xattrs`. This assumes
/// the next minor release carries that change.
const HOST_INDEPENDENT_EXT4_TWOLITER: Version = Version::new(0, 28, 0);

/// Present when SELinux is enabled, in enforcing or permissive mode; both label new files.
const SELINUX_ENFORCE: &str = "/sys/fs/selinux/enforce";

/// erofs-utils commands reprosys uses to compare ROOT-A.
const EROFS_TOOLS: &[&str] = &["fsck.erofs", "dump.erofs"];

/// Exit code of `reprosys compare` when the images were compared but did not verify.
const REPROSYS_NOT_VERIFIED: i32 = 2;

/// Lines of the build log shown when the build fails.
const BUILD_LOG_TAIL: usize = 40;

/// The TUF repository the rebuild is built for. The build embeds its root role,
/// `roles/<repo>.root.json`, so the checkout's own `default` role is left alone.
const VERIFY_PUBLISH_REPO: &str = "reprosys-verify";

#[derive(Debug, Parser)]
pub(crate) enum VerifyCommand {
    Ami(VerifyAmi),
    Kit(kit::VerifyKit),
}

impl VerifyCommand {
    pub(crate) async fn run(self) -> Result<()> {
        match self {
            VerifyCommand::Ami(command) => command.run().await,
            VerifyCommand::Kit(command) => command.run().await,
        }
    }
}

/// Rebuild the image behind an AMI from source and compare it against the AMI's snapshots.
///
/// Only variants with an erofs root filesystem are supported.
///
/// Needs Docker, git, cargo-make, lz4, erofs-utils, the TUF root role the AMI was built with
/// (see --root-role), and AWS credentials allowing ec2:DescribeImages,
/// ec2:DescribeSnapshots, ec2:CopySnapshot, ec2:DeleteSnapshot, ebs:ListSnapshotBlocks and
/// ebs:GetSnapshotBlock.
#[derive(Debug, Parser)]
pub(crate) struct VerifyAmi {
    /// The AMI to verify.
    #[clap(long = "ami-id")]
    pub(crate) ami_id: String,

    /// The AWS region of the AMI.
    #[clap(long)]
    pub(crate) region: Option<String>,

    /// The AWS profile to use.
    #[clap(long)]
    pub(crate) profile: Option<String>,

    /// An existing clean clone of the source repository to build from. The commit the AMI was
    /// built from is checked out in it. Conflicts with --repo.
    #[clap(long = "source-dir", conflicts_with = "repo")]
    pub(crate) source_dir: Option<PathBuf>,

    /// The git repository to clone the source from.
    #[clap(long, default_value = DEFAULT_REPO)]
    pub(crate) repo: String,

    /// The TUF root role the AMI was built with. Defaults to the highest-numbered
    /// `<N>.root.json` at the top of the source checkout, which is the role official Bottlerocket
    /// images ship.
    #[clap(long = "root-role")]
    pub(crate) root_role: Option<PathBuf>,

    /// Directory for the source checkout, downloaded images, report and log. Defaults to
    /// `./twoliter-verify-<ami-id>`.
    #[clap(long = "work-dir")]
    pub(crate) work_dir: Option<PathBuf>,

    /// Keep the source checkout and images in the work directory when done. The report and the
    /// log are always kept.
    #[clap(long)]
    pub(crate) keep: bool,

    /// Show the build output instead of writing it to `build.log` in the work directory, and list
    /// every differing file, full hashes and an explanation of each check in the summary.
    #[clap(long)]
    pub(crate) verbose: bool,
}

/// The build identity of an AMI, as printed by `reprosys resolve`.
#[derive(Debug, Deserialize)]
struct AmiInfo {
    ami_name: String,
    variant: String,
    arch: String,
    version: String,
    build_id: String,
    filename_prefix: String,
    owner: String,
    owned_by_caller: bool,
}

/// The output of `reprosys download`.
#[derive(Debug, Deserialize)]
struct Downloaded {
    root_image: PathBuf,
    data_image: Option<PathBuf>,
}

/// Images rebuilt from source.
struct Built {
    os_image: PathBuf,
    data_image: Option<PathBuf>,
}

impl VerifyAmi {
    pub(super) async fn run(&self) -> Result<()> {
        let mut cleanup = Vec::new();
        let result = self.verify(&mut cleanup).await;
        clean_up(self.keep, &cleanup).await;
        result
    }

    /// Verify the AMI, adding the directories to delete when done to `cleanup`.
    async fn verify(&self, cleanup: &mut Vec<PathBuf>) -> Result<()> {
        let (work_dir, tools) = prepare(
            &["git", "cargo-make", "lz4", "docker"],
            self.work_dir.as_deref(),
            format!("twoliter-verify-{}", self.ami_id),
        )
        .await?;
        let reprosys = tools.path().join("reprosys");

        let (_, resolved) = run_reprosys(self.reprosys(&reprosys, "resolve"), &[])
            .await
            .context("Unable to determine the build that produced the AMI")?;
        let info: AmiInfo = serde_json::from_slice(&resolved)
            .context("Unable to parse 'reprosys resolve' output")?;
        let ami_info_path = work_dir.join("ami.json");
        fs::write(&ami_info_path, &resolved).await?;
        info!(
            "AMI {} ('{}') is {} {} version {} built from commit {}",
            self.ami_id, info.ami_name, info.variant, info.arch, info.version, info.build_id
        );
        if !info.owned_by_caller {
            info!(
                "AMI is owned by account {}; its snapshots will be copied into this account to download them",
                info.owner
            );
        }

        let (source_dir, cloned) = match &self.source_dir {
            Some(dir) => (dir.absolutize()?.into_owned(), false),
            None => (work_dir.join("source"), true),
        };
        checkout(
            &source_dir,
            cloned.then_some(self.repo.as_str()),
            Some(&info.build_id),
            &info.version,
            "AMI",
        )
        .await?;
        // Only once checkout has succeeded.
        if cloned {
            cleanup.push(source_dir.clone());
        }
        if let Some(pinned) =
            check_twoliter_version(&source_dir, &MIN_REPRODUCIBLE_TWOLITER).await?
        {
            let arch = std::env::consts::ARCH;
            let causes = private_host_differences(&pinned, selinux_enabled(), arch);
            if !causes.is_empty() {
                let data = if arch == "aarch64" {
                    " An ext4 DATA partition, if the variant has one, will fail verification. To \
                     verify such a variant, use an x86_64 host."
                } else {
                    ""
                };
                warn!(
                    "\n\n!!! The source pins twoliter {pinned}, and twoliter releases before \
                     {HOST_INDEPENDENT_EXT4_TWOLITER} build ext4 partitions that depend on the \
                     build host. Here, {}. PRIVATE will be reported as an expected \
                     difference.{data} !!!\n",
                    causes.join(", and ")
                );
            }
        }
        // Fail now rather than after the build if ROOT-A cannot be compared.
        ensure!(
            uses_erofs_root(&source_dir, &info.variant).await? != Some(false),
            "Variant {} has an ext4 root filesystem, which is not supported; only variants with \
             the 'erofs-root-partition' image feature can be verified",
            info.variant
        );
        for tool in EROFS_TOOLS {
            which::which(tool).with_context(|| {
                format!(
                    "The root filesystem is compared using erofs-utils, but '{tool}' was not \
                     found in PATH. Install erofs-utils and run again."
                )
            })?;
        }
        let root_role = match &self.root_role {
            Some(role) => role.clone(),
            None => committed_root_role(&source_dir)?,
        };
        install_root_role(&source_dir, &root_role).await?;

        let images_dir = work_dir.join("images");
        cleanup.push(images_dir.clone());
        let download = async {
            let mut download = self.reprosys(&reprosys, "download");
            download.arg("--out-dir").arg(&images_dir);
            let (_, out) = run_reprosys(download, &[])
                .await
                .context("Unable to download the AMI's snapshots")?;
            serde_json::from_slice::<Downloaded>(&out)
                .context("Unable to parse 'reprosys download' output")
        };
        // Let both finish even if one fails, so snapshot copies get deleted.
        let download = async {
            let result = download.await;
            if let Err(e) = &result {
                warn!("{e:#}; waiting for the build to finish (press Ctrl-C to stop it)");
            }
            result
        };
        let build_log = (!self.verbose).then(|| work_dir.join("build.log"));
        let build = async {
            let result = build(&source_dir, &images_dir, build_log.as_deref(), &info).await;
            if let Err(e) = &result {
                warn!("{e:#}; waiting for the snapshot download to finish");
            }
            result
        };
        let (downloaded, built) = tokio::join!(download, build);
        let (downloaded, built) = (downloaded?, built?);

        let report_path = work_dir.join("report.json");
        let log_path = work_dir.join("log.jsonl");
        let mut compare = self.reprosys(&reprosys, "compare");
        compare
            .arg("--expected")
            .arg(&built.os_image)
            .arg("--actual")
            .arg(&downloaded.root_image)
            .arg("--report")
            .arg(&report_path)
            .arg("--log")
            .arg(&log_path)
            .arg("--ami-info")
            .arg(&ami_info_path)
            .arg("--summary")
            .args(["--twoliter-version", env!("CARGO_PKG_VERSION")])
            .arg("--source-dir")
            .arg(&source_dir);
        if self.verbose {
            compare.arg("--verbose");
        }
        // reprosys fails verification when only one side has a data image.
        if let Some(expected) = &built.data_image {
            compare.arg("--expected-data").arg(expected);
        }
        if let Some(actual) = &downloaded.data_image {
            compare.arg("--actual-data").arg(actual);
        }
        let mut files = vec![("Report:", report_path.as_path()), ("Log:", &log_path)];
        files.extend(build_log.as_deref().map(|log| ("Build log:", log)));
        let what = format!("AMI {}", self.ami_id);
        compare_and_report(compare, &files, &what, &info.build_id).await
    }

    /// A `reprosys` `subcommand`, with this AMI and the AWS options unless it compares images.
    fn reprosys(&self, tool: &Path, subcommand: &str) -> Command {
        let mut cmd = Command::new(tool);
        cmd.arg(subcommand);
        if subcommand != "compare" {
            cmd.arg("--ami-id").arg(&self.ami_id);
            if let Some(region) = &self.region {
                cmd.arg("--region").arg(region);
            }
            if let Some(profile) = &self.profile {
                cmd.arg("--profile").arg(profile);
            }
        }
        cmd
    }
}

/// Check that `tools` are in PATH, create the work directory, `default` unless one is given, and
/// install twoliter's tools, returning the work directory and the tools directory.
async fn prepare(
    tools: &[&str],
    work_dir: Option<&Path>,
    default: String,
) -> Result<(PathBuf, TempDir)> {
    for tool in tools {
        which::which(tool)
            .with_context(|| format!("'{tool}' is required but not found in PATH"))?;
    }
    let work_dir = work_dir.map_or_else(|| PathBuf::from(default), Path::to_path_buf);
    let work_dir = work_dir.absolutize()?.into_owned();
    fs::create_dir_all(&work_dir).await?;
    let tools = TempDir::new().context("Unable to create a directory for tools")?;
    install_tools(tools.path()).await?;
    Ok((work_dir, tools))
}

/// Delete `dirs`, unless `keep`.
async fn clean_up(keep: bool, dirs: &[PathBuf]) {
    if !keep {
        for dir in dirs {
            if let Err(e) = fs::remove_dir_all(dir).await {
                warn!("Unable to clean up '{}': {e:#}", dir.display());
            }
        }
    }
}

/// Run a reprosys comparison, print its summary and `files`, and fail unless it verified that
/// `what` was built from `commit`.
async fn compare_and_report(
    compare: Command,
    files: &[(&str, &Path)],
    what: &str,
    commit: &str,
) -> Result<()> {
    let (code, summary) = run_reprosys(compare, &[REPROSYS_NOT_VERIFIED])
        .await
        .with_context(|| format!("Unable to compare the rebuild against {what}"))?;
    print!("{}", String::from_utf8_lossy(&summary));
    for (label, path) in files {
        println!("{label:<10} {}", path.display());
    }
    ensure!(
        code == 0,
        "{what} could not be verified as built from commit {commit}"
    );
    println!("{what} was verified as built from commit {commit}");
    Ok(())
}

/// Run reprosys and return its exit code and stdout; `ok_codes` are non-zero exit codes that are
/// not errors.
async fn run_reprosys(mut cmd: Command, ok_codes: &[i32]) -> Result<(i32, Vec<u8>)> {
    // `Command::output` would pipe stderr; spawn so reprosys progress stays visible.
    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("Unable to run reprosys")?
        .wait_with_output()
        .await
        .context("Unable to wait for reprosys")?;
    let code = output.status.code().unwrap_or(-1);
    ensure!(
        code == 0 || ok_codes.contains(&code),
        "reprosys {} failed with exit code {code}",
        cmd.as_std().get_args().next().unwrap_or_default().display()
    );
    Ok((code, output.stdout))
}

async fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdout(Stdio::piped())
        // `Command::output` would pipe stderr, hiding git's error messages.
        .stderr(Stdio::inherit())
        .spawn()
        .context("Unable to run git")?
        .wait_with_output()
        .await
        .context("Unable to wait for git")?;
    ensure!(
        output.status.success(),
        "'git {}' failed in '{}'",
        args.join(" "),
        dir.display()
    );
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The commit `rev` names in `dir`.
async fn rev_parse(dir: &Path, rev: &str) -> Result<String> {
    git(
        dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ],
    )
    .await
}

async fn read_toml(path: &Path) -> Result<toml::Value> {
    toml::from_str(&fs::read_to_string(path).await?)
        .with_context(|| format!("Unable to parse '{}'", path.display()))
}

/// Clone `repo` into `dir`, or fetch it if `dir` is a checkout from an earlier run.
async fn fetch_source(dir: &Path, repo: &str) -> Result<()> {
    if dir.exists() {
        info!("Reusing existing checkout in '{}'", dir.display());
        git(dir, &["fetch", "--tags", "origin"]).await?;
    } else {
        info!("Cloning '{repo}' into '{}'", dir.display());
        let parent = dir.parent().context("Source directory has no parent")?;
        let name = dir.file_name().context("Source directory has no name")?;
        git(parent, &["clone", repo, &name.to_string_lossy()]).await?;
    }
    Ok(())
}

/// Fail unless `dir` has no uncommitted changes and no untracked or ignored files: build output
/// left from an earlier build would be found as the rebuild's, and other files can change the
/// build.
async fn ensure_clean(dir: &Path) -> Result<()> {
    let status = git(dir, &["status", "--porcelain", "--ignored"]).await?;
    ensure!(
        status.is_empty(),
        "Source directory '{}' has uncommitted changes, or untracked or ignored files such as the \
         output of an earlier build, which could change the build or be taken for its output. \
         Use a fresh clone, or omit --source-dir to let twoliter clone the source",
        dir.display()
    );
    Ok(())
}

/// Check out `commit` in `dir`, or the commit tagged `v<version>` if none is given, and return it.
/// `what` is what was built from the commit, for messages.
///
/// If `repo` is given, `dir` is twoliter's own clone of it: it is cloned or fetched, and reset to
/// the commit with every untracked and ignored file removed. Otherwise `dir` must be clean.
async fn checkout(
    dir: &Path,
    repo: Option<&str>,
    commit: Option<&str>,
    version: &str,
    what: &str,
) -> Result<String> {
    match repo {
        Some(repo) => fetch_source(dir, repo).await?,
        None => ensure_clean(dir).await?,
    }

    let tag = format!("v{version}");
    let tagged = rev_parse(dir, &tag).await.ok();
    let commit = match commit {
        Some(commit) => {
            let resolved = rev_parse(dir, commit).await.with_context(|| {
                format!(
                    "Commit {commit} is not in '{}', or the abbreviation is ambiguous",
                    dir.display()
                )
            })?;
            match &tagged {
                Some(tagged) if *tagged == resolved => info!("Tag {tag} points at {resolved}"),
                Some(tagged) => warn!("Tag {tag} points at {tagged}, not at {resolved}"),
                None => warn!("Tag {tag} does not exist in the source repository"),
            }
            resolved
        }
        None => tagged.with_context(|| {
            format!(
                "Tag {tag} does not exist in '{}'; pass the commit the {what} was built from \
                 with --commit",
                dir.display()
            )
        })?,
    };
    if repo.is_some() {
        git(
            dir,
            &["checkout", "--quiet", "--force", "--detach", &commit],
        )
        .await?;
        git(dir, &["clean", "-ffdx", "--quiet"]).await?;
    } else {
        warn!("Checking out commit {commit} in '{}'", dir.display());
        git(dir, &["checkout", "--quiet", "--detach", &commit]).await?;
    }
    info!("Checked out commit {commit}");

    let release_version = read_release_version(dir).await?;
    ensure!(
        release_version == version,
        "Commit {commit} has release version {release_version}, but the {what} is version \
         {version}"
    );
    Ok(commit)
}

/// Read the release version from Twoliter.toml, which every tag from v1.30.0 has. Older
/// checkouts predate twoliter 0.26, the first release whose builds are reproducible.
async fn read_release_version(dir: &Path) -> Result<String> {
    let path = dir.join("Twoliter.toml");
    read_toml(&path)
        .await?
        .get("release-version")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .with_context(|| format!("No release-version found in '{}'", path.display()))
}

/// Read the twoliter version the checkout pins, from the `env` table of Makefile.toml as in
/// Bottlerocket, or a `TWOLITER_VERSION` variable in the Makefile as in kit repositories.
async fn pinned_twoliter_version(dir: &Path) -> Result<String> {
    let path = dir.join("Makefile.toml");
    if path.exists() {
        if let Some(version) = read_toml(&path)
            .await?
            .get("env")
            .and_then(|env| env.get("TWOLITER_VERSION"))
            .and_then(|v| v.as_str())
        {
            return Ok(version.to_string());
        }
    }
    let path = dir.join("Makefile");
    if path.exists() {
        for line in fs::read_to_string(&path).await?.lines() {
            let Some(rest) = line.trim().strip_prefix("TWOLITER_VERSION") else {
                continue;
            };
            let rest = rest.trim_start();
            if let Some(value) = ["?=", ":=", "="]
                .iter()
                .find_map(|op| rest.strip_prefix(op))
            {
                let value = value.split('#').next().unwrap_or_default();
                return Ok(value.trim().trim_matches('"').to_string());
            }
        }
    }
    bail!(
        "No TWOLITER_VERSION found in Makefile.toml or Makefile in '{}'",
        dir.display()
    )
}

/// Warn when the checkout pins a twoliter older than `minimum`, the first whose builds are
/// expected to reproduce, and return the pinned version if it parses.
async fn check_twoliter_version(dir: &Path, minimum: &Version) -> Result<Option<Version>> {
    let pinned = pinned_twoliter_version(dir).await?;
    let version = Version::parse(pinned.trim_start_matches('v')).ok();
    match &version {
        Some(version) if version >= minimum => {
            info!("Source pins twoliter {version}");
        }
        _ => warn!(
            "\n\n!!! The source pins twoliter {pinned}, which predates reproducible builds \
             (twoliter {minimum}). The rebuild is expected to differ from what was published. \
             !!!\n"
        ),
    }
    Ok(version)
}

/// Whether this host enforces SELinux.
fn selinux_enabled() -> bool {
    Path::new(SELINUX_ENFORCE).exists()
}

/// Why a rebuild on a host with `selinux_enabled` and `arch` makes ext4 partitions that differ
/// from those built on an x86_64 host with SELinux disabled, when the source pins twoliter
/// `pinned`. A pre-release of the first host-independent release counts as that release.
fn private_host_differences(
    pinned: &Version,
    selinux_enabled: bool,
    arch: &str,
) -> Vec<&'static str> {
    if Version::new(pinned.major, pinned.minor, pinned.patch) >= HOST_INDEPENDENT_EXT4_TWOLITER {
        return Vec::new();
    }
    let mut causes = Vec::new();
    if selinux_enabled {
        causes.push(
            "SELinux is enabled, so mke2fs copies the SELinux labels of the rebuilt files into \
             PRIVATE",
        );
    }
    if arch == "aarch64" {
        causes.push(
            "the host is aarch64, whose char is unsigned, so mke2fs flags the directory hashes of \
             PRIVATE and any ext4 DATA partition as unsigned",
        );
    }
    causes
}

/// The highest-numbered TUF root role, `<N>.root.json`, at the top of the checkout `dir`.
fn committed_root_role(dir: &Path) -> Result<PathBuf> {
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("Unable to list '{}'", dir.display()))?;
    let mut roles = Vec::new();
    for entry in entries {
        let path = entry
            .with_context(|| format!("Unable to list '{}'", dir.display()))?
            .path();
        let version = path
            .file_name()
            .and_then(|name| name.to_str()?.strip_suffix(".root.json"))
            .filter(|n| n.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|n| n.parse::<u64>().ok());
        if let Some(version) = version.filter(|_| path.is_file()) {
            roles.push((version, path));
        }
    }
    let highest = roles.into_iter().max().map(|(_, path)| path);
    highest.with_context(|| {
        format!(
            "No TUF root role named '<N>.root.json' at the top of '{}'; pass --root-role with \
             the root role the AMI was built with",
            dir.display()
        )
    })
}

/// Install `role` in the checkout `dir` as the root role the build embeds.
async fn install_root_role(dir: &Path, role: &Path) -> Result<()> {
    let contents = fs::read(role).await?;
    let roles = dir.join("roles");
    fs::create_dir_all(&roles).await?;
    fs::write(
        roles.join(format!("{VERIFY_PUBLISH_REPO}.root.json")),
        &contents,
    )
    .await?;
    info!(
        "The rebuild embeds TUF root role '{}' (sha256 {:x})",
        role.display(),
        Sha256::digest(&contents)
    );
    Ok(())
}

/// Whether the variant enables the `erofs-root-partition` image feature, or None if its manifest
/// is missing.
async fn uses_erofs_root(dir: &Path, variant: &str) -> Result<Option<bool>> {
    let path = dir.join("variants").join(variant).join("Cargo.toml");
    if !path.exists() {
        warn!(
            "'{}' does not exist; unable to tell whether the variant has an erofs root filesystem",
            path.display()
        );
        return Ok(None);
    }
    Ok(Some(
        read_toml(&path)
            .await?
            .get("package")
            .and_then(|p| p.get("metadata"))
            .and_then(|m| m.get("build-variant"))
            .and_then(|b| b.get("image-features"))
            .and_then(|f| f.get("erofs-root-partition"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    ))
}

/// Build the variant with the checkout's own entry point and the root role `install_root_role`
/// installed, and decompress the images. Output goes to `log` if given.
async fn build(
    source_dir: &Path,
    images_dir: &Path,
    log: Option<&Path>,
    info: &AmiInfo,
) -> Result<Built> {
    let mut cmd = Command::new("cargo");
    cmd.args(["make", "-e"])
        .arg(format!("BUILDSYS_VARIANT={}", info.variant))
        .arg("-e")
        .arg(format!("BUILDSYS_ARCH={}", info.arch))
        .arg("-e")
        .arg(format!("PUBLISH_REPO={VERIFY_PUBLISH_REPO}"))
        // No signing key exists for the role, and building an image needs none.
        .args(["-e", "ALLOW_MISSING_KEY=true"]);
    run_build(
        cmd,
        source_dir,
        log,
        &format!("{} {}", info.variant, info.arch),
    )
    .await?;

    let variant_dir = source_dir
        .join("build/images")
        .join(format!("{}-{}", info.arch, info.variant))
        .join(format!("{}-{}", info.version, info.build_id));
    let os_lz4 = variant_dir.join(format!("{}.img.lz4", info.filename_prefix));
    ensure!(
        os_lz4.is_file(),
        "The build did not produce '{}'; the build ID may differ from the AMI's",
        os_lz4.display()
    );
    fs::create_dir_all(images_dir).await?;
    let os_image = images_dir.join("rebuilt.img");
    lz4_decompress(&os_lz4, &os_image).await?;

    let data_lz4 = variant_dir.join(format!("{}-data.img.lz4", info.filename_prefix));
    let data_image = if data_lz4.is_file() {
        let path = images_dir.join("rebuilt-data.img");
        lz4_decompress(&data_lz4, &path).await?;
        Some(path)
    } else {
        None
    };
    Ok(Built {
        os_image,
        data_image,
    })
}

/// Run the build command `cmd` in `source_dir`, describing what it builds as `what`.
///
/// The build output is written to `log` if given, and shown otherwise.
async fn run_build(
    mut cmd: Command,
    source_dir: &Path,
    log: Option<&Path>,
    what: &str,
) -> Result<()> {
    cmd.current_dir(source_dir).kill_on_drop(true);
    match log {
        Some(log) => {
            info!(
                "Building {what}; this takes a while. Build output is written to '{}'",
                log.display()
            );
            let file = std::fs::File::create(log)
                .with_context(|| format!("Unable to create '{}'", log.display()))?;
            cmd.stdin(Stdio::null())
                .stdout(file.try_clone()?)
                .stderr(file);
        }
        None => info!("Building {what} in '{}'", source_dir.display()),
    }
    let started = Instant::now();
    let program = cmd.as_std().get_program().to_string_lossy().into_owned();
    let status = cmd
        .status()
        .await
        .with_context(|| format!("Unable to run {program}"))?;
    if !status.success() {
        if let Some(log) = log {
            print_log_tail(log).await;
        }
        bail!("Building {what} from source failed");
    }
    info!(
        "Built {what} in {}m {:02}s",
        started.elapsed().as_secs() / 60,
        started.elapsed().as_secs() % 60
    );
    Ok(())
}

/// Show the end of a failed build's log, where the error usually is.
async fn print_log_tail(log: &Path) {
    let Ok(contents) = fs::read_to_string(log).await else {
        return;
    };
    let lines: Vec<&str> = contents.lines().collect();
    let tail = &lines[lines.len().saturating_sub(BUILD_LOG_TAIL)..];
    eprintln!(
        "Last {} lines of '{}':\n{}",
        tail.len(),
        log.display(),
        tail.join("\n")
    );
}

async fn lz4_decompress(input: &Path, output: &Path) -> Result<()> {
    let status = Command::new("lz4")
        .args(["-d", "-f", "-q", "--sparse"])
        .arg(input)
        .arg(output)
        .status()
        .await
        .context("Unable to run lz4")?;
    ensure!(
        status.success(),
        "Unable to decompress '{}'",
        input.display()
    );
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn warns_of_hosts_that_change_private() {
        let old = Version::new(0, 27, 0);
        let rc = Version::parse("0.28.0-rc.1").unwrap();
        for (pinned, selinux_enabled, arch, expected) in [
            (&old, false, "x86_64", &[][..]),
            (&old, true, "x86_64", &["SELinux"]),
            (&old, false, "aarch64", &["aarch64"]),
            (&old, true, "aarch64", &["SELinux", "aarch64"]),
            (&rc, true, "aarch64", &[]),
            (&HOST_INDEPENDENT_EXT4_TWOLITER, true, "aarch64", &[]),
            (&Version::new(0, 29, 1), true, "aarch64", &[]),
        ] {
            let causes = private_host_differences(pinned, selinux_enabled, arch);
            let case = format!("{pinned} {selinux_enabled} {arch}: {causes:?}");
            assert_eq!(causes.len(), expected.len(), "{case}");
            for (cause, about) in causes.iter().zip(expected) {
                assert!(cause.contains(about), "{case}");
            }
        }
    }

    #[tokio::test]
    async fn detects_erofs_root() {
        let dir = TempDir::new().unwrap();
        let variant = dir.path().join("variants/aws-dev");
        std::fs::create_dir_all(&variant).unwrap();
        for (manifest, expected) in [
            (
                "[package.metadata.build-variant.image-features]\nerofs-root-partition = true\n",
                true,
            ),
            ("[package]\nname = \"aws-dev\"\n", false),
        ] {
            std::fs::write(variant.join("Cargo.toml"), manifest).unwrap();
            let found = uses_erofs_root(dir.path(), "aws-dev").await.unwrap();
            assert_eq!(found, Some(expected));
        }
    }

    #[test]
    fn finds_the_highest_numbered_root_role() {
        let dir = TempDir::new().unwrap();
        let error = committed_root_role(dir.path()).unwrap_err();
        assert!(error.to_string().contains("pass --root-role"), "{error}");
        for name in [
            "default.root.json",
            "+12.root.json",
            ".root.json",
            "9.root.json",
            "10.root.json",
            "11.root.json.bak",
        ] {
            std::fs::write(dir.path().join(name), name).unwrap();
        }
        std::fs::create_dir(dir.path().join("12.root.json")).unwrap();
        assert_eq!(
            committed_root_role(dir.path()).unwrap(),
            dir.path().join("10.root.json")
        );

        let unnumbered = TempDir::new().unwrap();
        std::fs::write(unnumbered.path().join("default.root.json"), "{}").unwrap();
        assert!(committed_root_role(unnumbered.path()).is_err());
    }

    #[tokio::test]
    async fn reads_pinned_twoliter_from_makefile() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("Makefile"),
            "TOP := here\nTWOLITER_VERSION ?= \"0.26.0\" # pinned\nTWOLITER_SHA256_AARCH64 ?= \"abc\"\n",
        )
        .unwrap();
        assert_eq!(pinned_twoliter_version(dir.path()).await.unwrap(), "0.26.0");
    }

    #[tokio::test]
    async fn run_reprosys_inherits_stderr_and_allows_listed_exit_codes() {
        let script = |code: i32| {
            let mut cmd = Command::new("sh");
            cmd.arg("-c").arg(format!(
                "test /proc/self/fd/2 -ef /proc/$PPID/fd/2 || exit 99; printf out; exit {code}"
            ));
            cmd
        };
        assert_eq!(
            run_reprosys(script(0), &[]).await.unwrap(),
            (0, b"out".to_vec())
        );
        assert_eq!(
            run_reprosys(script(REPROSYS_NOT_VERIFIED), &[REPROSYS_NOT_VERIFIED])
                .await
                .unwrap(),
            (REPROSYS_NOT_VERIFIED, b"out".to_vec())
        );
        let error = run_reprosys(script(REPROSYS_NOT_VERIFIED), &[])
            .await
            .unwrap_err();
        assert!(
            error.to_string().ends_with("failed with exit code 2"),
            "{error}"
        );
    }
}
