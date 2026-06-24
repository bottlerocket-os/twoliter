//! The ami module owns the 'ami' subcommand and controls the process of registering and copying
//! EC2 AMIs.

pub(crate) mod launch_permissions;
pub(crate) mod public;
mod register;
mod snapshot;
pub(crate) mod wait;

use self::register::mk_amispec;
use crate::aws::ami::launch_permissions::get_launch_permissions;
use crate::aws::ami::public::ami_is_public;
use crate::aws::publish_ami::{get_snapshots, modify_image, modify_snapshots, ModifyOptions};
use crate::aws::{client::build_client_config_for_role, region_from_string};
use crate::frompath::FromPath;
use crate::Args;
use aws_sdk_ebs::Client as EbsClient;
use aws_sdk_ec2::error::ProvideErrorMetadata;
use aws_sdk_ec2::operation::copy_image::{CopyImageError, CopyImageOutput};
use aws_sdk_ec2::types::OperationType;
use aws_sdk_ec2::{config::Region, Client as Ec2Client};
use aws_sdk_sts::operation::get_caller_identity::{
    GetCallerIdentityError, GetCallerIdentityOutput,
};
use aws_sdk_sts::Client as StsClient;
use buildsys::manifest::ManifestInfo;
use clap::Parser;
use error_utils::AwsSdkError;
use futures::future::{join, lazy, ready, FutureExt};
use futures::stream::{self, StreamExt};
use futures::TryFutureExt;
use log::{error, info, trace, warn};
use pubsys_config::{AwsConfig as PubsysAwsConfig, InfraConfig};
use register::{get_ami_id, register_image, RegisteredIds};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use snafu::{ensure, OptionExt, ResultExt};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use wait::wait_for_ami;

const WARN_SEPARATOR: &str = "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!";

/// Builds Bottlerocket AMIs using latest build artifacts
#[derive(Debug, Parser)]
pub(crate) struct AmiArgs {
    /// Path to the image containing the os volume
    #[arg(short = 'o', long)]
    os_image: PathBuf,

    /// Path to the image containing the data volume
    #[arg(short = 'd', long)]
    data_image: Option<PathBuf>,

    /// Path to the variant manifest
    #[arg(short = 'v', long, value_parser = parse_variant_manifest)]
    variant_manifest: VariantManifest,

    /// Path to the UEFI data
    #[arg(short = 'e', long, value_parser = parse_uefi_data)]
    uefi_data: Option<FromPath<String>>,

    /// The architecture of the machine image
    #[arg(short = 'a', long)]
    arch: String,

    /// The desired AMI name
    #[arg(short = 'n', long)]
    name: String,

    /// The desired AMI description
    #[arg(long)]
    description: Option<String>,

    /// Don't display progress bars
    #[arg(long)]
    no_progress: bool,

    /// Regions where you want the AMI, the first will be used as the base for copying
    #[arg(long, value_delimiter = ',')]
    regions: Vec<String>,

    /// amispec file containing AMI registration options.
    #[arg(long, value_parser = parse_toml_file::<toml::Table>)]
    amispec_file: Option<FromPath<toml::Table>>,

    /// If specified, save created regional AMI IDs in JSON at this path.
    #[arg(long)]
    ami_output: Option<PathBuf>,
}

