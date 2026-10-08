//! PCR prediction tool for Bottlerocket.
//!
//! Predicts TPM Platform Configuration Register (PCR) values based on
//! boot components, EFI variables, and GPT partition tables.

mod aws;
mod diskfs;
mod efi;
mod error;
mod gpt;
mod pcrs;
mod pe;
mod platform;
mod predict;

use aws_config::profile::ProfileFileCredentialsProvider;
use aws_types::region::Region;
use aws_types::SdkConfig;
use clap::{Parser, Subcommand};
use coldsnap::SnapshotDownloader;
use snafu::prelude::*;
use std::fs;
use std::path::PathBuf;

use crate::error::Result;
use crate::platform::Platform;
use crate::predict::{PcrContext, PcrPredictions};

/// Command-line arguments for pcrsys.
#[derive(Parser)]
#[command(version, about = "Predict TPM PCR values for Bottlerocket")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

/// Subcommands for PCR prediction from different sources.
#[derive(Subcommand)]
enum Command {
    /// Predict PCRs from a local disk image
    Disk {
        /// Path to disk image containing GPT, ESP, and boot partitions
        #[arg(long)]
        image: PathBuf,

        /// Path to efi-vars.json containing Secure Boot variables
        #[arg(long)]
        efi_vars: PathBuf,

        /// Target platform (aws, vmware, metal)
        #[arg(long, value_enum, default_value_t = Platform::Aws)]
        platform: Platform,
    },

    /// Predict PCRs from an AWS AMI
    Ami {
        /// AMI ID (e.g., ami-0123456789abcdef0)
        #[arg(long)]
        ami_id: String,

        /// AWS region to use
        #[arg(long)]
        region: Option<String>,

        /// AWS profile to use
        #[arg(long)]
        profile: Option<String>,
    },
}

#[snafu::report]
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    match &args.command {
        Command::Disk {
            image,
            efi_vars,
            platform,
        } => run_disk(image, efi_vars, *platform),
        Command::Ami {
            ami_id,
            region,
            profile,
        } => run_ami(ami_id, region.as_ref(), profile.as_ref()).await,
    }
}

/// Run PCR prediction from a local disk image file.
fn run_disk(image: &PathBuf, efi_vars_path: &PathBuf, platform: Platform) -> Result<()> {
    let efi_vars_json = fs::read_to_string(efi_vars_path).whatever_context(format!(
        "failed to read efi-vars: {}",
        efi_vars_path.display()
    ))?;

    let mut disk = fs::File::open(image)
        .whatever_context(format!("failed to open disk image: {}", image.display()))?;

    let efi_vars: efi::EfiVars =
        serde_json::from_str(&efi_vars_json).whatever_context("failed to parse efi-vars.json")?;

    let predictions = predict_pcrs(&efi_vars, &mut disk, platform)?;
    let json = serde_json::to_string_pretty(&predictions)
        .whatever_context("failed to serialize predictions")?;
    println!("{json}");

    Ok(())
}

/// Run PCR prediction from an AWS AMI by downloading its snapshot.
async fn run_ami(ami_id: &str, region: Option<&String>, profile: Option<&String>) -> Result<()> {
    let config = build_client_config(region, profile).await;

    let ec2_client = aws_sdk_ec2::Client::new(&config);
    let ebs_client = aws_sdk_ebs::Client::new(&config);

    let efi_vars = aws::ami::get_uefi_data(&ec2_client, ami_id).await?;

    let snapshot_id = aws::ami::get_root_snapshot_id(&ec2_client, ami_id).await?;

    let temp_file =
        tempfile::NamedTempFile::new().whatever_context("failed to create temp file")?;

    let downloader = SnapshotDownloader::new(ebs_client);
    downloader
        .download_to_file(&snapshot_id, temp_file.path(), None, None, None)
        .await
        .whatever_context("failed to download snapshot")?;

    let mut disk =
        fs::File::open(temp_file.path()).whatever_context("failed to open downloaded snapshot")?;

    let predictions = predict_pcrs(&efi_vars, &mut disk, Platform::Aws)?;
    let json = serde_json::to_string_pretty(&predictions)
        .whatever_context("failed to serialize predictions")?;
    println!("{json}");

    Ok(())
}

/// Build AWS SDK config, handling region and profile options like coldsnap.
async fn build_client_config(region: Option<&String>, profile: Option<&String>) -> SdkConfig {
    let config = match (region, profile) {
        (Some(r), _) => aws_config::from_env().region(Region::new(r.clone())),
        (None, Some(p)) => aws_config::from_env().region(
            aws_config::profile::ProfileFileRegionProvider::builder()
                .profile_name(p)
                .build(),
        ),
        (None, None) => aws_config::from_env(),
    };

    let config = match profile {
        Some(p) => config.credentials_provider(
            ProfileFileCredentialsProvider::builder()
                .profile_name(p)
                .build(),
        ),
        None => config,
    };

    config.load().await
}

