//! PCR 9: Kernel Command Line and UKI initrd
//!
//! PCR 9 measures the kernel command line, which for Bottlerocket consists of:
//! 1. Static parameters from grub.cfg
//! 2. Dynamic parameters from bootconfig.data (kernel.* and init.* sections)
//!
//! The final /proc/cmdline format is:
//! `<kernel.* params> BOOT_IMAGE=<path> <grub params before --> -- <init.* params> <grub params after -->`
//!
//! Direct UKI boot instead measures EFI LoadOptions followed by the loaded initrd.

use crate::error::Result;
use crate::predict::{
    extend_pcr_data, extend_pcr_string, PcrContext, PcrIndex, PcrRecord, PCR_INIT_VAL,
};
use bootconfig::predict_grub_cmdline as predict_cmdline;
use snafu::{OptionExt, ResultExt};
use std::fmt::Write;

// The stub uses uppercase B in the trailer's namesize field. Preserve the exact
// bytes because the archive is measured into PCR 9.
const CPIO_TRAILER: &[u8] = b"07070100000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000B00000000TRAILER!!!\0\0\0\0";

/// Predict PCR 9 value.
///
/// GRUB extends the command line plus a newline matching /proc/cmdline.
/// UKI extends LoadOptions and the stub-generated os-release initrd.
pub fn predict(ctx: &PcrContext) -> Result<Option<(PcrIndex, PcrRecord)>> {
    if let Some(uki) = ctx.uki {
        // Linux EFI stub measures LoadOptions, then the complete loaded initrd.
        // The supported stub supplies only the generated os-release CPIO archive.
        let options = bootconfig::uki_load_options(uki.cmdline()?)?;
        let pcr = extend_pcr_data(&PCR_INIT_VAL, &options);
        let pcr = extend_pcr_data(&pcr, &osrel_archive(uki.section(".osrel"))?);
        return Ok(Some((PcrIndex::Pcr9, PcrRecord::new(pcr))));
    }
    if ctx
        .partitions
        .whatever_context("GRUB partition layout missing")?
        .boot_b
        .is_some()
    {
        return Ok(None);
    }

    let mut cmdline = predict_cmdline(ctx.grub_cfg, ctx.bootconfig, Some(ctx.boot_partuuid))?;
    cmdline.push('\n');
    let pcr9 = extend_pcr_string(&PCR_INIT_VAL, &cmdline);
    Ok(Some((PcrIndex::Pcr9, PcrRecord::new(pcr9))))
}

// systemd v257.13 pack_cpio_literal(".extra", "os-release", 0555, 0444).
fn osrel_archive(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    cpio_entry(&mut out, ".extra", 1, 0o40555, &[])?;
    cpio_entry(&mut out, ".extra/os-release", 2, 0o100444, data)?;
    out.extend_from_slice(CPIO_TRAILER);
    Ok(out)
}

