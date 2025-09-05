//! This module generates an `amispec` to be used during the AMI registration process.
//!
//! An `amispec` defines the arguments that are passed to the EC2 RegisterImage call, including the
//! EBS block device mappings which control EBS volumes and their properties.
//!
//! `amispec`s can be defined using handlebars-templated values for any field, allowing pubsys to
//! inject AMI registration settings into the user-provided template.
//!
//! The `amispec` used to register an AMI is created using the following process:
//! * A default `amispec` template is defined as a TOML table within pubsys
//! * The default TOML table is modified to support features of the variant that is being registered
//!     * Default EBS block device mappings are created for each snapshot
//!     * If the variant supports secure boot, then the template has uefi-data updated, and sets
//!       the default `boot-mode` to `uefi-preferred`
//! * The default TOML template is merged with the user-provided TOML document, using a recursive
//!   "upsert" procedure.
//! * The resulting TOML document is parsed as a `TemplatedAmiSpec`
//! * The `TemplatedAmiSpec` is finally rendered into the final `AmiSpec`.
//!
//! The handlebars render context is defined in [`AmispecRenderContext`] and includes basic
//! AMI properties like the desired architecture and name, as well as information regarding
//! EBS block devices required for the volumes based on the variant definition.
use super::merge_toml::Merge;
use super::BottlerocketSnapshots;
use crate::aws::ami::VariantManifest;
use amispec::{
    ebs::{EbsBlockDeviceKind, InvalidValueError},
    AmiSpec, TemplateOf, TemplatedAmiSpec,
};
use buildsys::manifest::{ImageFeature, PartitionPlan};
use log::{debug, info, warn};
use serde::Serialize;
use snafu::{OptionExt, ResultExt, Snafu};
use std::collections::HashMap;

use crate::aws::ami::AmiArgs;

type Result<T, E = AmispecError> = std::result::Result<T, E>;

const ROOT_DEVICE_NAME: &str = "/dev/xvda";
const DATA_DEVICE_NAME: &str = "/dev/xvdb";
pub(crate) const VIRT_TYPE: &str = "hvm";

lazy_static::lazy_static! {
    /// The default amispec template.
    ///
    /// pubsys modifies this structure based on attributes of the variant being registered:
    /// * If the variant has Secure Boot enabled,
    ///     * The boot-mode is set to uefi-preferred
    ///     * uefi-data is added to the template
    static ref DEFAULT_AMISPEC_TEMPLATE: toml::Table = toml::toml! {
        name = "{{ ami.unique_name }}"
        description = "{{ ami.description }}"
        architecture = "{{ ami.arch }}"
        root-device-name = "/dev/xvda"
        sriov-net-support = "simple"
        virtualization-type = VIRT_TYPE
        ena-support = true
    };

    static ref DEFAULT_ROOT_DEVICE_TEMPLATE: toml::Table = toml::toml! {
        ["/dev/xvda".ebs]
        volume-type = "gp2"
        volume-size = "{{ block_devices.root.volume_size }}"
        snapshot-id = "{{ block_devices.root.snapshot_id }}"
        delete-on-termination = true
    };

    static ref DEFAULT_DATA_DEVICE_TEMPLATE: toml::Table = toml::toml! {
        ["/dev/xvdb".ebs]
        volume-type = "gp2"
        volume-size = "{{ block_devices.data.volume_size }}"
        snapshot-id = "{{ block_devices.data.snapshot_id }}"
        delete-on-termination = true
    };
}

/// Creates an `amispec` that defines the parameters that are used when registering an AMI.
pub(crate) fn create_amispec(
    ami_args: &AmiArgs,
    bottlerocket_snapshots: Option<&BottlerocketSnapshots>,
) -> Result<AmiSpec> {
    let amispec_template_toml = default_amispec_template(
        &ami_args.variant_manifest,
        bottlerocket_snapshots,
        ami_args.uefi_data.as_ref().map(|s| s.as_str()),
    )?;

    let render_context = AmispecRenderContext::from_ami_args(ami_args, bottlerocket_snapshots)?;
    debug!("Creating amispec with render context:\n{render_context:#?}",);

    let mut amispec = create_amispec_from_base(amispec_template_toml, &render_context, ami_args)?;

    ensure_amispec_volume_sizes(&mut amispec, &render_context.block_devices)?;

    Ok(amispec)
}

