use crate::Args;
use buildsys_config::DockerArchitecture;
use clap::Parser;
use log::{debug, info, trace};
use pubsys_config::InfraConfig;
use regex::Regex;
use snafu::{ensure, OptionExt, ResultExt};
use std::path::PathBuf;
use tokio::process::Command;

/// Takes a local kit built using buildsys and publishes it to a vendor specified in Infra.toml
#[derive(Debug, Parser)]
pub(crate) struct PublishKitArgs {
    /// Location of the local kit
    #[arg(long)]
    kit_path: PathBuf,

    /// Vendor to publish kit to
    #[arg(long)]
    vendor: String,

    /// The version and build ID of the kit that should be published, e.g. v1.0.0-abcd123
    #[arg(long)]
    version: String,
}

macro_rules! docker {
    ($arg: expr) => {{
        let result = Command::new("docker")
            .args($arg)
            .output()
            .await
            .context(error::CommandSnafu { command: "docker" })?;
        ensure!(
            result.status.success(),
            error::PublishFailSnafu {
                reason: String::from_utf8_lossy(&result.stderr)
            }
        );
        result.stdout
    }};
}

pub(crate) async fn run(args: &Args, publish_kit_args: &PublishKitArgs) -> Result<()> {
    // If a lock file exists, use that, otherwise use Infra.toml
    let infra_config = InfraConfig::from_path_or_lock(&args.infra_config_path, false)
        .context(error::ConfigSnafu)?;
    trace!("Parsed infra config: {:?}", infra_config);

    // Fetch the vendor container registry uri
    let vendor = infra_config
        .vendor
        .as_ref()
        .context(error::NoVendorsSnafu)?
        .get(&publish_kit_args.vendor)
        .context(error::VendorNotFoundSnafu {
            name: publish_kit_args.vendor.clone(),
        })?;
    let vendor_registry_uri = vendor.registry.clone();
    debug!(
        "Found vendor container registry at uri: {}",
        vendor_registry_uri
    );

    // Auto resolve the expected paths for the kit contents archive
    let kit_path = publish_kit_args.kit_path.as_path();
    let kit_name = kit_path
        .file_name()
        .context(error::InvalidPathSnafu { path: &kit_path })?
        .to_string_lossy();
    let kit_version = publish_kit_args.version.clone();

    let mut platform_images = Vec::new();
    for arch in ["aarch64", "x86_64"] {
        let docker_arch =
            DockerArchitecture::try_from(arch).context(error::InvalidArchitectureSnafu)?;

        let kit_filename = format!("{}-{}-{}.tar", &kit_name, &kit_version, arch);
        let path = kit_path.join(&kit_filename);

        if !path.exists() {
            debug!("Kit image does not exist for arch {}", arch);
            continue;
        }

        let out = docker!(["load", format!("--input={}", path.display()).as_str(),]);
        let out = String::from_utf8_lossy(&out);
        let digest_expression =
            Regex::new("(?<digest>sha256:[0-9a-f]{64})").context(error::RegexSnafu)?;
        let caps = digest_expression
            .captures(&out)
            .context(error::NoDigestSnafu)?;
        let digest = &caps["digest"];

        let arch_specific_target_uri = format!(
            "{}/{}:{}-{}",
            vendor_registry_uri, kit_name, &kit_version, arch
        );

        docker!(["tag", digest, &arch_specific_target_uri,]);

        info!(
            "Pushing kit image for platform {} to {}",
            arch, &arch_specific_target_uri
        );
        docker!(["push", &arch_specific_target_uri,]);

        platform_images.push((docker_arch, arch_specific_target_uri.clone()));
    }

    let target_uri = format!("{}/{}:{}", vendor_registry_uri, kit_name, kit_version);

    let images: Vec<&str> = platform_images
        .iter()
        .map(|(_, image)| image.as_str())
        .collect();

    let mut manifest_create_args = vec!["manifest", "create", &target_uri];
    manifest_create_args.extend_from_slice(&images);
    docker!(manifest_create_args);

    for (arch, image) in platform_images.iter() {
        docker!([
            "manifest",
            "annotate",
            format!("--arch={}", arch).as_str(),
            &target_uri,
            image,
        ]);
    }

    info!("Pushing kit to {}", &target_uri);
    docker!(["manifest", "push", &target_uri,]);

    info!("Successfully published kit to {}", target_uri);

    // Cleans up the manifest list. This doesn't serve a purpose outside of publishing the kit and
    // takes up space.
    info!("Cleaning up local manifest");
    docker!(["manifest", "rm", &target_uri]);

    Ok(())
}

mod error {
    use snafu::Snafu;
    use std::path::PathBuf;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub(super)))]
    pub(crate) enum Error {
        #[snafu(display("IO error running command `{}`: {}", command, source))]
        Command {
            source: std::io::Error,
            command: String,
        },
        #[snafu(display("Error reading config: {}", source))]
        Config { source: pubsys_config::Error },
        #[snafu(display("Invalid architecture: {}", source))]
        InvalidArchitecture { source: anyhow::Error },
        #[snafu(display("Failed not get kit name from path {}", path.display()))]
        InvalidPath { path: PathBuf },
        #[snafu(display("No kit archive(s) exist at path {}", path.display()))]
        NoArchive { path: PathBuf },
        #[snafu(display("No digest returned by `docker load`"))]
        NoDigest,
        #[snafu(display("No vendors specified in Infra.toml, you must specify at least one"))]
        NoVendors,
        #[snafu(display("No kit version found, must be included in kit file name"))]
        NoVersion,
        #[snafu(display("Docker failed to load and push kit image: {}", reason))]
        PublishFail { reason: String },
        #[snafu(display("IO error reading directory {}: {}", dir.display(), source))]
        ReadDir {
            source: std::io::Error,
            dir: PathBuf,
        },
        #[snafu(display("Failed to parse kit filename: {}", source))]
        Regex { source: regex::Error },
        #[snafu(display("Vendor '{}' not specified in Infra.toml", name))]
        VendorNotFound { name: String },
    }
}

pub(crate) use error::Error;

type Result<T> = std::result::Result<T, Error>;