/// Common entrypoint from main()
pub(crate) async fn run(args: &Args, ami_args: &AmiArgs) -> Result<()> {
    match _run(args, ami_args).await {
        Ok(amis) => {
            // Write the AMI IDs to file if requested
            if let Some(ref path) = ami_args.ami_output {
                write_amis(path, &amis)
                    .await
                    .context(error::WriteAmisSnafu { path })?;
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}

async fn _run(args: &Args, ami_args: &AmiArgs) -> Result<RegionAccountImageMap> {
    // Maps each region to a map of account ID -> the AMI registered/copied into that account.
    let mut amis: RegionAccountImageMap = HashMap::new();

    // If a lock file exists, use that, otherwise use Infra.toml or default
    let infra_config = InfraConfig::from_path_or_lock(&args.infra_config_path, true)
        .context(error::ConfigSnafu)?;
    trace!("Using infra config: {infra_config:?}");

    let aws = infra_config.aws.unwrap_or_default();

    // If the user gave an override list of regions, use that, otherwise use what's in the config.
    let mut regions = if !ami_args.regions.is_empty() {
        ami_args.regions.clone()
    } else {
        aws.regions.clone().into()
    }
    .into_iter()
    .map(|name| region_from_string(&name))
    .collect::<Vec<Region>>();

    ensure!(
        !regions.is_empty(),
        error::MissingConfigSnafu {
            missing: "aws.regions"
        }
    );

    // We register in this base region first, then copy from there to any other regions/accounts.
    let base_region = regions.remove(0);

    // A region can target multiple accounts, one per configured role. We register the AMI once,
    // using the first role of the base region (the "base account"), then copy it to every other
    // (region, account) target.
    let base_role = roles_for_region(&aws, &base_region)
        .into_iter()
        .next()
        .flatten();

    // Build EBS client for snapshot management, and EC2 client for registration
    let client_config =
        build_client_config_for_role(&base_region, &base_region, &aws, base_role.as_deref()).await;

    let base_ebs_client = EbsClient::new(&client_config);

    let base_ec2_client = Ec2Client::new(&client_config);

    // Render an amispec without any defined snapshots.
    //
    // This is used to determine the desired name and architecture, so that we can see if our AMI
    // already exists.
    let tentative_amispec =
        mk_amispec::create_minimal_amispec(ami_args).context(error::AmispecSnafu)?;

    let arch = tentative_amispec
        .architecture
        .as_ref()
        .map(ToString::to_string)
        .context(error::MissingArchitectureSnafu)?;

    // Check if the AMI already exists, in which case we can use the existing ID, otherwise we
    // register a new one.
    let maybe_id = get_ami_id(
        &tentative_amispec.name,
        &arch,
        &base_region,
        &base_ec2_client,
    )
    .await
    .context(error::GetAmiIdSnafu {
        arch: &arch,
        name: &tentative_amispec.name,
        region: base_region.as_ref(),
    })?;

    // If the AMI does not exist yet, `public` should be false and `launch_permissions` empty
    let mut public = false;
    let mut launch_permissions = vec![];

    let (ids_of_image, already_registered) = if let Some(found_id) = maybe_id {
        warn!(
            "\n{}\n\nFound '{}' already registered in {}: {}\n\n{0}",
            WARN_SEPARATOR, tentative_amispec.name, base_region, found_id
        );
        let snapshot_ids = get_snapshots(&found_id, &base_region, &base_ec2_client)
            .await
            .context(error::GetSnapshotsSnafu {
                image_id: &found_id,
                region: base_region.as_ref(),
            })?;
        let found_ids = RegisteredIds {
            image_id: found_id.clone(),
            snapshot_ids,
        };

        public = ami_is_public(&base_ec2_client, base_region.as_ref(), &found_id)
            .await
            .context(error::IsAmiPublicSnafu {
                image_id: found_id.clone(),
                region: base_region.to_string(),
            })?;

        launch_permissions =
            get_launch_permissions(&base_ec2_client, base_region.as_ref(), &found_id)
                .await
                .context(error::DescribeImageAttributeSnafu {
                    image_id: found_id,
                    region: base_region.to_string(),
                })?;

        (found_ids, true)
    } else {
        info!(
            "Registering '{}' in {}",
            tentative_amispec.name, base_region
        );
        let new_ids = register_image(ami_args, &base_region, base_ebs_client, &base_ec2_client)
            .await
            .context(error::RegisterImageSnafu {
                name: &tentative_amispec.name,
                arch: &arch,
                region: base_region.as_ref(),
            })?;
        info!(
            "Registered AMI '{}' in {}: {}",
            tentative_amispec.name, base_region, new_ids.image_id
        );
        (new_ids, false)
    };

    // Eliminate our reference to `ami_args` to force the use of `tentative_amispec` to determine
    // AMI properties.
    #[expect(unused_variables)]
    let ami_args = ();

    // Resolve the account ID behind the base region's (base) role; this keys the source AMI in our
    // output map. When a role is configured we parse the account ID out of its ARN; otherwise we
    // fall back to STS to discover the account behind the base/global credentials.
    let base_account_id = match &base_role {
        Some(role) => account_id_from_role_arn(role)
            .context(error::ParseRoleArnSnafu { role: role.clone() })?,
        None => {
            let base_sts_client = StsClient::new(&client_config);
            get_account_id(&base_sts_client, &base_region).await?
        }
    };

    amis.entry(base_region.as_ref().to_string())
        .or_default()
        .insert(
            base_account_id.clone(),
            Image::new(
                &ids_of_image.image_id,
                &tentative_amispec.name,
                Some(public),
                Some(launch_permissions),
            ),
        );

    // Build the full list of (region, role) targets we need to copy into, which is the cartesian
    // product of every region and every role configured for it, minus the source (base region,
    // base role) pair which we just registered above.
    let mut targets = Vec::new();
    for region in std::iter::once(&base_region).chain(regions.iter()) {
        for role in roles_for_region(&aws, region) {
            // Skip the source pair; it's already registered.
            if region.as_ref() == base_region.as_ref() && role == base_role {
                continue;
            }
            targets.push(RegionRole {
                region: region.clone(),
                role,
            });
        }
    }

    // If we don't need to copy AMIs to any other target, we're done.
    if targets.is_empty() {
        return Ok(amis);
    }

    // Wait for AMI to be available so it can be copied
    let successes_required = if already_registered { 1 } else { 3 };
    wait_for_ami(
        &ids_of_image.image_id,
        &base_region,
        &base_region,
        "available",
        successes_required,
        &aws,
        base_role.clone(),
    )
    .await
    .context(error::WaitAmiSnafu {
        id: &ids_of_image.image_id,
        region: base_region.as_ref(),
    })?;

    // For every other target, initiate copy-image calls.

    // First, make EC2 and STS clients per target so we can resolve account IDs, fetch, and copy
    // AMIs.  We make a map storing our clients because they're used in a future and need to live
    // until the future is resolved.
    let mut ec2_clients = HashMap::with_capacity(targets.len());
    for target in targets.iter() {
        let client_config = build_client_config_for_role(
            &target.region,
            &base_region,
            &aws,
            target.role.as_deref(),
        )
        .await;
        let ec2_client = Ec2Client::new(&client_config);
        ec2_clients.insert(target.clone(), ec2_client);
    }

    // Resolve the account ID behind each target's role so we can key the output map and grant
    // access to the source snapshots/AMI.
    info!("Getting account IDs for targets so we can grant access to copy source AMI");
    let account_ids = get_account_ids(&targets, &base_region, &aws).await?;

    // Grant access to every target account (other than the base account) so they can copy the AMI
    // and its snapshots.
    let grant_account_ids: HashSet<String> = account_ids
        .values()
        .filter(|account_id| **account_id != base_account_id)
        .cloned()
        .collect();
    if !grant_account_ids.is_empty() {
        info!("Granting access to target accounts so we can copy the AMI");
        let modify_options = ModifyOptions {
            user_ids: grant_account_ids.into_iter().collect(),
            group_names: Vec::new(),
            organization_arns: Vec::new(),
            organizational_unit_arns: Vec::new(),
        };

        modify_snapshots(
            &modify_options,
            &OperationType::Add,
            &ids_of_image.snapshot_ids,
            &base_ec2_client,
            &base_region,
        )
        .await
        .context(error::GrantAccessSnafu {
            thing: "snapshots",
            region: base_region.as_ref(),
        })?;

        modify_image(
            &modify_options,
            &OperationType::Add,
            &ids_of_image.image_id,
            &base_ec2_client,
        )
        .await
        .context(error::GrantImageAccessSnafu {
            thing: "image",
            region: base_region.as_ref(),
        })?;
    }

    // First, we check if the AMI already exists in each target (region + account).
    info!("Checking whether AMIs already exist in target regions/accounts");
    let mut get_requests = Vec::with_capacity(targets.len());
    for target in targets.iter() {
        let ec2_client = &ec2_clients[target];
        let get_request = get_ami_id(&tentative_amispec.name, &arch, &target.region, ec2_client);
        let info_future = ready(target.clone());
        get_requests.push(join(info_future, get_request));
    }
    let request_stream = stream::iter(get_requests).buffer_unordered(4);
    let get_responses: Vec<(
        RegionRole,
        std::result::Result<Option<String>, register::Error>,
    )> = request_stream.collect().await;

    // If an AMI already existed, just add it to our list, otherwise prepare a copy request.
    let mut copy_requests = Vec::with_capacity(targets.len());
    for (target, get_response) in get_responses {
        let region = &target.region;
        let account_id = &account_ids[&target];
        let get_response = get_response.context(error::GetAmiIdSnafu {
            name: &tentative_amispec.name,
            arch: &arch,
            region: region.as_ref(),
        })?;
        if let Some(id) = get_response {
            info!(
                "Found '{}' already registered in {} ({}): {}",
                tentative_amispec.name, region, account_id, id
            );
            let public = ami_is_public(&ec2_clients[&target], region.as_ref(), &id)
                .await
                .context(error::IsAmiPublicSnafu {
                    image_id: id.clone(),
                    region: base_region.to_string(),
                })?;

            let launch_permissions =
                get_launch_permissions(&ec2_clients[&target], region.as_ref(), &id)
                    .await
                    .context(error::DescribeImageAttributeSnafu {
                        region: region.as_ref(),
                        image_id: id.clone(),
                    })?;

            amis.entry(region.as_ref().to_string()).or_default().insert(
                account_id.clone(),
                Image::new(
                    &id,
                    &tentative_amispec.name,
                    Some(public),
                    Some(launch_permissions),
                ),
            );
            continue;
        }

        let ec2_client = &ec2_clients[&target];
        let base_region = base_region.to_owned();
        let copy_future = ec2_client
            .copy_image()
            .set_description(tentative_amispec.description.clone())
            .set_name(Some(tentative_amispec.name.clone()))
            .set_source_image_id(Some(ids_of_image.image_id.clone()))
            .set_source_region(Some(base_region.as_ref().to_string()))
            .set_copy_image_tags(Some(true))
            .send()
            .map_err(AwsSdkError::from);

        // Store the target so we can output it to the user
        let target_future = ready(target.clone());
        // Let the user know the copy is starting, when this future goes to run
        let dest_region = region.clone();
        let dest_account = account_id.clone();
        let message_future = lazy(move |_| {
            info!("Starting copy from {base_region} to {dest_region} ({dest_account})")
        });
        copy_requests.push(message_future.then(|_| join(target_future, copy_future)));
    }

    // If all targets already have the AMI, we're done.
    if copy_requests.is_empty() {
        return Ok(amis);
    }

    // Start requests; they return almost immediately and the copying work is done by the service
    // afterward.  You should wait for the AMI status to be "available" before launching it.
    // (We still use buffer_unordered, rather than something like join_all, to retain some control
    // over the number of requests going out in case we need it later, but this will effectively
    // spin through all targets quickly because the requests return before any copying is done.)
    let request_stream = stream::iter(copy_requests).buffer_unordered(4);
    // Run through the stream and collect results into a list.
    let copy_responses: Vec<(
        RegionRole,
        std::result::Result<CopyImageOutput, AwsSdkError<CopyImageError>>,
    )> = request_stream.collect().await;

    // Report on successes and errors; don't fail immediately if we see an error so we can report
    // all successful IDs.
    let mut saw_error = false;
    for (target, copy_response) in copy_responses {
        let region = &target.region;
        let account_id = &account_ids[&target];
        match copy_response {
            Ok(success) => {
                if let Some(image_id) = success.image_id {
                    info!(
                        "Registered AMI '{}' in {} ({}): {}",
                        tentative_amispec.name, region, account_id, image_id,
                    );
                    amis.entry(region.as_ref().to_string()).or_default().insert(
                        account_id.clone(),
                        Image::new(
                            &image_id,
                            &tentative_amispec.name,
                            Some(false),
                            Some(vec![]),
                        ),
                    );
                } else {
                    saw_error = true;
                    error!(
                        "Registered AMI '{}' in {} ({}) but didn't receive an AMI ID!",
                        tentative_amispec.name, region, account_id,
                    );
                }
            }
            Err(e) => {
                saw_error = true;
                error!(
                    "Copy to {} ({}) failed: {}",
                    region,
                    account_id,
                    e.as_service_error()
                        .and_then(|err| err.code())
                        .unwrap_or("unknown")
                );
            }
        }
    }

    ensure!(!saw_error, error::AmiCopySnafu);

    Ok(amis)
}

/// An account ID, in its 12-digit string form.
pub(crate) type AccountId = String;

/// The output of an AMI publish run: for each region, a map of account ID to the AMI that was
/// registered or copied into that account.  A single region can map to multiple accounts when
/// multiple roles are configured for it.  The `ssm` and `publish-ami` subcommands consume this
/// (serialized to JSON, inside an [`AmiInputFile`] envelope) to know which AMIs exist where.
pub(crate) type RegionAccountImageMap = HashMap<String, HashMap<AccountId, Image>>;

/// The current schema version of the `--ami-input` / `--ami-output` JSON file.
///
/// Version 1 is the legacy, pre-multi-account format: a bare `{region: Image}` map with no
/// version field. It was never written with an explicit `schema_version`, so we don't define a
/// constant for it; it's recognized structurally on read (see [`AmiInputFile`]).
///
/// Version 2 introduced the per-account nesting (`{region: {account: Image}}`) and the explicit
/// `schema_version` envelope. New files are always written as v2.
pub(crate) const AMI_INPUT_SCHEMA_VERSION: u32 = 2;

/// The current (v2) envelope written to `--ami-output`. Wrapping the AMI map in a struct with an
/// explicit `schema_version` lets consumers (including external tooling) detect the format and
/// lets us evolve it without silently misparsing.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct AmiInputFile {
    /// The schema version of this file; see [`AMI_INPUT_SCHEMA_VERSION`].
    pub(crate) schema_version: u32,
    /// The region -> account -> AMI mapping.
    pub(crate) amis: RegionAccountImageMap,
}

/// The result of parsing an `--ami-input` file: either the current versioned, per-account map, or
/// a legacy v1 map that still needs to be lifted into the per-account model (see
/// [`lift_legacy_ami_input`]).
#[derive(Debug)]
pub(crate) enum AmiInput {
    /// The current (v2+) format: a region -> account -> AMI map.
    Versioned(RegionAccountImageMap),
    /// The legacy v1 format: a bare region -> AMI map, with no account dimension.
    Legacy(HashMap<String, Image>),
}

/// A tolerant reader for the `--ami-input` file that accepts either the current versioned format
/// or the legacy unversioned one. This is `#[serde(untagged)]` so that v1 files - which predate
/// the `schema_version` field - still parse without it, while v2+ files are recognized by their
/// envelope. Serde tries the variants top-to-bottom, so `Versioned` must come first.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum MaybeVersionedAmiInput {
    /// The current format: an explicit `schema_version` plus the nested AMI map.
    Versioned(AmiInputFile),
    /// The legacy v1 format: a bare `region -> Image` map, no version field.
    Legacy(HashMap<String, Image>),
}

impl AmiInputFile {
    /// Wraps an AMI map in an envelope stamped with the current schema version.
    pub(crate) fn new(amis: RegionAccountImageMap) -> Self {
        Self {
            schema_version: AMI_INPUT_SCHEMA_VERSION,
            amis,
        }
    }

    /// Parses an `--ami-input` file from JSON bytes, classifying it as either the current
    /// versioned format or a legacy v1 file. A versioned file with an unrecognized
    /// `schema_version` is rejected. Legacy files are returned as-is for the caller to lift via
    /// [`lift_legacy_ami_input`] (which needs config/credentials this sync parser doesn't have).
    pub(crate) fn from_json(bytes: &[u8]) -> std::result::Result<AmiInput, AmiInputError> {
        let parsed: MaybeVersionedAmiInput = serde_json::from_slice(bytes)
            .map_err(|source| AmiInputError::Deserialize { source })?;
        match parsed {
            MaybeVersionedAmiInput::Versioned(file) => {
                ensure!(
                    file.schema_version == AMI_INPUT_SCHEMA_VERSION,
                    UnsupportedVersionSnafu {
                        found: file.schema_version,
                        supported: AMI_INPUT_SCHEMA_VERSION,
                    }
                );
                Ok(AmiInput::Versioned(file.amis))
            }
            MaybeVersionedAmiInput::Legacy(map) => Ok(AmiInput::Legacy(map)),
        }
    }
}

/// Reads an `--ami-input` file and returns it in the current per-account form, transparently
/// lifting a legacy v1 file if necessary. This is the entry point the `ssm` and `publish-ami`
/// subcommands should use.
pub(crate) async fn read_ami_input(
    bytes: &[u8],
    pubsys_aws_config: &PubsysAwsConfig,
    base_region: &Region,
) -> std::result::Result<RegionAccountImageMap, AmiInputError> {
    match AmiInputFile::from_json(bytes)? {
        AmiInput::Versioned(amis) => Ok(amis),
        AmiInput::Legacy(legacy) => {
            info!("AMI input is in the legacy (v1) format; lifting it into the per-account model");
            lift_legacy_ami_input(legacy, pubsys_aws_config, base_region).await
        }
    }
}

/// Lifts a legacy (v1) `region -> Image` map into the current per-account model. A v1 file has one
/// AMI per region and no account dimension, so for each region we resolve the single account that
/// AMI belongs to: from the region's configured role ARN when one is set, or - for a region using
/// the base/global credentials with no role - via an STS `GetCallerIdentity` call.
pub(crate) async fn lift_legacy_ami_input(
    legacy: HashMap<String, Image>,
    pubsys_aws_config: &PubsysAwsConfig,
    base_region: &Region,
) -> std::result::Result<RegionAccountImageMap, AmiInputError> {
    let mut amis: RegionAccountImageMap = HashMap::new();
    for (region_name, image) in legacy {
        let region = region_from_string(&region_name);
        let account_id = region_account_id(&region, pubsys_aws_config, base_region).await?;
        amis.entry(region_name)
            .or_default()
            .insert(account_id, image);
    }
    Ok(amis)
}

/// Resolves the single account ID reachable in a region under the current config: the account
/// embedded in the region's configured role ARN, or - for a region with no role - the account
/// behind the base/global credentials (discovered via STS).
///
/// This is the right notion of "the account for a region" for the single-account-per-region flows
/// (`promote-ssm`, `validate-ssm`, and legacy `--ami-input` lifting). For a region configured with
/// multiple `roles`, it returns the first.
pub(crate) async fn region_account_id(
    region: &Region,
    pubsys_aws_config: &PubsysAwsConfig,
    base_region: &Region,
) -> std::result::Result<AccountId, AmiInputError> {
    let maybe_role = roles_for_region(pubsys_aws_config, region)
        .into_iter()
        .next()
        .flatten();

    match maybe_role {
        // The account ID is embedded in the role ARN - no API call needed.
        Some(role) => {
            account_id_from_role_arn(&role).context(LiftParseRoleArnSnafu { role: role.clone() })
        }
        // No role: ask STS which account the base/global credentials land in.
        None => {
            let client_config =
                build_client_config_for_role(region, base_region, pubsys_aws_config, None).await;
            let sts_client = StsClient::new(&client_config);
            let response = sts_client
                .get_caller_identity()
                .send()
                .await
                .map_err(AwsSdkError::from)
                .map_err(|source| AmiInputError::LiftResolveAccount {
                    region: region.as_ref().to_string(),
                    source: Box::new(source),
                })?;
            response.account.context(LiftMissingAccountSnafu {
                region: region.as_ref().to_string(),
            })
        }
    }
}

/// Errors that can occur while parsing or lifting an `--ami-input` file.
#[derive(Debug, snafu::Snafu)]
#[snafu(visibility(pub(crate)))]
pub(crate) enum AmiInputError {
    #[snafu(display(
        "AMI input file has schema_version {} but this pubsys only supports version {}. \
         Regenerate it with `pubsys ami` using a compatible version.",
        found,
        supported
    ))]
    UnsupportedVersion { found: u32, supported: u32 },

    #[snafu(display("Failed to deserialize AMI input file: {}", source))]
    Deserialize { source: serde_json::Error },

    #[snafu(display(
        "Failed to parse an account ID out of the role ARN '{}' while lifting a legacy AMI input \
         file; expected the form 'arn:aws:iam::<account>:role/<name>'",
        role
    ))]
    LiftParseRoleArn { role: String },

    #[snafu(display(
        "Failed to resolve the account for legacy region '{}' via STS: {}",
        region,
        source
    ))]
    LiftResolveAccount {
        region: String,
        source: Box<AwsSdkError<GetCallerIdentityError>>,
    },

    #[snafu(display("STS did not return an account ID for legacy region '{}'", region))]
    LiftMissingAccount { region: String },
}

