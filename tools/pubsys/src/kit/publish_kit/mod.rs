use crate::Args;
use clap::Parser;
use log::{debug, info, trace};
use oci_cli_wrapper::{DockerArchitecture, ImageTool};
use pubsys_config::InfraConfig;
use snafu::{ensure, OptionExt, ResultExt};
use std::path::PathBuf;

/// Takes a local kit built using buildsys and publishes it to a vendor specified in Infra.toml
#[derive(Debug, Parser)]
pub(crate) struct PublishKitArgs {
    /// Location of the local kit
    #[arg(long)]
    kit_path: PathBuf,

    /// Vendor to publish kit to
    #[arg(long)]
    vendor: String,

    /// Optionally push the kit a different repository name
    #[arg(long)]
    repo: Option<String>,

    /// The version of the kit that should be published
    #[arg(long)]
    version: String,

    /// The build id of the kit that should be published
    #[arg(long)]
    build_id: String,
}

pub(crate) async fn run(args: &Args, publish_kit_args: &PublishKitArgs) -> Result<()> {
    let image_tool = ImageTool::from_builtin_krane();

    // If a lock file exists, use that, otherwise use Infra.toml
    let infra_config = InfraConfig::from_path_or_lock(&args.infra_config_path, false)
        .context(error::ConfigSnafu)?;
    trace!("Parsed infra config: {:?}", infra_config);

    publish_kit(infra_config, publish_kit_args, &image_tool).await
}

async fn publish_kit(
    infra_config: InfraConfig,
    publish_kit_args: &PublishKitArgs,
    image_tool: &ImageTool,
) -> Result<()> {
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
    let build_id = publish_kit_args.build_id.clone();

    let repository_target = match publish_kit_args.repo.as_ref() {
        Some(repo) => repo.clone(),
        None => kit_name.to_string(),
    };

    let mut platform_images = Vec::new();
    for arch in ["aarch64", "x86_64"] {
        let docker_arch =
            DockerArchitecture::try_from(arch).context(error::InvalidArchitectureSnafu { arch })?;

        let kit_filename = format!("{}-{}-{}-{}.tar", &kit_name, &kit_version, &build_id, arch);
        let path = kit_path.join(&kit_filename);

        if !path.exists() {
            debug!("Kit image does not exist for arch {}", arch);
            continue;
        }

        let arch_specific_target_uri = format!(
            "{}/{}:{}-{}-{}",
            vendor_registry_uri, repository_target, &kit_version, &build_id, arch
        );

        info!(
            "Pushing kit image for platform {} to {}",
            arch, &arch_specific_target_uri
        );

        image_tool
            .push_oci_archive(&path, &arch_specific_target_uri)
            .await
            .context(error::PublishKitSnafu)?;

        platform_images.push((docker_arch, arch_specific_target_uri.clone()));
    }
    ensure!(
        !platform_images.is_empty(),
        error::NoArchiveSnafu { path: kit_path }
    );

    let target_uri = format!(
        "{}/{}:{}",
        vendor_registry_uri, repository_target, kit_version
    );

    info!("Pushing kit to {}", &target_uri);

    image_tool
        .push_multi_platform_manifest(platform_images, &target_uri)
        .await
        .context(error::PublishKitSnafu)?;

    info!("Successfully published kit to {}", target_uri);

    Ok(())
}

mod error {
    use snafu::Snafu;
    use std::path::PathBuf;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub(super)))]
    pub(crate) enum Error {
        #[snafu(display("Error reading config: {}", source))]
        Config { source: pubsys_config::Error },

        #[snafu(display("Could not convert {} to docker architecture: {}", arch, source))]
        InvalidArchitecture {
            source: oci_cli_wrapper::error::Error,
            arch: String,
        },

        #[snafu(display("Failed not get kit name from path {}", path.display()))]
        InvalidPath { path: PathBuf },

        #[snafu(display("No kit archive(s) exist at path {}", path.display()))]
        NoArchive { path: PathBuf },

        #[snafu(display("No vendors specified in Infra.toml, you must specify at least one"))]
        NoVendors,

        #[snafu(display("Could not publish kit: {}", source))]
        PublishKit {
            source: oci_cli_wrapper::error::Error,
        },

        #[snafu(display("Vendor '{}' not specified in Infra.toml", name))]
        VendorNotFound { name: String },
    }
}

pub(crate) use error::Error;

type Result<T> = std::result::Result<T, Error>;
