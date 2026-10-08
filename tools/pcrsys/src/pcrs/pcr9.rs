//! PCR 9: Kernel Command Line
//!
//! PCR 9 measures the kernel command line, which for Bottlerocket consists of:
//! 1. Static parameters from grub.cfg
//! 2. Dynamic parameters from bootconfig.data (kernel.* and init.* sections)
//!
//! The final /proc/cmdline format is:
//! `<kernel.* params> BOOT_IMAGE=<path> <grub params before --> -- <init.* params> <grub params after -->`

use crate::error::Result;
use crate::predict::{extend_pcr_string, PcrContext, PcrIndex, PcrRecord, PCR_INIT_VAL};
use bootconfig::predict_grub_cmdline as predict_cmdline;

/// Predict PCR 9 value.
///
/// PCR 9 = extend(init, SHA256(cmdline + newline))
/// The trailing newline matches /proc/cmdline format.
pub fn predict(ctx: &PcrContext) -> Result<Option<(PcrIndex, PcrRecord)>> {
    if ctx.partitions.boot_b.is_some() {
        return Ok(None);
    }

    let mut cmdline = predict_cmdline(ctx.grub_cfg, ctx.bootconfig, Some(ctx.boot_partuuid))?;
    cmdline.push('\n');
    let pcr9 = extend_pcr_string(&PCR_INIT_VAL, &cmdline);
    Ok(Some((PcrIndex::Pcr9, PcrRecord::new(pcr9))))
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