/// Identifies a single copy target: a region together with the role to assume to reach a
/// particular account in that region.  A `None` role means "use the base/global credentials".
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct RegionRole {
    pub(crate) region: Region,
    pub(crate) role: Option<String>,
}

/// Identifies a single AMI: a region together with the account that owns the AMI.  A single region
/// can contain multiple accounts when multiple roles are configured for it.  The `ssm` and
/// `publish-ami` subcommands use this to key AMIs read from the input JSON.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct RegionAccount {
    pub(crate) region: Region,
    pub(crate) account_id: AccountId,
}

/// Returns the publish targets for the given region: one entry per account to publish to, where
/// `Some(role)` means "assume this role" and `None` means "use the base/global credentials without
/// assuming a region-specific role".  A region with no configured role (or no config entry at all)
/// yields a single `None` target.
fn roles_for_region(pubsys_aws_config: &PubsysAwsConfig, region: &Region) -> Vec<Option<String>> {
    match pubsys_aws_config
        .region
        .get(region.as_ref())
        .and_then(|region_config| region_config.all_roles())
    {
        Some(roles) => roles.into_iter().map(Some).collect(),
        None => vec![None],
    }
}

/// If JSON output was requested, we serialize out a mapping of region to AMI information; this
/// struct holds the information we save about each AMI.  The `ssm` subcommand uses this
/// information to populate templates representing SSM parameter names and values.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq, Hash)]
pub(crate) struct Image {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) public: Option<bool>,
    pub(crate) launch_permissions: Option<Vec<LaunchPermissionDef>>,
}

