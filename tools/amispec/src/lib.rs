//! This crate provides an interface for specifying properties of an AWS AMI that can be applied
//! upon registration.
//!
//! Example:
//!
//! ```toml
//! name = "my-ami-name"
//! virtualization-type = "hvm"
//!
//! [block-device-mappings."/dev/xvda".ebs]
//! volume-type = "gp3"
//! volume-size= 500
//! ```
//!
//! Using [`TemplatedAmiSpec`], all subfields of the amispec can be specified as handlebars
//! templates, allowing them to be rendered by software later:
//!
//! ```toml
//! name = "{{ ami.name }}"
//! virtualization-type = "{{ ami.virtualization_type }}"
//! ```
//!
//!
//! The following AMI registration properties used for PV or S3-backed AMIs are not supported:
//! * ImageLocation
//! * KernelId
//! * RamdiskId
use aws_sdk_ec2::operation::register_image::RegisterImageInput;
use aws_sdk_ec2::operation::register_image::builders::RegisterImageInputBuilder;
use aws_sdk_ec2::types::BlockDeviceMapping as SdkBlockDeviceMapping;
use bon::Builder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod ebs;

pub use ebs::EbsBlockDevice;
pub use serde_templated::{Template, TemplateOf, Templated, TemplatedError};

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Default, Builder, Templated)]
#[templated(
    derive(Debug, Builder, Clone, Eq, PartialEq),
    forward_attrs(serde, builder)
)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
#[builder(on(_, into))]
pub struct AmiSpec {
    #[templated(
        template_as = "Option<HashMap<Templated<DeviceName>, TemplatedBlockDeviceMapping>>",
        forward_attrs(builder, serde)
    )]
    #[builder(field)]
    #[serde(alias = "block-device-mapping")]
    pub block_device_mappings: Option<HashMap<DeviceName, BlockDeviceMapping>>,

    pub name: String,
    pub architecture: Option<Architecture>,
    pub billing_product: Option<Vec<String>>,
    pub boot_mode: Option<BootMode>,
    pub description: Option<String>,
    pub ena_support: Option<bool>,
    pub imds_support: Option<ImdsSupport>,
    pub root_device_name: Option<String>,
    pub sriov_net_support: Option<SriovNetSupport>,
    pub tags: Option<HashMap<String, String>>,
    pub tpm_support: Option<TpmSupport>,
    pub uefi_data: Option<String>,
    pub virtualization_type: Option<VirtualizationType>,
}

// Allow adding a single BlockDeviceMapping per function call to the builder
impl<S: ami_spec_builder::State> AmiSpecBuilder<S> {
    pub fn block_device_mapping(
        mut self,
        named_block_device_mapping: impl Into<NamedBlockDeviceMapping>,
    ) -> Self {
        let named_block_device_mapping: NamedBlockDeviceMapping = named_block_device_mapping.into();
        let (device_name, bdm) = named_block_device_mapping.split_device_name();
        self.block_device_mappings
            .get_or_insert_with(Default::default)
            .insert(device_name, bdm);
        self
    }

    pub fn maybe_block_device_mapping(
        self,
        named_block_device_mapping: Option<impl Into<NamedBlockDeviceMapping>>,
    ) -> Self {
        if let Some(named_block_device_mapping) = named_block_device_mapping {
            self.block_device_mapping(named_block_device_mapping)
        } else {
            self
        }
    }
}

// Allow adding a single TemplatedBlockDeviceMapping per function call to the builder
impl<S: templated_ami_spec_builder::State> TemplatedAmiSpecBuilder<S> {
    pub fn block_device_mapping(
        mut self,
        named_block_device_mapping: impl Into<TemplatedNamedBlockDeviceMapping>,
    ) -> Self {
        let TemplatedNamedBlockDeviceMapping {
            device_name,
            block_device_mapping,
        } = named_block_device_mapping.into();
        self.block_device_mappings
            .get_or_insert_with(Default::default)
            .insert(device_name, block_device_mapping);
        self
    }

    pub fn maybe_block_device_mapping(
        self,
        named_block_device_mapping: Option<impl Into<TemplatedNamedBlockDeviceMapping>>,
    ) -> Self {
        if let Some(named_block_device_mapping) = named_block_device_mapping {
            self.block_device_mapping(named_block_device_mapping)
        } else {
            self
        }
    }
}

