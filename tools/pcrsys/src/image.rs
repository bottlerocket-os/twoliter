//! A Bottlerocket disk image: its partition table and the chain the firmware boots.

use bootconfig::{Bootconfig, GrubCmdline};
use ext4_view::Ext4;
use snafu::{ensure_whatever, ResultExt};
use std::io::{Read, Seek};

use crate::diskfs::{self, Esp};
use crate::efi::EfiVars;
use crate::error::Result;
use crate::gpt::PartitionTable;
use crate::pcrs;
use crate::pe::{PeImage, Shim, Uki};
use crate::platform::Platform;
use crate::predict::{PcrContext, PcrPredictions};

#[derive(Debug)]
pub struct DiskImage {
    pub gpt: PartitionTable,
    pub boot: BootChain,
}

#[derive(Debug)]
pub enum BootChain {
    /// Firmware runs the UKI from the ESP fallback path directly.
    Uki(Uki),
    /// Firmware runs shim, which chain-loads GRUB, which loads vmlinuz.
    Grub(Box<Grub>),
}

#[derive(Debug)]
pub struct Grub {
    pub shim: Shim,
    pub grub: PeImage,
    pub vmlinuz: PeImage,
    pub cmdline: GrubCmdline,
    pub bootconfig: Bootconfig,
    pub partitions: BootPartitions,
}

/// Partitions whose gptprio attribute bits change at runtime (PCR 5).
#[derive(Debug)]
pub struct BootPartitions {
    pub boot_a: u32,
    pub boot_b: Option<u32>,
    pub private: u32,
}

impl BootPartitions {
    /// Updates rewrite the BOOT-A/B files, so PCR 4 and 9 cannot be predicted.
    pub fn dual_bank(&self) -> bool {
        self.boot_b.is_some()
    }
}

impl DiskImage {
    /// Read the partition table and every file the boot chain measures.
    pub fn parse<R: Read + Seek>(disk: &mut R) -> Result<Self> {
        let gpt = PartitionTable::read_from(disk)?;
        let esp = diskfs::mount_esp(disk, gpt.byte_range(gpt.esp()?)?)?;
        let fallback = PeImage::parse(&diskfs::read_fallback(&esp)?)
            .whatever_context("invalid firmware fallback executable")?;

        let boot = match Uki::from_pe(fallback)? {
            Ok(uki) => BootChain::Uki(uki),
            Err(pe) => {
                let shim = Shim::from_pe(pe)?;
                BootChain::Grub(Box::new(Grub::parse(disk, &gpt, &esp, shim)?))
            }
        };
        Ok(Self { gpt, boot })
    }

    pub fn predict(&self, efi_vars: &EfiVars, platform: Platform) -> Result<PcrPredictions> {
        ensure_whatever!(
            matches!(self.boot, BootChain::Grub(_)) || platform == Platform::Aws,
            "direct UKI prediction supports AWS only"
        );
        let ctx = PcrContext {
            platform,
            efi_vars,
            image: self,
        };

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
}

impl Grub {
    fn parse<R: Read + Seek>(
        disk: &mut R,
        gpt: &PartitionTable,
        esp: &Esp,
        shim: Shim,
    ) -> Result<Self> {
        let (boot_a, boot_b) = gpt.boot_partitions()?;
        let grub = diskfs::read_grub(esp)?;
        let partitions = BootPartitions {
            boot_a,
            boot_b,
            private: gpt.private()?,
        };
        let boot = diskfs::mount_ext4(disk, gpt.byte_range(partitions.boot_a)?, "BOOT-A")?;
        let vmlinuz = read(&boot, "/vmlinuz")?;
        let grub_cfg = read(&boot, "/grub/grub.cfg")?;

        let private = diskfs::mount_ext4(disk, gpt.byte_range(partitions.private)?, "PRIVATE")?;
        let boot_partuuid = gpt.boot_partuuid()?.to_string();
        let grub_cmdline =
            GrubCmdline::parse(&grub_cfg, &boot_partuuid).whatever_context("invalid grub.cfg")?;

        let boot_cfg = read(&private, "/bootconfig.data")?;
        let bootconfig =
            Bootconfig::decode(&boot_cfg).whatever_context("invalid bootconfig.data")?;

        Ok(Self {
            shim,
            grub: PeImage::parse(&grub).whatever_context("invalid grub")?,
            vmlinuz: PeImage::parse(&vmlinuz).whatever_context("invalid vmlinuz")?,
            cmdline: grub_cmdline,
            bootconfig,
            partitions,
        })
    }
}

fn read(fs: &Ext4, path: &str) -> Result<Vec<u8>> {
    fs.read(path)
        .with_whatever_context(|_| format!("failed to read {path}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::predict::test_support::mock_gpt;
    use std::io::Cursor;

    #[test]
    fn test_parse_missing_efi() {
        let mut disk = Cursor::new(vec![0u8; 128 * 512]);
        mock_gpt(2).write_into(&mut disk).unwrap();
        let err = DiskImage::parse(&mut disk).unwrap_err();
        assert!(err.to_string().contains("EFI-A"));
    }

    #[test]
    fn test_parse_invalid_gpt() {
        let err = DiskImage::parse(&mut Cursor::new(vec![0u8; 512 * 68])).unwrap_err();
        assert!(err.to_string().contains("GPT"));
    }
}