impl Image {
    fn new(
        id: &str,
        name: &str,
        public: Option<bool>,
        launch_permissions: Option<Vec<LaunchPermissionDef>>,
    ) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            public,
            launch_permissions,
        }
    }
}

/// Resolves the account ID reachable through the given STS client (in the given region, used only
/// for error messages).
async fn get_account_id(sts_client: &StsClient, region: &Region) -> Result<String> {
    let response = sts_client
        .get_caller_identity()
        .send()
        .await
        .map_err(AwsSdkError::from)
        .context(error::GetCallerIdentitySnafu {
            region: region.as_ref(),
        })?;
    response.account.context(error::MissingInResponseSnafu {
        request_type: "GetCallerIdentity",
        missing: "account",
    })
}

/// Parses the 12-digit account ID out of an IAM role ARN of the form
/// `arn:aws:iam::<account>:role/<name>` (the partition may vary, e.g. `aws-us-gov`). Returns
/// `None` if the string isn't an ARN with an account ID in the expected position.
fn account_id_from_role_arn(role_arn: &str) -> Option<String> {
    // ARN format: arn:<partition>:<service>:<region>:<account-id>:<resource>
    // For IAM roles the region is empty and the account ID is the 5th colon-delimited field.
    let fields: Vec<&str> = role_arn.split(':').collect();
    let account_id = fields.get(4)?;
    // Account IDs are exactly 12 digits; guard against malformed input.
    if account_id.len() == 12 && account_id.bytes().all(|b| b.is_ascii_digit()) {
        Some((*account_id).to_string())
    } else {
        None
    }
}