fn create_amispec_from_base(
    mut base_amispec_template: toml::Table,
    render_context: &AmispecRenderContext,
    ami_args: &AmiArgs,
) -> Result<AmiSpec> {
    use amispec_error::*;

    if let Some(user_amispec_template) = &ami_args.amispec_file {
        info!("Merging user-defined amispec template for AMI properties");
        base_amispec_template.merge(user_amispec_template);
    } else {
        info!("Using default amispec for AMI properties");
    }

    debug!("Creating amispec with template:\n{base_amispec_template}");
    let amispec_template: TemplatedAmiSpec = base_amispec_template
        .try_into()
        .context(ParseAmiSpecTemplateSnafu)?;

    amispec_template
        .render(&render_context)
        .context(RenderAmiSpecSnafu)
}

/// A partial rendered AmiSpec
///
/// Useful for determining the desired name, description, and architecture of an AMI prior to the
/// creation of EBS snapshots.
pub(crate) struct MinimalAmiSpec {
    pub name: String,
    pub description: Option<String>,
    pub architecture: Option<amispec::Architecture>,
    pub tags: Option<HashMap<String, String>>,
}

impl From<AmiSpec> for MinimalAmiSpec {
    fn from(amispec: AmiSpec) -> Self {
        let AmiSpec {
            name,
            description,
            architecture,
            tags,
            ..
        } = amispec;

        Self {
            name,
            description,
            architecture,
            tags,
        }
    }
}

pub(crate) fn create_minimal_amispec(ami_args: &AmiArgs) -> Result<MinimalAmiSpec> {
    let render_context = AmispecRenderContext::from_ami_args(ami_args, None)?;
    create_amispec_from_base(DEFAULT_AMISPEC_TEMPLATE.clone(), &render_context, ami_args)
        .map(Into::into)
}

/// Configure's the volume sizes for an AmiSpec based on a desired block device layout.
///
/// Only devices referred to by the `block_devices` map are altered.
///
/// If the `AmiSpec` already specifies a volume size, they are only altered if the specified size is
/// less than the sizes specified in the given `block_devices` map. In this case, a warning message
/// is logged.
fn ensure_amispec_volume_sizes(
    amispec: &mut AmiSpec,
    block_devices: &HashMap<String, BlockDeviceRenderContext>,
) -> Result<()> {
    use amispec_error::*;

    let bdms = match amispec.block_device_mappings.as_mut() {
        Some(bdms) => bdms,
        None => return Ok(()),
    };

    let mut ebs_volumes_with_size_hints = bdms
        .values_mut()
        // Keep block device mappings for EBS volumes that have them
        .filter_map(|bdm| bdm.ebs.as_mut())
        // Keep EBS volumes and their size hints if the volume has a size hint associated with the
        // amispec's snapshot ID.
        .filter_map(|ebs_volume| {
            ebs_volume
                .snapshot_id()
                .map(str::to_string)
                .and_then(|snapshot_id| {
                    block_devices
                        .values()
                        .find(|device| device.snapshot_id == snapshot_id)
                        .map(|device| (ebs_volume, device))
                })
        });

    ebs_volumes_with_size_hints.try_for_each(|(ebs_volume, device_settings)| {
        let size_hint = device_settings.volume_size;

        debug!("Ensuring device '{}' requested with sufficient size", device_settings.device_name);
        if let Some(curr_size) = ebs_volume.volume_size() {
            if size_hint > curr_size {
                warn!(
                    "EBS volume '{ebs_volume:?}' was configured with size \
                                    '{curr_size}', but size hints dictate that it should be \
                                    larger. Using volume size '{size_hint}'"
                );
                ebs_volume
                    .set_volume_size(Some(size_hint))
                    .context(InvalidEbsVolumeSizeSnafu)?;
            } else {
                debug!("Volume for '{}' has sufficient size (requires '{size_hint}', has '{curr_size}'", device_settings.device_name)
            }
        } else {
            debug!("No volume size requested for '{}'. Using '{size_hint}'", device_settings.device_name);
            ebs_volume
                .set_volume_size(Some(size_hint))
                .context(InvalidEbsVolumeSizeSnafu)?;
        }
        Ok(())
    })
}

