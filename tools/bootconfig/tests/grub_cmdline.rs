mod common;

use bootconfig::{Bootconfig, Error, GrubCmdline, Result};
use common::encoded_bootconfig_data;
use test_case::test_case;

/// The command line the kernel reports, after it adds the bootconfig parameters.
fn predict_cmdline(grub_cfg: &[u8], bootconfig: &str, boot_partuuid: &str) -> Result<String> {
    let grub_cmdline = GrubCmdline::parse(grub_cfg, boot_partuuid)?;
    let bootconfig = Bootconfig::decode(&encoded_bootconfig_data(bootconfig))?;
    Ok(grub_cmdline.proc_cmdline(&bootconfig))
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
    b"linux ($root)/vmlinuz -- x",
    "kernel.MSG = \"hello world\"\n",
    r#"MSG="hello world" BOOT_IMAGE=/vmlinuz -- x"#
    ; "bootconfig_quoted_value_not_repaired"
)]
fn test_predict_cmdline(grub_cfg: &[u8], bootconfig: &str, expected: &str) {
    assert_eq!(
        predict_cmdline(grub_cfg, bootconfig, "abcd-1234").unwrap(),
        expected
    );
}

#[test_case(
    br#"linux ($root)/vmlinuz dm-mod.create="a -- b" -- x"#,
    r#"BOOT_IMAGE=/vmlinuz "dm-mod.create=a -- b" -- x"#
    ; "the_separator_inside_a_value_does_not_divide_the_arguments"
)]
#[test_case(
    b"linux ($root)/vmlinuz quiet",
    "BOOT_IMAGE=/vmlinuz quiet"
    ; "every_argument_stays_with_the_kernel_without_a_separator"
)]
#[test_case(
    b"linux ($root)/vmlinuz",
    "BOOT_IMAGE=/vmlinuz "
    ; "a_directive_with_no_arguments_boots_the_image_alone"
)]
fn test_predict_cmdline_separator(grub_cfg: &[u8], expected: &str) {
    assert_eq!(
        predict_cmdline(grub_cfg, "", "abcd-1234").unwrap(),
        expected
    );
}

#[test_case(
    br#"linux ($root)/vmlinuz quiet dm-mod.create="a b" -- systemd.log_target=journal"#,
    r#"BOOT_IMAGE=/vmlinuz quiet "dm-mod.create=a b" -- systemd.log_target=journal"#
    ; "every_argument_in_order_with_the_separator"
)]
#[test_case(
    b"linux ($root)/vmlinuz quiet",
    "BOOT_IMAGE=/vmlinuz quiet"
    ; "no_separator_is_added"
)]
#[test_case(
    b"linux ($root)/vmlinuz",
    "BOOT_IMAGE=/vmlinuz "
    ; "the_image_alone_keeps_the_space_grub_writes_after_it"
)]
fn test_grub_cmdline(grub_cfg: &[u8], expected: &str) {
    let grub_cmdline = GrubCmdline::parse(grub_cfg, "abcd-1234").unwrap();
    assert_eq!(grub_cmdline.to_string(), expected);
}

#[test]
fn load_options_are_utf16_without_a_nul() {
    let grub_cmdline = GrubCmdline::parse(b"linux ($root)/vmlinuz -- x", "").unwrap();
    let expected: Vec<u8> = "BOOT_IMAGE=/vmlinuz -- x"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    assert_eq!(grub_cmdline.load_options(), expected);
}

#[test]
fn test_predict_cmdline_multiple_partuuid_substitution() {
    let grub_cfg = b"linux ($root)/vmlinuz PARTUUID=/PARTNROFF=1 PARTUUID=/PARTNROFF=2 -- x";
    assert_eq!(
        predict_cmdline(grub_cfg, "", "uuid-here").unwrap(),
        concat!(
            "BOOT_IMAGE=/vmlinuz PARTUUID=uuid-here/PARTNROFF=1 ",
            "PARTUUID=uuid-here/PARTNROFF=2 -- x",
        )
    );
}

#[test]
fn a_kernel_on_an_explicit_device_is_rejected() {
    assert!(matches!(
        predict_cmdline(
            b"linux (hd0,gpt3)/vmlinuz console=tty0 -- x",
            "",
            "abcd-1234"
        ),
        Err(Error::InvalidGrubKernelPath { .. })
    ));
}

#[test]
fn a_directive_naming_no_kernel_is_rejected() {
    assert!(matches!(
        predict_cmdline(b"linux\n", "", "abcd-1234"),
        Err(Error::MissingGrubKernelPath)
    ));
}