/// Resolves the account ID for each given target. When the target has a role, the account ID is
/// parsed out of the role ARN (no API call). For any target without a role - i.e. using the
/// base/global credentials - we fall back to STS GetCallerIdentity to discover the account.
async fn get_account_ids(
    targets: &[RegionRole],
    base_region: &Region,
    pubsys_aws_config: &PubsysAwsConfig,
) -> Result<HashMap<RegionRole, String>> {
    let mut account_ids = HashMap::with_capacity(targets.len());

    // Targets without a role need STS to discover their account; collect them for a parallel
    // lookup. Targets with a role are resolved synchronously by parsing the ARN.
    let mut sts_targets = Vec::new();
    for target in targets.iter() {
        match &target.role {
            Some(role) => {
                let account_id = account_id_from_role_arn(role)
                    .context(error::ParseRoleArnSnafu { role: role.clone() })?;
                account_ids.insert(target.clone(), account_id);
            }
            None => sts_targets.push(target.clone()),
        }
    }

    if !sts_targets.is_empty() {
        // We make a map storing our clients because they're used in a future and need to live
        // until the future is resolved.
        let mut sts_clients = HashMap::with_capacity(sts_targets.len());
        for target in sts_targets.iter() {
            let client_config = build_client_config_for_role(
                &target.region,
                base_region,
                pubsys_aws_config,
                target.role.as_deref(),
            )
            .await;
            sts_clients.insert(target.clone(), StsClient::new(&client_config));
        }

        let mut requests = Vec::with_capacity(sts_targets.len());
        for target in sts_targets.iter() {
            let sts_client = &sts_clients[target];
            let response_future = sts_client
                .get_caller_identity()
                .send()
                .map_err(AwsSdkError::from);

            // Store the target so we can include it in any errors and key the result
            let target_future = ready(target.clone());
            requests.push(join(target_future, response_future));
        }

        let request_stream = stream::iter(requests).buffer_unordered(4);
        // Run through the stream and collect results into a list.
        let responses: Vec<(
            RegionRole,
            std::result::Result<GetCallerIdentityOutput, AwsSdkError<GetCallerIdentityError>>,
        )> = request_stream.collect().await;

        for (target, response) in responses {
            let response = response.context(error::GetCallerIdentitySnafu {
                region: target.region.as_ref(),
            })?;
            let account_id = response.account.context(error::MissingInResponseSnafu {
                request_type: "GetCallerIdentity",
                missing: "account",
            })?;
            account_ids.insert(target, account_id);
        }
    }

    trace!("Found account IDs {account_ids:?}");

    Ok(account_ids)
}

