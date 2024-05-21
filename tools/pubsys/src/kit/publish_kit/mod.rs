use crate::Args;
use base64::Engine;
use clap::Parser;
use log::{info, trace};
use pubsys_config::InfraConfig;
use semver::Version;
use serde::{Deserialize, Serialize};
use snafu::{ensure, OptionExt, ResultExt};
use std::path::PathBuf;
use tokio::fs::{read, remove_file, write};
use tokio::process::Command;

/// Defines the label key for a kit in a registry
const KIT_METADATA_KEY: &str = "dev.bottlerocket.kit.v1";

/// Takes a local kit built using buildsys and publishes it
/// to a vendor specified in Infra.toml
#[derive(Debug, Parser)]
pub(crate) struct PublishKitArgs {
    /// Location of the local kit
    #[arg(long)]
    kit_path: PathBuf,
    /// Vendor to publish kit to
    #[arg(long)]
    vendor: String,
}

/// Defines a minimal view of the kit metadata that is
/// needed for publishing, the shape of this metadata is controlled by
/// buildsys
#[derive(Deserialize, Serialize, Debug)]
struct MetadataView {
    name: String,
    version: Version,
}

pub(crate) async fn run(args: &Args, publish_kit_args: &PublishKitArgs) -> Result<()> {
    // If a lock file exists, use that, otherwise use Infra.toml
    let infra_config = InfraConfig::from_path_or_lock(&args.infra_config_path, false)
        .context(error::ConfigSnafu)?;
    trace!("Parsed infra config: {:?}", infra_config);
    ensure!(infra_config.kit_vendors.is_some(), error::NoVendorsSnafu);

    // Fetch the vendor container registry uri
    let vendor_registry_uri = infra_config
        .kit_vendors
        .as_ref()
        .unwrap()
        .get(&publish_kit_args.vendor)
        .context(error::VendorNotFoundSnafu {
            name: publish_kit_args.vendor.clone(),
        })?;
    info!(
        "Found vendor container registry at uri: {}",
        vendor_registry_uri
    );

    // Auto resolve the expected paths for the metadata file and kit contents archive
    let kit_path = publish_kit_args.kit_path.as_path();
    let metadata_file = kit_path.join("kit-metadata.json");
    let metadata_file = metadata_file.as_path();
    ensure!(
        metadata_file.exists(),
        error::NoMetadataSnafu {
            path: metadata_file
        }
    );
    let mut platforms = Vec::new();
    for arch in ["amd64", "arm64"] {
        let path = kit_path.join(format!("kit-{}.tar", arch));
        if path.exists() {
            platforms.push(format!("linux/{}", arch));
        }
    }
    ensure!(
        !platforms.is_empty(),
        error::NoArchiveSnafu { path: kit_path }
    );

    // Fetch a view of the metadata
    let metadata_blob = read(metadata_file).await.context(error::IoSnafu)?;
    let metadata: MetadataView =
        serde_json::from_slice(metadata_blob.as_slice()).context(error::DeserializationSnafu)?;
    trace!("Parse kit metadata to view: {:?}", metadata);

    info!(
        "Found local kit with name {} and version {}",
        metadata.name, metadata.version
    );

    // We only really need the version and name from the metadata here to build our publish uri
    let target_uri = format!(
        "{}/{}:{}",
        vendor_registry_uri, metadata.name, metadata.version
    );

    // Use docker to build and push the kit base image
    info!(
        "Creating and pushing kit container via docker to {}",
        &target_uri
    );
    let metadata_encoded =
        base64::engine::general_purpose::STANDARD.encode(metadata_blob.as_slice());

    let docker_file = format!(
        r#"FROM scratch
ARG TARGETARCH

LABEL {}={}

ADD kit-$TARGETARCH.tar /
"#,
        KIT_METADATA_KEY, metadata_encoded
    );
    let docker_file_path = kit_path.join("Dockerfile");
    write(&docker_file_path, docker_file)
        .await
        .context(error::IoSnafu)?;
    let tag = format!("--tag={}", target_uri);
    let platform = format!("--platform={}", platforms.join(","));

    let result = Command::new("docker")
        .args([
            "build",
            "--builder=container",
            "--push",
            platform.as_str(),
            tag.as_str(),
            ".",
        ])
        .current_dir(kit_path)
        .output()
        .await
        .context(error::IoSnafu)?;
    ensure!(
        result.status.success(),
        error::PublishFailSnafu {
            reason: String::from_utf8_lossy(&result.stderr)
        }
    );
    trace!("Docker log:\n {}", String::from_utf8_lossy(&result.stderr));
    info!("Successfully published kit to {}", target_uri);

    // Clean up the Dockerfile
    remove_file(&docker_file_path)
        .await
        .context(error::IoSnafu)?;

    Ok(())
}

mod error {
    use snafu::Snafu;
    use std::path::PathBuf;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub(super)))]
    pub(crate) enum Error {
        #[snafu(display(
            "Unsupported architecture for a kit: {} (must be x86_64 or arm64)",
            arch
        ))]
        Architecture { arch: String },
        #[snafu(display("Error reading config: {}", source))]
        Config { source: pubsys_config::Error },
        #[snafu(display("Malformed kit metadata failed to deserialize: {}", source))]
        Deserialization { source: serde_json::Error },
        #[snafu(display("IO error occurred: {}", source))]
        Io { source: std::io::Error },
        #[snafu(display("Malformed kit metadata, expected a {}", cause))]
        MalformedMetadata { cause: String },
        #[snafu(display("No kit archive(s) exist at path {}", path.display()))]
        NoArchive { path: PathBuf },
        #[snafu(display("No metadata file provided for kit at {}", path.display()))]
        NoMetadata { path: PathBuf },
        #[snafu(display("No vendors specified in Infra.toml, you must specify at least one"))]
        NoVendors,
        #[snafu(display("Docker failed to build and push kit image: {}", reason))]
        PublishFail { reason: String },
        #[snafu(display("Vendor '{}' not specified in Infra.toml", name))]
        VendorNotFound { name: String },
    }
}

pub(crate) use error::Error;

type Result<T> = std::result::Result<T, Error>;