impl AmiSpec {
    pub fn as_register_image_call(&self) -> RegisterImageInputBuilder {
        let AmiSpec {
            block_device_mappings,
            architecture,
            billing_product,
            boot_mode,
            description,
            ena_support,
            imds_support,
            name,
            root_device_name,
            sriov_net_support,
            tags,
            tpm_support,
            uefi_data,
            virtualization_type: _, // Only HVM is supported
        } = self;

        RegisterImageInput::builder()
            .name(name)
            // `amispec` only supports hvm images.
            // `paravirtual` is the RegisterImage default, so we just always set it to `hvm` here.
            .virtualization_type(VirtualizationType::Hvm.to_string())
            .set_architecture(architecture.map(Into::into))
            .set_billing_products(billing_product.clone())
            .set_block_device_mappings(block_device_mappings.as_ref().map(|bdms| {
                bdms.iter()
                    .map(|(device_name, block_device_mapping)| {
                        block_device_mapping.create_sdk_block_device_mapping(device_name)
                    })
                    .collect()
            }))
            .set_boot_mode(boot_mode.map(Into::into))
            .set_description(description.clone())
            .set_ena_support(*ena_support)
            .set_imds_support(imds_support.map(Into::into))
            .set_root_device_name(root_device_name.clone())
            .set_sriov_net_support(sriov_net_support.map(|sriov| sriov.to_string()))
            .set_tpm_support(tpm_support.map(Into::into))
            .set_uefi_data(uefi_data.clone())
            .set_tag_specifications(tags.as_ref().map(|tags| {
                vec![
                    aws_sdk_ec2::types::TagSpecification::builder()
                        .resource_type(aws_sdk_ec2::types::ResourceType::Image)
                        .set_tags(Some(
                            tags.iter()
                                .map(|(k, v)| {
                                    aws_sdk_ec2::types::Tag::builder().key(k).value(v).build()
                                })
                                .collect(),
                        ))
                        .build(),
                ]
            }))
    }
}

/// Possible device names are enumerated on
/// https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/device_naming.html
pub type DeviceName = String;

#[derive(Debug, Copy, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Architecture {
    I386,
    X86_64,
    #[serde(alias = "aarch64")]
    Arm64,
    X86_64Mac,
    #[serde(alias = "aarch64_mac")]
    Arm64Mac,
}
serde_plain::derive_fromstr_from_deserialize!(Architecture);
serde_plain::derive_display_from_serialize!(Architecture);