/// Parses a toml file, returning a `FromPath<T>`.
pub(crate) fn parse_toml_file<T: DeserializeOwned>(filepath: &str) -> Result<FromPath<T>> {
    let toml_str = std::fs::read_to_string(filepath).context(error::ReadFileSnafu { filepath })?;
    let toml_deserializer =
        toml::Deserializer::parse(&toml_str).context(error::ParseTomlSnafu { filepath })?;
    FromPath::deserialize_from_path(filepath, toml_deserializer)
        .context(error::ParseTomlSnafu { filepath })
}

/// Helper type that wraps buildsys::manifest::ManifestInfo but includes the filepath from which
/// it was loaded.
///
/// This allows us to emit more helpful error messages when an error occurs due to a variant
/// definition.
pub(crate) type VariantManifest = FromPath<ManifestInfo>;

pub(crate) fn parse_variant_manifest(filepath: &str) -> Result<VariantManifest> {
    let manifest =
        ManifestInfo::new(filepath).context(error::LoadVariantManifestSnafu { filepath })?;
    Ok(FromPath::new_from_path(manifest, filepath))
}

pub(crate) fn parse_uefi_data(filepath: &str) -> Result<FromPath<String>> {
    let uefi_data = std::fs::read_to_string(filepath).context(error::ReadFileSnafu { filepath })?;
    Ok(FromPath::new_from_path(uefi_data, filepath))
}

mod error {
    use super::register::mk_amispec;
    use crate::aws::{ami, publish_ami};
    use aws_sdk_ec2::operation::modify_image_attribute::ModifyImageAttributeError;
    use aws_sdk_sts::operation::get_caller_identity::GetCallerIdentityError;
    use buildsys::manifest;
    use error_utils::AwsSdkError;
    use snafu::Snafu;
    use std::path::PathBuf;