/// Run PCR prediction using EFI variables and a disk image file.
fn predict_pcrs(
    efi_vars: &efi::EfiVars,
    disk: &mut fs::File,
    platform: Platform,
) -> Result<PcrPredictions> {
    let gpt_bin = gpt::extract_primary_gpt(disk)?;
    let esp = gpt::find_esp(disk)?;
    let (fallback, machine, sidecars) = diskfs::extract_fallback(disk, &esp)?;
    let uki = pe::UkiImage::parse(&fallback, machine)?;
    let (partitions, grub, vmlinuz, grub_cfg, bootconfig, boot_partuuid) = if uki.is_some() {
        ensure_whatever!(
            platform == Platform::Aws,
            "direct UKI prediction supports AWS only"
        );
        ensure_whatever!(
            !sidecars,
            "UKI ESP contains unsupported credentials, extensions, or addons"
        );
        gpt::validate_uki_layout(disk)?;
        (
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            String::new(),
        )
    } else {
        let partitions = gpt::find_partitions(disk)?;
        let grub = diskfs::extract_grub(disk, &partitions)?;
        let vmlinuz = diskfs::extract_vmlinuz(disk, &partitions)?;
        let grub_cfg = diskfs::extract_grub_cfg(disk, &partitions)?;
        let bootconfig = diskfs::extract_bootconfig(disk, &partitions)?;
        let boot_partuuid = gpt::get_boot_partuuid(disk)?;
        (
            Some(partitions),
            grub,
            vmlinuz,
            grub_cfg,
            bootconfig,
            boot_partuuid,
        )
    };

    let ctx = PcrContext::builder()
        .platform(platform)
        .efi_vars(efi_vars)
        .maybe_partitions(partitions.as_ref())
        .maybe_uki(uki.as_ref())
        .gpt_bin(&gpt_bin)
        .shim(&fallback)
        .grub(&grub)
        .vmlinuz(&vmlinuz)
        .grub_cfg(&grub_cfg)
        .bootconfig(&bootconfig)
        .boot_partuuid(&boot_partuuid)
        .build();

    PcrPredictions::new()
        .try_extend(|| pcrs::pcr0::predict(&ctx))?
        .try_extend(|| pcrs::pcr1::predict(&ctx))?
        .try_extend(|| pcrs::pcr2::predict(&ctx))?
        .try_extend(|| pcrs::pcr3::predict(&ctx))?
        .try_extend(|| pcrs::pcr4::predict(&ctx))?
        .try_extend(|| pcrs::pcr5::predict(&ctx))?
        .try_extend(|| pcrs::pcr6::predict(&ctx))?
        .try_extend(|| pcrs::pcr7::predict(&ctx))?
        .try_extend(|| pcrs::pcr9::predict(&ctx))?
        .try_extend(|| pcrs::pcr10::predict(&ctx))?
        .try_extend(|| pcrs::pcr11::predict(&ctx))?
        .try_extend(|| pcrs::pcr12::predict(&ctx))?
        .try_extend(|| pcrs::pcr13::predict(&ctx))?
        .try_extend(|| pcrs::pcr14::predict(&ctx))?
        .try_extend(|| pcrs::pcr15::predict(&ctx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Seek, SeekFrom, Write};
    use test_case::test_case;

    fn uki_disk(
        fallback: &str,
        image: &[u8],
        sidecar: bool,
        ambiguous: bool,
    ) -> tempfile::NamedTempFile {
        let mut disk = tempfile::NamedTempFile::new().unwrap();
        disk.as_file_mut().set_len(10 * 1024 * 1024).unwrap();
        let mut gpt = gptman::GPT::new_from(disk.as_file_mut(), 512, [1; 16]).unwrap();
        gpt[1] = gptman::GPTPartitionEntry {
            partition_type_guid: uuid::Uuid::parse_str("c12a7328-f81f-11d2-ba4b-00a0c93ec93b")
                .unwrap()
                .to_bytes_le(),
            unique_partition_guid: [2; 16],
            starting_lba: 2048,
            ending_lba: 18431,
            attribute_bits: 0,
            partition_name: "EFI-A".into(),
        };
        gpt.write_into(disk.as_file_mut()).unwrap();
        let mut esp = Cursor::new(vec![0; 8 * 1024 * 1024]);
        fatfs::format_volume(&mut esp, fatfs::FormatVolumeOptions::new()).unwrap();
        esp.set_position(0);
        {
            let fs = fatfs::FileSystem::new(&mut esp, fatfs::FsOptions::new()).unwrap();
            let efi = fs.root_dir().create_dir("EFI").unwrap();
            let boot = efi.create_dir("BOOT").unwrap();
            boot.create_file(fallback)
                .unwrap()
                .write_all(image)
                .unwrap();
            if sidecar {
                boot.create_dir("bottlerocket.efi.extra.d")
                    .unwrap()
                    .create_file("input.cred")
                    .unwrap()
                    .write_all(b"external")
                    .unwrap();
            }
            if ambiguous {
                boot.create_file("bootaa64.efi")
                    .unwrap()
                    .write_all(image)
                    .unwrap();
            }
        }
        disk.as_file_mut()
            .seek(SeekFrom::Start(1024 * 1024))
            .unwrap();
        disk.as_file_mut().write_all(esp.get_ref()).unwrap();
        disk
    }

    #[test_case("bootx64.efi", 0x8664)]
    #[test_case("bootaa64.efi", 0xaa64)]
    fn uki_prediction_needs_only_esp_not_grub_or_private(fallback: &str, machine: u16) {
        // Given: a direct-UKI disk without GRUB or a private partition.
        let image = pe::tests::build_test_uki(machine);
        let mut disk = uki_disk(fallback, &image, false, false);
        assert!(gpt::find_partitions(disk.as_file_mut()).is_err());
        // When: predicting the disk through the normal route selection.
        let predictions =
            predict_pcrs(&pe::tests::efi_vars(), disk.as_file_mut(), Platform::Aws).unwrap();
        // Then: emit the complete supported PCR set, including zero PCR14.
        use predict::PcrIndex::*;
        assert_eq!(
            predictions.pcrs.keys().copied().collect::<Vec<_>>(),
            [
                Pcr0, Pcr2, Pcr3, Pcr4, Pcr5, Pcr6, Pcr7, Pcr9, Pcr10, Pcr11, Pcr12, Pcr13, Pcr14,
                Pcr15,
            ]
        );
        // Firmware and unused registers follow the same path as GRUB. These values
        // were measured on Mantle, NVIDIA FIPS and ECS instances, not read from the UKI.
        assert_eq!(
            predictions.pcrs[&Pcr0].sha256,
            ["737f767a12f54e70eecbc8684011323ae2fe2dd9f90785577969d7a2013e8c12"]
        );
        for index in [Pcr2, Pcr3, Pcr6] {
            assert_eq!(
                predictions.pcrs[&index].sha256,
                ["3d458cfe55cc03ea1f443f1562beec8df51c75e14a9fcf9a7234a13f198e7969"]
            );
        }
        for index in [Pcr10, Pcr14, Pcr15] {
            assert_eq!(predictions.pcrs[&index].sha256, [hex::encode([0; 32])]);
        }
    }

    #[test_case(true, false; "external_credential")]
    #[test_case(false, true; "ambiguous_architectures")]
    fn reject_unmodeled_esp_inputs(sidecar: bool, ambiguous: bool) {
        let mut disk = uki_disk(
            "bootx64.efi",
            &pe::tests::build_test_uki(0x8664),
            sidecar,
            ambiguous,
        );
        assert!(predict_pcrs(&pe::tests::efi_vars(), disk.as_file_mut(), Platform::Aws).is_err());
    }

    #[test]
    fn damaged_primary_gpt_is_not_predicted_using_backup_partition_discovery() {
        let mut disk = uki_disk(
            "bootx64.efi",
            &pe::tests::build_test_uki(0x8664),
            false,
            false,
        );
        disk.as_file_mut().seek(SeekFrom::Start(512)).unwrap();
        disk.as_file_mut().write_all(b"BROKEN!!").unwrap();
        assert!(predict_pcrs(&pe::tests::efi_vars(), disk.as_file_mut(), Platform::Aws).is_err());
    }
    #[test]
    fn file_alignment_padding_does_not_enter_pcr9_or_pcr11() {
        let mut image = pe::tests::build_test_uki(0x8664);
        // The command line's payload is shorter than its 512-byte raw section.
        image[1500] = 0xaa;
        let original_image = pe::tests::build_test_uki(0x8664);
        let original = pe::UkiImage::parse(&original_image, 0x8664)
            .unwrap()
            .unwrap();
        let changed = pe::UkiImage::parse(&image, 0x8664).unwrap().unwrap();
        let vars = pe::tests::efi_vars();
        let original_ctx = PcrContext::builder()
            .platform(Platform::Aws)
            .efi_vars(&vars)
            .uki(&original)
            .build();
        let changed_ctx = PcrContext::builder()
            .platform(Platform::Aws)
            .efi_vars(&vars)
            .uki(&changed)
            .build();
        assert_eq!(
            pcrs::pcr9::predict(&original_ctx)
                .unwrap()
                .unwrap()
                .1
                .sha256,
            pcrs::pcr9::predict(&changed_ctx).unwrap().unwrap().1.sha256
        );
        assert_eq!(
            pcrs::pcr11::predict(&original_ctx)
                .unwrap()
                .unwrap()
                .1
                .sha256,
            pcrs::pcr11::predict(&changed_ctx)
                .unwrap()
                .unwrap()
                .1
                .sha256
        );
        assert_ne!(
            pcrs::pcr4::predict(&original_ctx)
                .unwrap()
                .unwrap()
                .1
                .sha256,
            pcrs::pcr4::predict(&changed_ctx).unwrap().unwrap().1.sha256
        );
    }
}