/// Returns the default amispec template for a given variant.
///
/// This should be `DEFAULT_AMISPEC_TEMPLATE`, with some changes based on the variant:
/// * The block device mappings contain a data device if one is specified
/// * If the variant has secure boot enabled:
///     * `boot-mode` is set to `uefi-preferred`
///     * `uefi-data` is set to the input string
fn default_amispec_template(
    variant_manifest: &VariantManifest,
    bottlerocket_snapshots: Option<&BottlerocketSnapshots>,
    uefi_data: Option<&str>,
) -> Result<toml::Table> {
    use amispec_error::*;

    let mut amispec_template = DEFAULT_AMISPEC_TEMPLATE.clone();
    let mut block_device_mappings = toml::Table::new();

    // Ensure the creation of new snapshots results in a compile error so that this is updated.
    if let Some(snapshots) = bottlerocket_snapshots {
        let BottlerocketSnapshots {
            os_snapshot: _os_snapshot,
            data_snapshot,
        } = snapshots;

        block_device_mappings.extend(DEFAULT_ROOT_DEVICE_TEMPLATE.clone());

        if data_snapshot.is_some() {
            block_device_mappings.extend(DEFAULT_DATA_DEVICE_TEMPLATE.clone());
        }
        amispec_template.insert("block-device-mappings".into(), block_device_mappings.into());
    }

    // Setup UEFI properties if secureboot is enabled
    if variant_manifest
        .image_features()
        .iter()
        .flatten()
        .any(|f| *f == ImageFeature::UefiSecureBoot)
    {
        amispec_template.insert("boot-mode".into(), "uefi-preferred".into());
        amispec_template.insert(
            "uefi-data".into(),
            uefi_data.context(MissingUefiDataSnafu)?.into(),
        );
    }

    Ok(amispec_template)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct AmispecRenderContext {
    pub ami: AmiRenderContext,
    pub block_devices: HashMap<String, BlockDeviceRenderContext>,
}

impl AmispecRenderContext {
    /// Create the handlebars render context that is used to render the amispec template
    fn from_ami_args(
        ami_args: &AmiArgs,
        bottlerocket_snapshots: Option<&BottlerocketSnapshots>,
    ) -> Result<Self> {
        let block_device_render_context = bottlerocket_snapshots
            .map(|bottlerocket_snapshots| {
                bottlerocket_snapshots.block_device_render_context(&ami_args.variant_manifest)
            })
            .transpose()?;

        Ok(AmispecRenderContext {
            ami: AmiRenderContext {
                unique_name: ami_args.name.clone(),
                arch: ami_args.arch.clone(),
                description: ami_args.description.clone(),
            },
            block_devices: block_device_render_context.unwrap_or_default(),
        })
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct AmiRenderContext {
    pub unique_name: String,
    pub arch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct BlockDeviceRenderContext {
    pub device_name: String,
    pub snapshot_id: String,
    pub volume_size: u64,
}

impl BottlerocketSnapshots {
    /// Creates the block device handlebars render context from a set of created EBS snapshots
    fn block_device_render_context(
        &self,
        variant_manifest: &VariantManifest,
    ) -> Result<HashMap<String, BlockDeviceRenderContext>> {
        let image_layout =
            variant_manifest
                .image_layout()
                .context(amispec_error::MissingImageLayoutSnafu {
                    variant_manifest: Box::new(variant_manifest.clone()),
                })?;

        let (expects_data_volume, err_msg) = match image_layout.partition_plan {
            PartitionPlan::Split => (true, "expects data volume, but none provided"),
            PartitionPlan::Unified => (false, "does not expect data volume, but one provided"),
        };
        snafu::ensure!(
            expects_data_volume == self.data_snapshot.is_some(),
            amispec_error::ImageLayoutSnafu {
                variant_manifest: Box::new(variant_manifest.clone()),
                message: err_msg,
            }
        );

        let (os_volume_size, data_volume_size) = image_layout.publish_image_sizes_gib();

        let mut render_ctx = HashMap::new();
        render_ctx.insert(
            "root".into(),
            BlockDeviceRenderContext {
                device_name: ROOT_DEVICE_NAME.into(),
                snapshot_id: self.os_snapshot.clone(),
                volume_size: os_volume_size as u64,
            },
        );

        if let Some(data_snapshot) = &self.data_snapshot {
            render_ctx.insert(
                "data".into(),
                BlockDeviceRenderContext {
                    device_name: DATA_DEVICE_NAME.into(),
                    snapshot_id: data_snapshot.clone(),
                    volume_size: data_volume_size as u64,
                },
            );
        }

        Ok(render_ctx)
    }
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub(crate) enum AmispecError {
    #[snafu(display("Failed to create amispec for image layout from {}: {}", variant_manifest.display_path(), message))]
    ImageLayout {
        variant_manifest: Box<VariantManifest>,
        message: String,
    },

    #[snafu(display("Attempted to configure EBS volume with invalid size: {}", source))]
    InvalidEbsVolumeSize { source: InvalidValueError },

    #[snafu(display("Could not find image layout for '{}'", variant_manifest.display_path()))]
    MissingImageLayout {
        variant_manifest: Box<VariantManifest>,
    },

    #[snafu(display(
        "No uefi-data provided. Cannot register 'uefi-secure-boot' without uefi-data."
    ))]
    MissingUefiData,

    #[snafu(display("Failed to parse AMI spec template: {}", source))]
    ParseAmiSpecTemplate { source: toml::de::Error },

    #[snafu(display("Failed to render amispec for RegisterImages call: {}", source))]
    RenderAmiSpec { source: amispec::TemplatedError },
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::aws::ami::register::AmiArgs;
    use crate::aws::ami::VariantManifest;
    use amispec::{
        ebs, Architecture, BlockDeviceMapping, BootMode, ImdsSupport, SriovNetSupport, TpmSupport,
        VirtualizationType,
    };
    use buildsys::manifest::ManifestInfo;
    use maplit::hashmap;
    use std::path::PathBuf;

    use test_case::test_case;

    const ROOT_SNAP_ID: &str = "os-snapshot";
    const DATA_SNAP_ID: &str = "data-snapshot";
    const DEFAULT_ROOT_VOLUME_SIZE: u64 = 2;
    const DEFAULT_DATA_VOLUME_SIZE: u64 = 20;

    lazy_static::lazy_static! {
        static ref SPLIT_LAYOUT_SNAPSHOTS: BottlerocketSnapshots = BottlerocketSnapshots {
            os_snapshot: ROOT_SNAP_ID.into(),
            data_snapshot: Some(DATA_SNAP_ID.into()),
        };

        static ref UNIFIED_LAYOUT_SNAPSHOTS: BottlerocketSnapshots = BottlerocketSnapshots {
            os_snapshot: ROOT_SNAP_ID.into(),
            data_snapshot: None,
        };
    }

    #[test_case(
        AmiArgs {
            os_image: PathBuf::new(),
            data_image: None,
            variant_manifest: VariantManifest::new(
                toml::toml! {
                    [package]
                    name = "test-variant"

                    [package.metadata.build-variant.image-layout]
                    partition-plan = "unified"
                    os-image-size-gib = 100
                }.try_into().unwrap(),
            ),
            uefi_data: Default::default(),
            arch: "x86_64".into(),
            name: "test-ami".into(),
            description: Some("test description".into()),
            no_progress: true,
            regions: Vec::new(),
            amispec_file: None,
            ami_output: None,
        },
        UNIFIED_LAYOUT_SNAPSHOTS.clone(),
        AmiSpec::builder()
            .name("test-ami")
            .architecture(Architecture::X86_64)
            .ena_support(true)
            .description("test description")
            .root_device_name("/dev/xvda")
            .sriov_net_support(SriovNetSupport::Simple)
            .virtualization_type(VirtualizationType::Hvm)
            .block_device_mapping(BlockDeviceMapping::builder()
                .ebs(
                    ebs::Gp2::builder()
                        .snapshot_id(ROOT_SNAP_ID)
                        .volume_size(101).unwrap()
                        .delete_on_termination(true)
                        .build()
                )
                .build()
                .with_device_name("/dev/xvda"))
            .build();
        "default amispec template, no data snapshot"
    )]
    #[test_case(
        AmiArgs {
            os_image: PathBuf::new(),
            data_image: None,
            variant_manifest: VariantManifest::new(
                toml::toml! {
                    [package]
                    name = "test-variant"

                    [package.metadata.build-variant]
                }.try_into().unwrap(),
            ),
            uefi_data: Default::default(),
            arch: "x86_64".into(),
            name: "test-ami".into(),
            description: Some("test description".into()),
            no_progress: true,
            regions: Vec::new(),
            amispec_file: None,
            ami_output: None,
        },
        SPLIT_LAYOUT_SNAPSHOTS.clone(),
        AmiSpec::builder()
            .name("test-ami")
            .architecture(Architecture::X86_64)
            .ena_support(true)
            .description("test description")
            .root_device_name("/dev/xvda")
            .sriov_net_support(SriovNetSupport::Simple)
            .virtualization_type(VirtualizationType::Hvm)
            .block_device_mapping(BlockDeviceMapping::builder()
                .ebs(
                    ebs::Gp2::builder()
                        .snapshot_id(ROOT_SNAP_ID)
                        .volume_size(DEFAULT_ROOT_VOLUME_SIZE).unwrap()
                        .delete_on_termination(true)
                        .build()
                )
                .build()
                .with_device_name("/dev/xvda"))
           .block_device_mapping(BlockDeviceMapping::builder()
                .ebs(
                    ebs::Gp2::builder()
                        .snapshot_id(DATA_SNAP_ID)
                        .volume_size(DEFAULT_DATA_VOLUME_SIZE).unwrap()
                        .delete_on_termination(true)
                        .build()
                )
                .build()
                .with_device_name("/dev/xvdb"))
            .build();
        "default amispec template, with data snapshot"
    )]
    #[test_case(
        AmiArgs {
            os_image: PathBuf::new(),
            data_image: None,
            variant_manifest: VariantManifest::new(
                toml::toml! {
                    [package]
                    name = "test-variant"

                    [package.metadata.build-variant.image-features]
                    uefi-secure-boot = true
                }.try_into().unwrap(),
            ),
            uefi_data: Some("uefi-data!".to_string().into()),
            arch: "x86_64".into(),
            name: "test-ami".into(),
            description: Some("test description".into()),
            no_progress: true,
            regions: Vec::new(),
            amispec_file: Some(toml::toml! {
                name = "hello {{ ami.unique_name }}"
                description = "expand the default {{ ami.description }} with more"

                [block-device-mappings."/dev/xvda".ebs]
                volume-type = "gp3"

                [block-device-mappings."/dev/xvdb".ebs]
                volume-type = "io1"
                volume-size = 100
                iops = 500
            }.into()),
            ami_output: None,
        },
        SPLIT_LAYOUT_SNAPSHOTS.clone(),
        AmiSpec::builder()
            .name("hello test-ami")
            .description("expand the default test description with more")
            .architecture(Architecture::X86_64)
            .ena_support(true)
            .boot_mode(BootMode::UefiPreferred)
            .uefi_data("uefi-data!")
            .root_device_name("/dev/xvda")
            .sriov_net_support(SriovNetSupport::Simple)
            .virtualization_type(VirtualizationType::Hvm)
            .block_device_mapping(BlockDeviceMapping::builder()
                .ebs(
                    ebs::Gp3::builder()
                        .snapshot_id(ROOT_SNAP_ID)
                        .volume_size(DEFAULT_ROOT_VOLUME_SIZE).unwrap()
                        .delete_on_termination(true)
                        .build()
                )
                .build()
                .with_device_name("/dev/xvda"))
           .block_device_mapping(BlockDeviceMapping::builder()
                .ebs(
                    ebs::Io1::builder()
                        .snapshot_id(DATA_SNAP_ID)
                        .volume_size(100).unwrap()
                        .iops(500).unwrap()
                        .delete_on_termination(true)
                        .build()
                )
                .build()
                .with_device_name("/dev/xvdb"))
            .build();
        "override volume type for both volumes, use secure-boot"
    )]
    #[test_case(
        AmiArgs {
            os_image: PathBuf::new(),
            data_image: None,
            variant_manifest: VariantManifest::new(
                toml::toml! {
                    [package]
                    name = "test-variant"

                    [package.metadata.build-variant.image-features]
                    uefi-secure-boot = true

                    [package.metadata.build-variant.image-layout]
                    partition-plan = "unified"
                }.try_into().unwrap(),
            ),
            uefi_data: Some("uefi-data!".to_string().into()),
            arch: "x86_64".into(),
            name: "test-ami".into(),
            description: Some("test description".into()),
            no_progress: true,
            regions: Vec::new(),
            amispec_file: Some(toml::toml! {
                boot-mode = "uefi"
                tpm-support = "v2.0"
                imds-support = "v2.0"
            }.into()),
            ami_output: None,
        },
        UNIFIED_LAYOUT_SNAPSHOTS.clone(),
        AmiSpec::builder()
            .name("test-ami")
            .architecture(Architecture::X86_64)
            .ena_support(true)
            .description("test description")
            .root_device_name("/dev/xvda")
            .sriov_net_support(SriovNetSupport::Simple)
            .virtualization_type(VirtualizationType::Hvm)
            .tpm_support(TpmSupport::V2_0)
            .imds_support(ImdsSupport::V2_0)
            .boot_mode(BootMode::Uefi)
            .uefi_data("uefi-data!")
            .block_device_mapping(BlockDeviceMapping::builder()
                .ebs(
                    ebs::Gp2::builder()
                        .snapshot_id(ROOT_SNAP_ID)
                        .volume_size(22).unwrap()
                        .delete_on_termination(true)
                        .build()
                )
                .build()
                .with_device_name("/dev/xvda"))
            .build();
        "allow boot-mode overrides"
    )]
    fn test_create_amispec(ami_args: AmiArgs, snapshots: BottlerocketSnapshots, expected: AmiSpec) {
        let actual = create_amispec(&ami_args, Some(&snapshots)).unwrap();
        assert_eq!(actual, expected);
    }

    #[test_case(
        AmiSpec::builder()
            .name("test")
            .block_device_mapping(BlockDeviceMapping::builder()
                .ebs(
                    ebs::Gp3::builder()
                        .snapshot_id("snap-12345678")
                        .volume_size(4).unwrap()
                        .build()
                )
                .build()
                .with_device_name("/dev/xvda"))
            .build(),
        hashmap! {
            "friendly-volume-name".into() => BlockDeviceRenderContext {
                device_name: "doesn't matter, only snap ID does".into(),
                snapshot_id: "snap-12345678".into(),
                volume_size: 10
            },
        },
        AmiSpec::builder()
            .name("test")
            .block_device_mapping(BlockDeviceMapping::builder()
                .ebs(
                    ebs::Gp3::builder()
                        .snapshot_id("snap-12345678")
                        .volume_size(10).unwrap()
                        .build()
                )
                .build()
                .with_device_name("/dev/xvda"))

            .build()
        ; "block device mapping smaller than image"
    )]
    #[test_case(
        AmiSpec::builder()
            .name("test-bdm-bigger-than-snapshot")
            .block_device_mapping(BlockDeviceMapping::builder()
                .ebs(
                    ebs::Gp3::builder()
                        .snapshot_id("snapshot!")
                        .volume_size(10).unwrap()
                        .build()
                )
                .build()
                .with_device_name("/dev/xvda"))
            .build(),
        hashmap! {
            "friendly-volume-name".into() => BlockDeviceRenderContext {
                device_name: "doesn't matter, only snap ID does".into(),
                snapshot_id: "snapshot!".into(),
                volume_size: 2
            },
            "unrelated-volume-name".into() => BlockDeviceRenderContext {
                device_name: "doesn't matter, only snap ID does".into(),
                snapshot_id: "different-snapshot!".into(),
                volume_size: 20
            },
        },
        AmiSpec::builder()
            .name("test-bdm-bigger-than-snapshot")
            .block_device_mapping(BlockDeviceMapping::builder()
                .ebs(
                    ebs::Gp3::builder()
                        .snapshot_id("snapshot!")
                        .volume_size(10).unwrap()
                        .build()
                )
                .build()
                .with_device_name("/dev/xvda"))

            .build()
        ; "block device mapping larger than image"
    )]
    #[test_case(
        AmiSpec::builder()
            .name("test-bdm-snapshot-unrelated")
            .block_device_mapping(BlockDeviceMapping::builder()
                .ebs(
                    ebs::Gp3::builder()
                        .snapshot_id("some-random-snapshot")
                        .volume_size(1).unwrap()
                        .build()
                )
                .build()
                .with_device_name("/dev/xvda"))
            .build(),
        hashmap! {
            "friendly-volume-name".into() => BlockDeviceRenderContext {
                device_name: "doesn't matter, only snap ID does".into(),
                snapshot_id: "snapshot!".into(),
                volume_size: 2
            },
            "unrelated-volume-name".into() => BlockDeviceRenderContext {
                device_name: "doesn't matter, only snap ID does".into(),
                snapshot_id: "different-snapshot!".into(),
                volume_size: 20
            },
        },
        AmiSpec::builder()
            .name("test-bdm-snapshot-unrelated")
            .block_device_mapping(BlockDeviceMapping::builder()
                .ebs(
                    ebs::Gp3::builder()
                        .snapshot_id("some-random-snapshot")
                        .volume_size(1).unwrap()
                        .build()
                )
                .build()
                .with_device_name("/dev/xvda"))

            .build()
        ; "snapshots with no hints are ignored"
    )]
    fn test_ensure_amispec_volume_sizes(
        mut amispec: AmiSpec,
        block_devices: HashMap<String, BlockDeviceRenderContext>,
        expected: AmiSpec,
    ) {
        ensure_amispec_volume_sizes(&mut amispec, &block_devices).unwrap();
        assert_eq!(amispec, expected);
    }

    #[test_case(
        toml::toml! {
            [package]
            name = "test-variant"

        }.try_into().unwrap(),
        BottlerocketSnapshots {
            os_snapshot: "os-snapshot".to_string(),
            data_snapshot: Some("data-snapshot".to_string())

        },
        "uefi-data",
        toml::toml! {
            name = "{{ ami.unique_name }}"
            description = "{{ ami.description }}"
            architecture = "{{ ami.arch }}"
            root-device-name = "/dev/xvda"
            sriov-net-support = "simple"
            virtualization-type = VIRT_TYPE
            ena-support = true

            [block-device-mappings."/dev/xvda".ebs]
            volume-type = "gp2"
            volume-size = "{{ block_devices.root.volume_size }}"
            snapshot-id = "{{ block_devices.root.snapshot_id }}"
            delete-on-termination = true

            [block-device-mappings."/dev/xvdb".ebs]
            volume-type = "gp2"
            volume-size = "{{ block_devices.data.volume_size }}"
            snapshot-id = "{{ block_devices.data.snapshot_id }}"
            delete-on-termination = true
        };
        "no secure boot, two snapshots"
    )]
    #[test_case(
        toml::toml! {
            [package]
            name = "test-variant"

            [package.metadata.build-variant.image-features]
            uefi-secure-boot = true

            [package.metadata.build-variant.image-layout]
            partition-plan = "unified"

        }.try_into().unwrap(),
        BottlerocketSnapshots {
            os_snapshot: "os-snapshot".to_string(),
            data_snapshot: None

        },
        "uefi-data",
        toml::toml! {
            name = "{{ ami.unique_name }}"
            description = "{{ ami.description }}"
            architecture = "{{ ami.arch }}"
            root-device-name = "/dev/xvda"
            sriov-net-support = "simple"
            virtualization-type = VIRT_TYPE
            ena-support = true
            uefi-data = "uefi-data"
            boot-mode = "uefi-preferred"

            [block-device-mappings."/dev/xvda".ebs]
            volume-type = "gp2"
            volume-size = "{{ block_devices.root.volume_size }}"
            snapshot-id = "{{ block_devices.root.snapshot_id }}"
            delete-on-termination = true
        };
        "secure boot, one snapshot"
    )]
    fn test_default_amispec_template(
        variant_manifest: ManifestInfo,
        bottlerocket_snapshots: BottlerocketSnapshots,
        uefi_data: &str,
        expected_template: toml::Table,
    ) {
        let actual_template = default_amispec_template(
            &VariantManifest::new(variant_manifest),
            Some(&bottlerocket_snapshots),
            Some(uefi_data),
        )
        .unwrap();

        assert_eq!(actual_template, expected_template);
    }

    #[test_case(
        AmiArgs {
            os_image: PathBuf::new(),
            data_image: None,
            variant_manifest: VariantManifest::new(
                toml::toml! {
                    [package]
                    name = "test-variant"

                    [package.metadata.build-variant.image-layout]
                    os-image-size-gib = 100
                    partition-plan = "unified"
                }.try_into().unwrap(),
            ),
            uefi_data: Default::default(),
            arch: "x86_64".into(),
            name: "test-ami".into(),
            description: Some("test description".into()),
            no_progress: true,
            regions: Vec::new(),
            amispec_file: None,
            ami_output: None,
        },
        BottlerocketSnapshots {
            os_snapshot: "os-snapshot".to_string(),
            data_snapshot: None
        },
        AmispecRenderContext {
            ami: AmiRenderContext {
                unique_name: "test-ami".into(),
                arch: "x86_64".into(),
                description: Some("test description".into()),
            },
            block_devices: hashmap! {
                "root".into() => BlockDeviceRenderContext {
                    device_name: "/dev/xvda".into(),
                    snapshot_id: "os-snapshot".into(),
                    volume_size: 101
                }
            },
        };
        "one snapshot, specified image size"
    )]
    #[test_case(
        AmiArgs {
            os_image: PathBuf::new(),
            data_image: None,
            variant_manifest: VariantManifest::new(
                toml::toml! {
                    [package]
                    name = "test-variant"

                    [package.metadata.build-variant.image-layout]
                    partition-plan = "split"
                }.try_into().unwrap(),
            ),
            uefi_data: Default::default(),
            arch: "aarch64".into(),
            name: "has-data-volume".into(),
            description: Some("render with two snapshots".into()),
            no_progress: true,
            regions: Vec::new(),
            amispec_file: None,
            ami_output: None,
        },
        BottlerocketSnapshots {
            os_snapshot: "os-snapshot".to_string(),
            data_snapshot: Some("data-snapshot".to_string())
        },
        AmispecRenderContext {
            ami: AmiRenderContext {
                unique_name: "has-data-volume".into(),
                arch: "aarch64".into(),
                description: Some("render with two snapshots".into()),
            },
            block_devices: hashmap! {
                "root".into() => BlockDeviceRenderContext {
                    device_name: "/dev/xvda".into(),
                    snapshot_id: "os-snapshot".into(),
                    volume_size: 2,
                },
                "data".into() => BlockDeviceRenderContext {
                    device_name: "/dev/xvdb".into(),
                    snapshot_id: "data-snapshot".into(),
                    volume_size: 20,
                }
            },
        };
        "two snapshots"
    )]
    fn test_create_ami_render_context(
        ami_args: AmiArgs,
        snapshots: BottlerocketSnapshots,
        expected: AmispecRenderContext,
    ) {
        assert_eq!(
            AmispecRenderContext::from_ami_args(&ami_args, Some(&snapshots)).unwrap(),
            expected,
        )
    }
    #[test_case(
        AmiArgs {
            os_image: Default::default(),
            data_image: None,
            variant_manifest: VariantManifest::new(
                toml::toml! {
                    [package]
                    name = "test-variant"

                    [package.metadata.build-variant.image-layout]
                    partition-plan = "split"
                }.try_into().unwrap(),
            ),
            uefi_data: Default::default(),
            arch: "aarch64".into(),
            name: "test-ami".into(),
            description: Some("test description".into()),
            no_progress: true,
            regions: Vec::new(),
            amispec_file: None,
            ami_output: None,
        },
        UNIFIED_LAYOUT_SNAPSHOTS.clone();
        "split layout, one snaphot provided"
    )]
    #[test_case(
        AmiArgs {
            os_image: Default::default(),
            data_image: Default::default(),
            variant_manifest: VariantManifest::new(
                toml::toml! {
                    [package]
                    name = "test-variant"

                    [package.metadata.build-variant.image-layout]
                    partition-plan = "unified"
                }.try_into().unwrap(),
            ),
            uefi_data: Default::default(),
            arch: "aarch64".into(),
            name: "test-ami".into(),
            description: Some("test description".into()),
            no_progress: true,
            regions: Vec::new(),
            amispec_file: None,
            ami_output: None,
        },
        SPLIT_LAYOUT_SNAPSHOTS.clone();
        "unified layout, two snaphots provided"
    )]
    fn test_image_layout_respected(ami_args: AmiArgs, snapshots: BottlerocketSnapshots) {
        let amispec = create_amispec(&ami_args, Some(&snapshots));
        assert!(matches!(amispec, Err(AmispecError::ImageLayout { .. })));
    }
}