    use super::public;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub(super)))]
    pub(crate) enum Error {
        #[snafu(display("Some AMIs failed to copy, see above"))]
        AmiCopy,

        #[snafu(display("Failed to create an amispec from publication inputs: {}", source))]
        Amispec { source: mk_amispec::AmispecError },

        #[snafu(display("Error reading config: {}", source))]
        Config { source: pubsys_config::Error },

        #[snafu(display(
            "Failed to describe image attributes for image {} in region {}: {}",
            image_id,
            region,
            source
        ))]
        DescribeImageAttribute {
            image_id: String,
            region: String,
            #[snafu(source(from(super::launch_permissions::Error, Box::new)))]
            source: Box<super::launch_permissions::Error>,
        },

        #[snafu(display("Error getting AMI ID for {} {} in {}: {}", arch, name, region, source))]
        GetAmiId {
            name: String,
            arch: String,
            region: String,
            #[snafu(source(from(ami::register::Error, Box::new)))]
            source: Box<ami::register::Error>,
        },

        #[snafu(display("Error getting account ID in {}: {}", region, source))]
        GetCallerIdentity {
            region: String,
            #[snafu(source(from(AwsSdkError<GetCallerIdentityError>, Box::new)))]
            source: Box<AwsSdkError<GetCallerIdentityError>>,
        },

        #[snafu(display(
            "Failed to get snapshot IDs associated with {} in {}: {}",
            image_id,
            region,
            source
        ))]
        GetSnapshots {
            image_id: String,
            region: String,
            #[snafu(source(from(publish_ami::Error, Box::new)))]
            source: Box<publish_ami::Error>,
        },

        #[snafu(display("Failed to grant access to {} in {}: {}", thing, region, source))]
        GrantAccess {
            thing: String,
            region: String,
            #[snafu(source(from(publish_ami::Error, Box::new)))]
            source: Box<publish_ami::Error>,
        },

        #[snafu(display("Failed to grant access to {} in {}: {}", thing, region, source))]
        GrantImageAccess {
            thing: String,
            region: String,
            #[snafu(source(from(AwsSdkError<ModifyImageAttributeError>, Box::new)))]
            source: Box<AwsSdkError<ModifyImageAttributeError>>,
        },

        #[snafu(display(
            "Failed to check if AMI with id {} is public in {}: {}",
            image_id,
            region,
            source
        ))]
        IsAmiPublic {
            image_id: String,
            region: String,
            source: public::Error,
        },

        #[snafu(display("Failed to load variant manifest from {}: {}", filepath.display(), source))]
        LoadVariantManifest {
            filepath: PathBuf,
            #[snafu(source(from(manifest::Error, Box::new)))]
            source: Box<manifest::Error>,
        },

        #[snafu(display(
            "amispec rendered from pubsys inputs is missing a machine architecture."
        ))]
        MissingArchitecture,

        #[snafu(display("Infra.toml is missing {}", missing))]
        MissingConfig { missing: String },

        #[snafu(display("Response to {} was missing {}", request_type, missing))]
        MissingInResponse {
            request_type: String,
            missing: String,
        },

        #[snafu(display(
            "Failed to parse an account ID out of the role ARN '{}'; expected the form \
             'arn:aws:iam::<account>:role/<name>'",
            role
        ))]
        ParseRoleArn { role: String },

        #[snafu(display("Failed to parse file {} as toml: {}", filepath.display(), source))]
        ParseToml {
            filepath: PathBuf,
            source: toml::de::Error,
        },

        #[snafu(display("Failed to read file {}: {}", filepath.display(), source))]
        ReadFile {
            filepath: PathBuf,
            source: std::io::Error,
        },

        #[snafu(display("Error registering {} {} in {}: {}", arch, name, region, source))]
        RegisterImage {
            name: String,
            arch: String,
            region: String,
            #[snafu(source(from(ami::register::Error, Box::new)))]
            source: Box<ami::register::Error>,
        },

        #[snafu(display("AMI '{}' in {} did not become available: {}", id, region, source))]
        WaitAmi {
            id: String,
            region: String,
            #[snafu(source(from(ami::wait::Error, Box::new)))]
            source: Box<ami::wait::Error>,
        },

        #[snafu(display("Failed to write AMIs to '{}': {}", path.display(), source))]
        WriteAmis {
            path: PathBuf,
            #[snafu(source(from(publish_ami::Error, Box::new)))]
            source: Box<publish_ami::Error>,
        },
    }
}
pub(crate) use error::Error;

use self::launch_permissions::LaunchPermissionDef;

use super::publish_ami::write_amis;
type Result<T> = std::result::Result<T, error::Error>;

#[cfg(test)]
mod test {
    use super::{
        account_id_from_role_arn, lift_legacy_ami_input, AmiInput, AmiInputError, AmiInputFile,
        Image, Region, RegionAccountImageMap, AMI_INPUT_SCHEMA_VERSION,
    };
    use std::collections::HashMap;

    #[test]
    fn account_id_parsed_from_role_arn() {
        assert_eq!(
            account_id_from_role_arn("arn:aws:iam::012345678901:role/assume-regional"),
            Some("012345678901".to_string())
        );
        // Non-commercial partitions still have the account ID in the same position.
        assert_eq!(
            account_id_from_role_arn("arn:aws-us-gov:iam::098765432109:role/some/path"),
            Some("098765432109".to_string())
        );
    }

    #[test]
    fn account_id_from_malformed_arn_is_none() {
        // Not an ARN at all.
        assert_eq!(account_id_from_role_arn("not-an-arn"), None);
        // Account-ID field present but not 12 digits.
        assert_eq!(
            account_id_from_role_arn("arn:aws:iam::123:role/too-short"),
            None
        );
        // Account-ID field present but non-numeric.
        assert_eq!(
            account_id_from_role_arn("arn:aws:iam::notanaccount:role/x"),
            None
        );
    }

