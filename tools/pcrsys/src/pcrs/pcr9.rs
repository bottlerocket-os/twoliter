//! PCR 9: Kernel Command Line
//!
//! PCR 9 measures the kernel command line, which for Bottlerocket consists of:
//! 1. Static parameters from grub.cfg
//! 2. Dynamic parameters from bootconfig.data (kernel.* and init.* sections)
//!
//! The final /proc/cmdline format is:
//! `<kernel.* params> BOOT_IMAGE=<path> <grub params before --> -- <init.* params> <grub params after -->`

use crate::error::Result;
use crate::parsers::{bootconfig, grub};
use crate::predict::{extend_pcr_string, PcrContext, PcrIndex, PcrRecord, PCR_INIT_VAL};
use snafu::whatever;

const KERNEL_PATH_PREFIX: &str = "()/vmlinuz ";

/// Transform grub.cfg shell-style quoting `key="value"` to kernel cmdline format `"key=value"`.
///
/// grub.cfg uses shell-style quoting where values are quoted: `root="UUID=abc"`
/// The kernel command line expects the entire key=value pair quoted: `"root=UUID=abc"`
/// This function performs that transformation for PCR 9 prediction.
fn repair_quotes(cmdline: &str) -> String {
    let mut result = String::with_capacity(cmdline.len());
    let mut chars = cmdline.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '=' && chars.peek() == Some(&'"') {
            // Found `="`; scan back to find start of key
            let key_start = result.rfind(' ').map(|i| i + 1).unwrap_or(0);
            let key = result[key_start..].to_string();
            result.truncate(key_start);

            // Skip the opening quote
            chars.next();

            // Collect value until closing quote
            let mut value = String::new();
            for vc in chars.by_ref() {
                if vc == '"' {
                    break;
                }
                value.push(vc);
            }

            // Output as "key=value"
            result.push('"');
            result.push_str(&key);
            result.push('=');
            result.push_str(&value);
            result.push('"');
        } else {
            result.push(c);
        }
    }
    result
}

/// Predict /proc/cmdline from grub.cfg and bootconfig.
///
/// The kernel constructs /proc/cmdline as:
/// `<kernel.* bootconfig> BOOT_IMAGE=<path> <grub args> -- <init.* bootconfig> <grub args after -->`
///
/// If `boot_partuuid` is provided, replaces `PARTUUID=/PARTNROFF=` with the actual UUID.
fn predict_cmdline(
    grub_cfg: &[u8],
    bootconfig_data: &[u8],
    boot_partuuid: Option<&str>,
) -> Result<String> {
    let grub_cmdline = grub::parse(grub_cfg)?;
    let bootconfig_params = bootconfig::parse(bootconfig_data)?;
    let kernel_params = bootconfig::format_params(&bootconfig_params.kernel);
    let init_params = bootconfig::format_params(&bootconfig_params.init);

    // Verify and transform kernel path
    if !grub_cmdline.starts_with(KERNEL_PATH_PREFIX) {
        whatever!(
            "grub.cfg kernel path must start with '{}', got: {}",
            KERNEL_PATH_PREFIX.trim(),
            &grub_cmdline[..grub_cmdline.len().min(20)]
        );
    }
    let mut grub_args = grub_cmdline.replacen(KERNEL_PATH_PREFIX, "BOOT_IMAGE=/vmlinuz ", 1);

    // Substitute PARTUUID placeholder with actual boot partition UUID
    if let Some(uuid) = boot_partuuid {
        grub_args = grub_args.replace(
            "PARTUUID=/PARTNROFF=",
            &format!("PARTUUID={uuid}/PARTNROFF="),
        );
    }

    // Apply kernel's quote repair transformation to grub args
    grub_args = repair_quotes(&grub_args);

    // Split grub args at "--"
    let (before_sep, after_sep) = if let Some(pos) = grub_args.find(" -- ") {
        (&grub_args[..pos], &grub_args[pos + 4..])
    } else {
        (grub_args.as_str(), "")
    };

    // Construct final cmdline
    let mut cmdline = String::new();
    cmdline.push_str(&kernel_params);
    cmdline.push_str(before_sep);
    cmdline.push_str(" -- ");
    cmdline.push_str(&init_params);
    cmdline.push_str(after_sep);

    Ok(cmdline)
}

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
        let text_bytes = text.as_bytes();
        let size = text_bytes.len() as u32;
        let checksum: u32 = text_bytes.iter().map(|&b| b as u32).sum();
        let padding = (4 - (text_bytes.len() % 4)) % 4;
        let mut data = text_bytes.to_vec();
        data.extend(vec![0u8; padding]);
        data.extend(size.to_le_bytes());
        data.extend(checksum.to_le_bytes());
        data.extend(b"#BOOTCONFIG\n");
        data
    }

    #[test_case("key=value", "key=value" ; "no_quotes_unchanged")]
    #[test_case("simple", "simple" ; "no_equals_unchanged")]
    #[test_case(r#"key="value""#, r#""key=value""# ; "simple_quoted_value")]
    #[test_case(r#"key="value with spaces""#, r#""key=value with spaces""# ; "quoted_value_with_spaces")]
    #[test_case(r#"foo=bar key="quoted value" baz=qux"#, r#"foo=bar "key=quoted value" baz=qux"# ; "mixed_quoted_and_unquoted")]
    #[test_case(r#"dm-mod.create="root,,,ro,0 123 verity""#, r#""dm-mod.create=root,,,ro,0 123 verity""# ; "dm_mod_create_style")]
    #[test_case(r#"a="1" b="2" c="3""#, r#""a=1" "b=2" "c=3""# ; "multiple_quoted_values")]
    #[test_case(r#"first="val""#, r#""first=val""# ; "quoted_at_start")]
    #[test_case("", "" ; "empty_string")]
    fn test_repair_quotes(input: &str, expected: &str) {
        assert_eq!(repair_quotes(input), expected);
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
