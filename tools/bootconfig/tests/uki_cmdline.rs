mod common;

use bootconfig::{Bootconfig, Error, Result, UkiCmdline};
use common::encoded_bootconfig_data;
use test_case::test_case;

/// Compose a UKI command line from bootconfig text and the image's base arguments.
fn compose(bootconfig: &str, kernel_args: &str, init_args: &str) -> Result<String> {
    let bootconfig = Bootconfig::decode(&encoded_bootconfig_data(bootconfig))?;
    Ok(UkiCmdline::compose(&bootconfig, kernel_args, init_args)?.to_string())
}

#[test]
fn a_section_must_be_utf8() {
    assert!(matches!(
        UkiCmdline::decode(b"\xff".as_slice()),
        Err(Error::InvalidUtf8 { .. })
    ));
}

#[test]
fn data_after_the_nul_padding_is_rejected() {
    assert!(matches!(
        UkiCmdline::decode(b"quiet\0extra"),
        Err(Error::DataAfterNul)
    ));
}

#[test_case(b"quiet  ro" ; "a_run_of_spaces")]
#[test_case(b"quiet\tro" ; "a_tab")]
#[test_case(b" quiet" ; "a_leading_space")]
#[test_case(b"quiet \0" ; "a_trailing_space")]
fn a_section_that_does_not_format_back_to_its_bytes_is_rejected(section: &[u8]) {
    assert!(matches!(
        UkiCmdline::decode(section),
        Err(Error::UncollapsedWhitespace)
    ));
}

#[test_case("é" ; "non_ascii")]
#[test_case("quiet\x7f" ; "a_del")]
#[test_case("dm=\"a\tb\"" ; "a_quoted_tab")]
fn a_section_systemd_stub_would_alter_is_rejected(section: &str) {
    assert!(matches!(
        UkiCmdline::decode(section.as_bytes()),
        Err(Error::UnprintableCharacter)
    ));
}

#[test]
fn a_quoted_run_of_spaces_is_rejected() {
    assert!(matches!(
        UkiCmdline::decode(b"dm=\"a  b\""),
        Err(Error::UncollapsedWhitespace)
    ));
}

#[test]
fn a_composed_command_line_decodes_to_itself() {
    let bootconfig = "kernel.fips = 1\ninit.systemd.unit = fipscheck.target\n";
    let composed = compose(bootconfig, r#"root=/dev/dm-0 dm-mod.create="a b""#, "quiet").unwrap();
    let decoded = UkiCmdline::decode(composed.as_bytes()).unwrap();
    assert_eq!(decoded.to_string(), composed);
}

#[test]
fn load_options_end_with_a_utf16_nul() {
    let uki_cmdline = UkiCmdline::decode(b"A B").unwrap();
    assert_eq!(uki_cmdline.load_options(), b"A\0 \0B\0\0\0");
}

#[test]
fn test_embed_quote_value() {
    assert_eq!(
        compose("", "dm-mod.create=\"a b\"", "").unwrap(),
        "dm-mod.create=\"a b\""
    );
}

#[test_case("root=/dev/dm-0  ro" ; "a_run_of_spaces")]
#[test_case("root=/dev/dm-0\tro" ; "a_tab")]
fn test_trim_whitespaces(kernel_args: &str) {
    assert_eq!(compose("", kernel_args, "").unwrap(), "root=/dev/dm-0 ro");
}

#[test]
fn an_unpaired_quote_is_rejected() {
    assert!(matches!(
        compose("", "dm-mod.create=\"root", ""),
        Err(Error::UnpairedQuote)
    ));
}
