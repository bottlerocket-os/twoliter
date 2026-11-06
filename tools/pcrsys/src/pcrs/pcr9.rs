//! PCR 9: Kernel Command Line and UKI initrd
//!
//! PCR 9 measures the kernel command line, which for Bottlerocket consists of:
//! 1. Static parameters from grub.cfg
//! 2. Dynamic parameters from bootconfig.data (kernel.* and init.* sections)
//!
//! The final /proc/cmdline format is:
//! `<kernel.* params> BOOT_IMAGE=<path> <grub params before --> -- <init.* params> <grub params after -->`

use crate::error::Result;
use crate::image::BootChain;
use crate::pe::Arch;
use crate::predict::{
    extend_pcr_data, extend_pcr_string, PcrContext, PcrIndex, PcrRecord, PCR_INIT_VAL,
};
use snafu::ResultExt;
use std::fmt::Write;

// systemd-stub writes the trailer's namesize with an uppercase B; these are its exact bytes.
const CPIO_TRAILER: &[u8] = b"07070100000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000B00000000TRAILER!!!\0\0\0\0";

/// Predict PCR 9 value.
///
/// GRUB images extend the /proc/cmdline contents, newline included, after the aarch64 LoadOptions.
/// UKI extends LoadOptions and the stub-generated os-release initrd.
pub fn predict(ctx: &PcrContext) -> Result<Option<(PcrIndex, PcrRecord)>> {
    let pcr9 = match &ctx.image.boot {
        BootChain::Uki(uki) => {
            let pcr = extend_pcr_data(&PCR_INIT_VAL, &uki.cmdline().load_options());
            extend_pcr_data(&pcr, &stub_generated_uki_initrd(uki.osrel())?)
        }
        BootChain::Grub(grub) if grub.partitions.dual_bank() => return Ok(None),
        BootChain::Grub(grub) => {
            let pcr = match grub.vmlinuz.arch() {
                Arch::Aarch64 => extend_pcr_data(&PCR_INIT_VAL, &grub.cmdline.load_options()),
                Arch::X86_64 => PCR_INIT_VAL,
            };
            let proc_cmdline = grub.cmdline.proc_cmdline(&grub.bootconfig);
            extend_pcr_string(&pcr, &format!("{proc_cmdline}\n"))
        }
    };
    Ok(Some((PcrIndex::Pcr9, PcrRecord::new(pcr9))))
}

/// The initrd systemd v257.13 builds with `pack_cpio_literal(".extra", "os-release", 0555, 0444)`.
fn stub_generated_uki_initrd(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = cpio_entry(".extra", 1, 0o40555, &[])?;
    out.extend(cpio_entry(".extra/os-release", 2, 0o100444, data)?);
    out.extend_from_slice(CPIO_TRAILER);
    Ok(out)
}

/// Encode one newc CPIO entry: header and NUL-terminated name, then data, each 4-byte aligned.
fn cpio_entry(name: &str, inode: u32, mode: u32, data: &[u8]) -> Result<Vec<u8>> {
    let size = u32::try_from(data.len()).whatever_context("CPIO payload too large")?;
    // namesize counts the terminating NUL.
    let namesize = u32::try_from(name.len() + 1).whatever_context("CPIO name too long")?;

    // inode, mode, uid, gid, nlink, mtime, filesize, devmajor, devminor, rdevmajor, rdevminor,
    // namesize, check
    let mut header = String::from("070701");
    for word in [inode, mode, 0, 0, 1, 0, size, 0, 0, 0, 0, namesize, 0] {
        write!(&mut header, "{word:08x}").whatever_context("failed to format CPIO header")?;
    }
    let mut out = header.into_bytes();
    out.extend(name.as_bytes());
    out.push(0);
    out.resize(out.len().next_multiple_of(4), 0);
    out.extend(data);
    out.resize(out.len().next_multiple_of(4), 0);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::Grub;
    use crate::pe::PeImage;
    use crate::platform::Platform;
    use crate::predict::test_support::{build_test_pe, mock_grub, mock_uki, MockCtx};
    use bootconfig::{Bootconfig, GrubCmdline};

    fn encoded_bootconfig_data(bootconfig: &str) -> Vec<u8> {
        let mut data = bootconfig.as_bytes().to_vec();
        data.push(0);
        data.resize(data.len().next_multiple_of(4), 0);
        let size = u32::try_from(data.len()).expect("bootconfig fits in u32");
        let checksum = data.iter().map(|&byte| u32::from(byte)).sum::<u32>();
        data.extend(size.to_le_bytes());
        data.extend(checksum.to_le_bytes());
        data.extend(b"#BOOTCONFIG\n");
        data
    }

    fn predict_grub(machine: u16, grub_cfg: &[u8], bootconfig: &str) -> PcrRecord {
        let vmlinuz = build_test_pe(machine, &[(".text", b"kernel")]);
        let grub_cmdline = GrubCmdline::parse(grub_cfg, "").unwrap();
        let bootconfig = Bootconfig::decode(&encoded_bootconfig_data(bootconfig)).unwrap();
        let m = MockCtx::with_boot(BootChain::Grub(Box::new(Grub {
            vmlinuz: PeImage::parse(&vmlinuz).unwrap(),
            cmdline: grub_cmdline,
            bootconfig,
            ..mock_grub()
        })));
        predict(&m.build(Platform::Aws)).unwrap().unwrap().1
    }

    #[test]
    fn test_predict_includes_trailing_newline() {
        let record = predict_grub(0x8664, b"linux ($root)/vmlinuz -- x", "");
        let expected = extend_pcr_string(&PCR_INIT_VAL, "BOOT_IMAGE=/vmlinuz -- x\n");
        assert_eq!(record, PcrRecord::new(expected));
    }

    #[test]
    fn aarch64_first_measures_the_load_options_grub_sets() {
        let record = predict_grub(
            0xaa64,
            b"linux ($root)/vmlinuz quiet -- x",
            "kernel.A = 1\n",
        );
        let load_options: Vec<u8> = "BOOT_IMAGE=/vmlinuz quiet -- x"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let expected = extend_pcr_data(&PCR_INIT_VAL, &load_options);
        let expected = extend_pcr_string(&expected, "A=1 BOOT_IMAGE=/vmlinuz quiet -- x\n");
        assert_eq!(record, PcrRecord::new(expected));
    }

    #[test]
    fn a_uki_measures_its_load_options_then_the_os_release_initrd() {
        let uki = mock_uki(0x8664);
        // The fixture's command line is "quiet", in UTF-16LE with a terminating NUL.
        let expected = extend_pcr_data(&PCR_INIT_VAL, b"q\0u\0i\0e\0t\0\0\0");
        let expected = extend_pcr_data(&expected, &stub_generated_uki_initrd(uki.osrel()).unwrap());
        let m = MockCtx::with_boot(BootChain::Uki(uki));
        let record = predict(&m.build(Platform::Aws)).unwrap().unwrap().1;
        assert_eq!(record, PcrRecord::new(expected));
    }

    #[test]
    fn test_predict_skipped_for_ab() {
        let m = MockCtx::dual_bank();
        assert!(predict(&m.build(Platform::Aws)).unwrap().is_none());
    }
}