fn cpio_entry(out: &mut Vec<u8>, name: &str, inode: u32, mode: u32, data: &[u8]) -> Result<()> {
    let size = u32::try_from(data.len()).whatever_context("CPIO payload too large")?;
    let namesize = u32::try_from(name.len() + 1).whatever_context("CPIO name too long")?;
    let mut header = String::from("070701");
    for word in [inode, mode, 0, 0, 1, 0, size, 0, 0, 0, 0, namesize, 0] {
        write!(&mut header, "{word:08x}").whatever_context("failed to format CPIO header")?;
    }
    out.extend(header.as_bytes());
    out.extend(name.as_bytes());
    out.push(0);
    out.resize(out.len().next_multiple_of(4), 0);
    out.extend(data);
    out.resize(out.len().next_multiple_of(4), 0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    fn make_bootconfig(text: &str) -> Vec<u8> {
        let mut data = text.as_bytes().to_vec();
        data.push(0);
        data.resize(data.len().next_multiple_of(4), 0);
        let size = data.len() as u32;
        let checksum: u32 = data.iter().map(|&b| u32::from(b)).sum();
        data.extend(size.to_le_bytes());
        data.extend(checksum.to_le_bytes());
        data.extend(b"#BOOTCONFIG\n");
        data
    }

    #[test_case(
        b"linux ($root)/vmlinuz console=tty0 -- systemd.log_target=journal",
        "kernel.FOO = bar\n",
        "FOO=bar BOOT_IMAGE=/vmlinuz console=tty0 -- systemd.log_target=journal"
        ; "kernel_param_before_boot_image"
    )]
    #[test_case(
        b"linux ($root)/vmlinuz quiet -- init=/sbin/init",
        "init.BAZ = qux\n",
        "BOOT_IMAGE=/vmlinuz quiet -- BAZ=qux init=/sbin/init"
        ; "init_param_after_separator"
    )]
    #[test_case(
        b"linux ($root)/vmlinuz console=tty0 -- systemd.log_color=0",
        "kernel.A = 1\ninit.B = 2\n",
        "A=1 BOOT_IMAGE=/vmlinuz console=tty0 -- B=2 systemd.log_color=0"
        ; "both_kernel_and_init_params"
    )]
    #[test_case(
        b"linux ($root)/vmlinuz quiet -- x",
        "",
        "BOOT_IMAGE=/vmlinuz quiet -- x"
        ; "empty_bootconfig"
    )]
    #[test_case(
        b"linux ($root)/vmlinuz -- x",
        "kernel.A = 1\n",
        "A=1 BOOT_IMAGE=/vmlinuz -- x"
        ; "kernel_only_no_init_bootconfig"
    )]
    #[test_case(
        b"linux ($root)/vmlinuz foo -- bar",
        "init.X = y\n",
        "BOOT_IMAGE=/vmlinuz foo -- X=y bar"
        ; "init_only_no_kernel_bootconfig"
    )]
    #[test_case(
        b"linux ($root)/vmlinuz dm-mod.create=\"root verity\" -- x",
        "",
        r#"BOOT_IMAGE=/vmlinuz "dm-mod.create=root verity" -- x"#
        ; "grub_quoted_value_gets_repaired"
    )]
    #[test_case(
        b"linux ($root)/vmlinuz -- x",
        "kernel.MSG = \"hello world\"\n",
        r#"MSG="hello world" BOOT_IMAGE=/vmlinuz -- x"#
        ; "bootconfig_quoted_value_not_repaired"
    )]
    #[test_case(
        b"linux ($root)/vmlinuz a=1 b=2 -- c=3 d=4",
        "kernel.K = v\ninit.I = w\n",
        "K=v BOOT_IMAGE=/vmlinuz a=1 b=2 -- I=w c=3 d=4"
        ; "multiple_grub_args_both_sides"
    )]
    #[test_case(
        b"linux ($root)/vmlinuz PARTUUID=/PARTNROFF=1 PARTUUID=/PARTNROFF=2 -- x",
        "",
        "BOOT_IMAGE=/vmlinuz PARTUUID=/PARTNROFF=1 PARTUUID=/PARTNROFF=2 -- x"
        ; "multiple_partuuid_without_substitution"
    )]
    fn test_predict_cmdline(grub_cfg: &[u8], bootconfig_text: &str, expected: &str) {
        let bootconfig = make_bootconfig(bootconfig_text);
        let cmdline = predict_cmdline(grub_cfg, &bootconfig, None).unwrap();
        assert_eq!(cmdline, expected);
    }

    #[test]
    fn test_predict_cmdline_partuuid_substitution() {
        let grub_cfg = b"linux ($root)/vmlinuz root=PARTUUID=/PARTNROFF=1 -- x";
        let bootconfig = make_bootconfig("");
        let cmdline = predict_cmdline(grub_cfg, &bootconfig, Some("abcd-1234")).unwrap();
        assert_eq!(
            cmdline,
            "BOOT_IMAGE=/vmlinuz root=PARTUUID=abcd-1234/PARTNROFF=1 -- x"
        );
    }

    #[test]
    fn test_predict_cmdline_multiple_partuuid_substitution() {
        let grub_cfg = b"linux ($root)/vmlinuz PARTUUID=/PARTNROFF=1 PARTUUID=/PARTNROFF=2 -- x";
        let bootconfig = make_bootconfig("");
        let cmdline = predict_cmdline(grub_cfg, &bootconfig, Some("uuid-here")).unwrap();
        assert_eq!(cmdline, "BOOT_IMAGE=/vmlinuz PARTUUID=uuid-here/PARTNROFF=1 PARTUUID=uuid-here/PARTNROFF=2 -- x");
    }

    #[test]
    fn test_predict_includes_trailing_newline() {
        let grub_cfg = b"linux ($root)/vmlinuz -- x";
        let bootconfig = make_bootconfig("");
        use crate::predict::test_support::MockCtx;
        let m = MockCtx::new();
        let ctx = PcrContext::builder()
            .platform(crate::platform::Platform::Aws)
            .efi_vars(&m.efi_vars)
            .partitions(&m.layout)
            .grub_cfg(grub_cfg.as_slice())
            .bootconfig(bootconfig.as_slice())
            .build();
        let result = predict(&ctx).unwrap().unwrap();
        let cmdline_with_newline = "BOOT_IMAGE=/vmlinuz -- x\n";
        let expected = extend_pcr_string(&PCR_INIT_VAL, cmdline_with_newline);
        assert_eq!(result.1.sha256[0], hex::encode(expected));
    }

    #[test_case(b"linux /wrong/path console=tty0 -- x" ; "wrong_kernel_path")]
    #[test_case(b"linux (hd0,gpt3)/vmlinuz console=tty0 -- x" ; "explicit_device_in_path")]
    fn test_predict_cmdline_errors(grub_cfg: &[u8]) {
        let bootconfig = make_bootconfig("");
        let result = predict_cmdline(grub_cfg, &bootconfig, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_predict_skipped_for_ab() {
        use crate::predict::test_support::MockCtx;
        let m = MockCtx::dual_bank();
        let ctx = m.build(crate::platform::Platform::Aws);
        assert!(predict(&ctx).unwrap().is_none());
    }
}