    /// The `--ami-input` JSON contract is shared by the `ami`, `ssm`, and `publish-ami`
    /// subcommands.  This locks its versioned envelope around the nested
    /// `region -> account -> image` shape.
    #[test]
    fn ami_input_file_json_round_trip() {
        let json = r#"{
            "schema_version": 2,
            "amis": {
                "us-west-2": {
                    "777777777777": {
                        "id": "ami-aaa",
                        "name": "my-ami",
                        "public": false,
                        "launch_permissions": []
                    },
                    "999999999999": {
                        "id": "ami-bbb",
                        "name": "my-ami",
                        "public": false,
                        "launch_permissions": []
                    }
                },
                "us-east-1": {
                    "777777777777": {
                        "id": "ami-ccc",
                        "name": "my-ami",
                        "public": true,
                        "launch_permissions": null
                    }
                }
            }
        }"#;

        let parsed = expect_versioned(AmiInputFile::from_json(json.as_bytes()).unwrap());

        // Two accounts in us-west-2, one in us-east-1.
        assert_eq!(parsed["us-west-2"].len(), 2);
        assert_eq!(parsed["us-west-2"]["777777777777"].id, "ami-aaa");
        assert_eq!(parsed["us-west-2"]["999999999999"].id, "ami-bbb");
        assert_eq!(parsed["us-east-1"].len(), 1);
        assert_eq!(parsed["us-east-1"]["777777777777"].public, Some(true));

        // Round-trips back through the envelope to an equivalent structure.
        let reserialized = serde_json::to_string(&AmiInputFile::new(parsed.clone())).unwrap();
        let reparsed = expect_versioned(AmiInputFile::from_json(reserialized.as_bytes()).unwrap());
        assert_eq!(parsed, reparsed);
    }

    /// A single account per region (the common case) is just a one-entry inner map.
    #[test]
    fn ami_input_file_single_account() {
        let mut map = RegionAccountImageMap::new();
        map.entry("us-west-2".to_string()).or_default().insert(
            "123456789012".to_string(),
            Image::new("ami-123", "my-ami", Some(false), Some(vec![])),
        );

        let json = serde_json::to_string(&AmiInputFile::new(map)).unwrap();
        let parsed = expect_versioned(AmiInputFile::from_json(json.as_bytes()).unwrap());
        assert_eq!(parsed["us-west-2"]["123456789012"].id, "ami-123");
    }

    /// What `pubsys ami` writes must be readable by `from_json` (the format the other subcommands
    /// consume).
    #[test]
    fn ami_input_file_written_format_is_versioned() {
        let map = RegionAccountImageMap::new();
        let written = serde_json::to_value(AmiInputFile::new(map)).unwrap();
        assert_eq!(written["schema_version"], AMI_INPUT_SCHEMA_VERSION);
        assert!(written.get("amis").is_some());
    }

    /// A file with a different schema version is rejected with a clear error rather than
    /// misparsed.
    #[test]
    fn ami_input_file_rejects_unsupported_version() {
        let json = r#"{"schema_version": 999, "amis": {}}"#;
        let err = AmiInputFile::from_json(json.as_bytes()).unwrap_err();
        assert!(matches!(
            err,
            AmiInputError::UnsupportedVersion { found: 999, .. }
        ));
    }

    /// A legacy (v1, unversioned) file is recognized and returned as `Legacy` for the caller to
    /// lift, rather than being misparsed as the versioned format.
    #[test]
    fn ami_input_file_recognizes_legacy() {
        // This is the old pre-envelope shape: a bare region -> image map.
        let json = r#"{"us-west-2": {"id": "ami-aaa", "name": "x", "public": false, "launch_permissions": []}}"#;
        let parsed = AmiInputFile::from_json(json.as_bytes()).unwrap();
        match parsed {
            AmiInput::Legacy(map) => {
                assert_eq!(map["us-west-2"].id, "ami-aaa");
            }
            AmiInput::Versioned(_) => panic!("legacy file should not parse as versioned"),
        }
    }

    /// Lifting a legacy file resolves each region's account from its configured role ARN, with no
    /// API calls needed in the common (role-configured) case.
    #[tokio::test]
    async fn lift_legacy_uses_role_arn() {
        use pubsys_config::{AwsConfig, AwsRegionConfig, RoleConfig};

        let mut region = std::collections::HashMap::new();
        region.insert(
            "us-west-2".to_string(),
            AwsRegionConfig {
                role_config: Some(RoleConfig::Role(
                    "arn:aws:iam::012345678901:role/assume-regional".to_string(),
                )),
            },
        );
        let aws = AwsConfig {
            region,
            ..AwsConfig::default()
        };

        let mut legacy = HashMap::new();
        legacy.insert(
            "us-west-2".to_string(),
            Image::new("ami-aaa", "my-ami", Some(false), Some(vec![])),
        );

        let base_region = Region::new("us-west-2");
        let lifted = lift_legacy_ami_input(legacy, &aws, &base_region)
            .await
            .unwrap();

        // The single region's AMI is now keyed under the account parsed from the role ARN.
        assert_eq!(lifted["us-west-2"].len(), 1);
        assert_eq!(lifted["us-west-2"]["012345678901"].id, "ami-aaa");
    }

    /// Unwraps the versioned variant for tests that only exercise the current format.
    fn expect_versioned(input: AmiInput) -> RegionAccountImageMap {
        match input {
            AmiInput::Versioned(map) => map,
            AmiInput::Legacy(_) => panic!("expected versioned AMI input"),
        }
    }
}