impl From<Architecture> for aws_sdk_ec2::types::ArchitectureValues {
    fn from(value: Architecture) -> Self {
        match value {
            Architecture::I386 => Self::I386,
            Architecture::X86_64 => Self::X8664,
            Architecture::Arm64 => Self::Arm64,
            Architecture::X86_64Mac => Self::X8664Mac,
            Architecture::Arm64Mac => Self::Arm64Mac,
        }
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum BootMode {
    LegacyBios,
    Uefi,
    UefiPreferred,
}
serde_plain::derive_fromstr_from_deserialize!(BootMode);
serde_plain::derive_display_from_serialize!(BootMode);

impl From<BootMode> for aws_sdk_ec2::types::BootModeValues {
    fn from(value: BootMode) -> Self {
        match value {
            BootMode::LegacyBios => Self::LegacyBios,
            BootMode::Uefi => Self::Uefi,
            BootMode::UefiPreferred => Self::UefiPreferred,
        }
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub enum ImdsSupport {
    #[serde(rename = "v2.0")]
    V2_0,
}
serde_plain::derive_fromstr_from_deserialize!(ImdsSupport);
serde_plain::derive_display_from_serialize!(ImdsSupport);

impl From<ImdsSupport> for aws_sdk_ec2::types::ImdsSupportValues {
    fn from(value: ImdsSupport) -> Self {
        match value {
            ImdsSupport::V2_0 => Self::V20,
        }
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub enum TpmSupport {
    #[serde(rename = "v2.0")]
    V2_0,
}
serde_plain::derive_fromstr_from_deserialize!(TpmSupport);
serde_plain::derive_display_from_serialize!(TpmSupport);

impl From<TpmSupport> for aws_sdk_ec2::types::TpmSupportValues {
    fn from(value: TpmSupport) -> Self {
        match value {
            TpmSupport::V2_0 => Self::V20,
        }
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub enum SriovNetSupport {
    #[serde(rename = "simple")]
    Simple,
}
serde_plain::derive_fromstr_from_deserialize!(SriovNetSupport);
serde_plain::derive_display_from_serialize!(SriovNetSupport);

#[derive(Debug, Copy, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum VirtualizationType {
    Hvm,
    // Paravirtual is not supported
}
serde_plain::derive_fromstr_from_deserialize!(VirtualizationType);
serde_plain::derive_display_from_serialize!(VirtualizationType);

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Default, Builder, Templated)]
#[templated(
    derive(Debug, Builder, Clone, Eq, PartialEq, Default),
    forward_attrs(serde, builder)
)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
#[builder(on(_, into))]
pub struct BlockDeviceMapping {
    #[templated(template_as = "Option<ebs::TemplatedEbsBlockDevice>")]
    pub ebs: Option<EbsBlockDevice>,
    pub no_device: Option<String>,
    pub virtual_name: Option<String>,
}

impl BlockDeviceMapping {
    pub(crate) fn create_sdk_block_device_mapping(
        &self,
        device_name: impl Into<String>,
    ) -> SdkBlockDeviceMapping {
        SdkBlockDeviceMapping::builder()
            .set_ebs(
                self.ebs
                    .as_ref()
                    .map(|block_device| block_device.create_sdk_ebs_block_device()),
            )
            .set_no_device(self.no_device.clone())
            .set_virtual_name(self.virtual_name.clone())
            .device_name(device_name.into())
            .build()
    }

    /// Associates a device name with this block device mapping
    pub fn with_device_name(self, device_name: impl Into<String>) -> NamedBlockDeviceMapping {
        NamedBlockDeviceMapping {
            device_name: device_name.into(),
            block_device_mapping: self,
        }
    }
}

impl<S: block_device_mapping_builder::IsComplete> BlockDeviceMappingBuilder<S> {
    pub fn build_with_device_name(self, device_name: impl Into<String>) -> NamedBlockDeviceMapping {
        NamedBlockDeviceMapping {
            device_name: device_name.into(),
            block_device_mapping: self.build(),
        }
    }
}

impl<S: templated_block_device_mapping_builder::IsComplete> TemplatedBlockDeviceMappingBuilder<S> {
    pub fn build_with_device_name(
        self,
        device_name: impl Into<Templated<String>>,
    ) -> TemplatedNamedBlockDeviceMapping {
        TemplatedNamedBlockDeviceMapping {
            device_name: device_name.into(),
            block_device_mapping: self.build(),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Templated)]
pub struct NamedBlockDeviceMapping {
    device_name: String,
    #[templated(template_as = "TemplatedBlockDeviceMapping")]
    block_device_mapping: BlockDeviceMapping,
}

impl NamedBlockDeviceMapping {
    pub fn split_device_name(self) -> (String, BlockDeviceMapping) {
        (self.device_name, self.block_device_mapping)
    }
}

impl<S: Into<String>, B: Into<BlockDeviceMapping>> From<(S, B)> for NamedBlockDeviceMapping {
    fn from((device_name, block_device_mapping): (S, B)) -> Self {
        Self {
            device_name: device_name.into(),
            block_device_mapping: block_device_mapping.into(),
        }
    }
}

impl<S: Into<Templated<String>>, B: Into<TemplatedBlockDeviceMapping>> From<(S, B)>
    for TemplatedNamedBlockDeviceMapping
{
    fn from((device_name, block_device_mapping): (S, B)) -> Self {
        Self {
            device_name: device_name.into(),
            block_device_mapping: block_device_mapping.into(),
        }
    }
}

#[cfg(test)]
mod test {
    use super::ebs::*;
    use super::*;
    use aws_sdk_ec2::types::{
        ArchitectureValues as SdkArchitectureValues, BootModeValues as SdkBootModeValues,
        EbsBlockDevice as SdkEbsBlockDevice, VolumeType as SdkVolumeType,
    };
    use maplit::hashmap;
    use serde_templated::TemplateOf;
    use test_case::test_case;
    use toml::toml;

    #[test_case(r#"
        name = "my-ami"
        architecture = "x86_64"
        root-device-name = "/dev/xvda"
        boot-mode = "uefi-preferred"
        description = "a very cool ami"
        ena-support = true
        sriov-net-support = "simple"
        virtualization-type = "hvm"

        [block-device-mapping."/dev/xvda".ebs]
        volume-type = "gp3"
        delete-on-termination = true
        snapshot-id ="snap-12345678"
        volume-size = 4
    "#,
    AmiSpec::builder()
        .name("my-ami")
        .architecture(Architecture::X86_64)
        .root_device_name("/dev/xvda")
        .boot_mode(BootMode::UefiPreferred)
        .description("a very cool ami")
        .ena_support(true)
        .sriov_net_support(SriovNetSupport::Simple)
        .virtualization_type(VirtualizationType::Hvm)
        .block_device_mapping(
            BlockDeviceMapping::builder()
                .ebs(
                    Gp3::builder()
                        .snapshot_id("snap-12345678")
                        .delete_on_termination(true)
                        .volume_size(4).unwrap()
                        .build()
                )
                .build()
                .with_device_name("/dev/xvda"))
        .build();
        "parse simple spec"
    )]
    #[test_case(
        r#"name = "minimal""#,
        AmiSpec::builder().name("minimal").build();
        "parse minimal spec"
    )]
    #[test_case(r#"
        name = "multiple-block-device-mappings"
        architecture = "aarch64"
        ena-support = true
        root-device-name = "/dev/xvda"
        imds-support = "v2.0"

        [block-device-mappings."/dev/xvda".ebs]
        volume-type = "gp2"

        [block-device-mappings."/dev/xvdb".ebs]
        volume-type = "gp3"
        volume-size = 100
    "#,
    AmiSpec::builder()
        .name("multiple-block-device-mappings")
        .architecture(Architecture::Arm64)
        .ena_support(true)
        .root_device_name("/dev/xvda")
        .imds_support(ImdsSupport::V2_0)
        .block_device_mapping(
            BlockDeviceMapping::builder()
                .ebs(Gp2::default())
                .build_with_device_name("/dev/xvda"))
        .block_device_mapping(
            BlockDeviceMapping::builder()
                .ebs(
                    Gp3::builder()
                        .volume_size(100).unwrap()
                        .build()
                )
                .build_with_device_name("/dev/xvdb"))
        .build();
        "parse multiple bdms"
    )]
    #[test_case(r#"
        name = "ami-with-tags"
        architecture = "x86_64"

        [tags]
        Environment = "production"
        Team = "platform"
        Version = "1.0"
    "#,
    AmiSpec::builder()
        .name("ami-with-tags")
        .architecture(Architecture::X86_64)
        .tags(hashmap! {
            "Environment".into() => "production".into(),
            "Team".into() => "platform".into(),
            "Version".into() => "1.0".into(),
        })
        .build();
        "parse tags"
    )]
    fn test_parse_ami_spec_toml(toml_str: &str, expected: AmiSpec) {
        let spec: AmiSpec = toml::from_str(&toml_str).unwrap();
        assert_eq!(spec, expected);
    }

    #[test_case(
        toml!(
            name = "my-ami"
            architecture = "x86_64"
            root-device-name = "/dev/xvda"
            boot-mode = "uefi-preferred"
            description = "a very cool ami"
            ena-support = true
            sriov-net-support = "simple"
            virtualization-type = "hvm"

            [block-device-mappings."/dev/xvda".ebs]
            volume-type = "gp3"
            snapshot-id = "snap-12345678"
            delete-on-termination = true
            volume-size = 4
        ),
        HashMap::<String, String>::new(),
        RegisterImageInput::builder()
            .architecture(SdkArchitectureValues::X8664)
            .block_device_mappings(
                SdkBlockDeviceMapping::builder()
                    .device_name("/dev/xvda".to_string())
                    .ebs(
                            SdkEbsBlockDevice::builder()
                                .delete_on_termination(true)
                                .snapshot_id("snap-12345678".to_string())
                                .volume_size(4)
                                .volume_type(SdkVolumeType::Gp3)
                                .build(),
                    )
                    .build()
            )
            .boot_mode(SdkBootModeValues::UefiPreferred)
            .description("a very cool ami".to_string())
            .ena_support(true)
            .name("my-ami".to_string())
            .root_device_name("/dev/xvda".to_string())
            .sriov_net_support("simple".to_string())
            .virtualization_type("hvm".to_string())
            .build()
            .unwrap();
            "simple spec"
    )]
    #[test_case(
        toml!(
            name = "{{ ami.name }}"
            architecture = "{{ variant.arch }}"
        ),
        hashmap! {
            "ami" => hashmap! {
                "name" => "my-ami"
            },
            "variant" => hashmap! {
                "arch" => "x86_64"
            }
        },
        RegisterImageInput::builder()
            .architecture(SdkArchitectureValues::X8664)
            .name("my-ami".to_string())
            .virtualization_type("hvm".to_string())
            .build()
            .unwrap();
        "simple spec with templates"
    )]
    #[test_case(
        toml!(
            name = "{{ name }}"
            virtualization-type = "hvm"

            [block-device-mappings."/dev/xvda".ebs]
            volume-type = "io2"
            snapshot-id = "{{ data_volume_snapshot }}"
            delete-on-termination = true
            iops = 1000
            volume-size = 4
        ),
        hashmap! {
            "name" => "templated-snapshot",
            "data_volume_snapshot" => "snap-12345678",
        },
        RegisterImageInput::builder()
            .name("templated-snapshot".to_string())
            .virtualization_type("hvm".to_string())
            .block_device_mappings(SdkBlockDeviceMapping::builder()
                .device_name("/dev/xvda".to_string())
                .ebs(
                    SdkEbsBlockDevice::builder()
                        .delete_on_termination(true)
                        .iops(1000)
                        .snapshot_id("snap-12345678".to_string())
                        .volume_size(4)
                        .volume_type(SdkVolumeType::Io2)
                        .build(),
                )
                .build()
            )
            .build()
            .unwrap();
        "templated snapshot id"
    )]
    #[test_case(
        toml!(
            name = "ami-with-tags"
            architecture = "x86_64"

            [tags]
            Environment = "production"
            Team = "{{ team_name }}"
        ),
        hashmap! {
            "team_name" => "platform",
        },
        RegisterImageInput::builder()
            .name("ami-with-tags".to_string())
            .architecture(SdkArchitectureValues::X8664)
            .virtualization_type("hvm".to_string())
            .set_tag_specifications(Some(vec![
                aws_sdk_ec2::types::TagSpecification::builder()
                    .resource_type(aws_sdk_ec2::types::ResourceType::Image)
                    .tags(aws_sdk_ec2::types::Tag::builder().key("Environment").value("production").build())
                    .tags(aws_sdk_ec2::types::Tag::builder().key("Team").value("platform").build())
                    .build()
            ]))
            .build()
            .unwrap();
        "tags with templates"
    )]
    fn test_render_templated_amispec_to_sdk(
        templated_amispec: toml::Table,
        template_context: impl Serialize,
        expected: RegisterImageInput,
    ) {
        let templated_amispec: TemplatedAmiSpec = templated_amispec.try_into().unwrap();
        let amispec = templated_amispec.render(&template_context).unwrap();
        let rendered_sdk_request = amispec.as_register_image_call().build().unwrap();
        assert_eq!(rendered_sdk_request, expected);
    }

    #[test_case(r#"
        name = "invalid-block-device"

        [block-device-mappings."/dev/xvda".ebs]
        volume-type = "standard"
        # throughput not supported for standard volumes
        throughput = 200
    "#;
    "throughput not valid for standard"
    )]
    #[test_case(r#"
        name = "too-small-st1"

        [block-device-mappings."/dev/xvda".ebs]
        volume-type = "st1"
        volume-size = 10
    "#;
    "volume-size too small for st1"
    )]
    #[test_case(r#"
        name = "unknown-attribute"
        what = "is this"
    "#;
    "unknown attribute"
    )]
    fn test_invalid_amispec(amispec_toml: &str) {
        let amispec: Result<AmiSpec, toml::de::Error> = toml::from_str(amispec_toml);
        assert!(amispec.is_err());
    }

    #[test_case(r#"
            name = "{{ ami.name }}"
            architecture = "{{ variant.arch }}"
        "#,
        hashmap! {
            "ami" => hashmap! {
                "name" => "my-ami"
            },
            "variant" => hashmap! {
                "arch" => "bad arch"
            }
        };
        "rendered architecture is not valid"
    )]
    #[test_case(r#"
            name = "ami-name"
            architecture = "{{ unset }}"

            [block-device-mappings."/dev/xvda".ebs]
            volume-type = "sc1"
            snapshot-id = "{{ unset_var }}"
        "#,
        HashMap::<String, String>::new();
        "unset template var"
    )]
    fn test_render_failures(templated_amispec_toml: &str, template_context: impl Serialize) {
        let templated_amispec: TemplatedAmiSpec = toml::from_str(templated_amispec_toml).unwrap();
        assert!(templated_amispec.render(&template_context).is_err());
    }
}
