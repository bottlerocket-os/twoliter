use crate::{error::Result, parse, parsers::grub, BootConfig, Parameter};
use snafu::{ensure_whatever, whatever};

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
pub fn predict_grub_cmdline(
    grub_cfg: &[u8],
    bootconfig_data: &[u8],
    boot_partuuid: Option<&str>,
) -> Result<String> {
    let grub_cmdline = grub::parse(grub_cfg)?;
    let bootconfig_params = parse(bootconfig_data)?;
    let kernel_params = format_params(&bootconfig_params.kernel)?;
    let init_params = format_params(&bootconfig_params.init)?;

    // Verify and transform kernel path
    if !grub_cmdline.starts_with(KERNEL_PATH_PREFIX) {
        whatever!(
            "grub.cfg kernel path must start with '{}', got: {}",
            KERNEL_PATH_PREFIX.trim(),
            grub_cmdline.chars().take(20).collect::<String>()
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

/// Render using Linux `xbc_snprint_cmdline` rules, including trailing spaces.
///
/// Double quotes inside values cannot be faithfully represented in a kernel
/// argument and are rejected instead of being interpreted as shell escapes.
pub fn format_params(params: &[Parameter]) -> Result<String> {
    let mut out = String::new();
    for parameter in params {
        out.push_str(&parameter.name);
        if let Some(value) = &parameter.value {
            ensure_whatever!(
                !value.contains('"'),
                "unsupported quote in '{}'",
                parameter.name
            );
            out.push('=');
            let quote = value.contains([' ', '\t', '\r', '\n']);
            if quote {
                out.push('"');
            }
            out.push_str(value);
            if quote {
                out.push('"');
            }
        }
        out.push(' ');
    }
    Ok(out)
}

impl BootConfig {
    /// Compose the signed UKI command line from bootconfig and base arguments.
    /// Always includes an explicit `--` boundary, even with no init arguments.
    ///
    /// Rejects control characters, an unbalanced quote, an extra argument
    /// separator, and command lines exceeding the supported kernel's 2048-byte
    /// buffer including NUL. No GRUB variable or quote repair is performed.
    pub fn uki_cmdline(&self, kernel: &str, init: &str) -> Result<String> {
        validate_arguments(kernel)?;
        validate_arguments(init)?;
        let kernel = format!("{}{kernel}", format_params(&self.kernel)?);
        let init = format!("{}{init}", format_params(&self.init)?);
        validate_arguments(&kernel)?;
        validate_arguments(&init)?;
        let cmdline = [kernel.trim(), "--", init.trim()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        ensure_whatever!(
            cmdline.len() < 2048,
            "UKI command line exceeds kernel buffer"
        );
        Ok(cmdline)
    }
}

fn validate_arguments(args: &str) -> Result<()> {
    ensure_whatever!(
        args.is_ascii() && !args.chars().any(char::is_control),
        "UKI command line must be printable ASCII"
    );
    let mut quoted = false;
    let mut token_start = 0;
    for (i, c) in args.char_indices() {
        if c == '"' {
            quoted = !quoted;
        }
        if c == ' ' && !quoted {
            ensure_whatever!(
                &args[token_start..i] != "--",
                "unexpected command-line separator"
            );
            token_start = i + 1;
        }
    }
    ensure_whatever!(!quoted, "unbalanced command-line quote");
    ensure_whatever!(
        &args[token_start..] != "--",
        "unexpected command-line separator"
    );
    Ok(())
}

/// Apply systemd-stub's control-character and surrounding whitespace handling.
/// The returned bytes are EFI LoadOptions, encoded as UTF-16LE including NUL.
///
/// Embedded NUL characters are rejected rather than truncating the command line.
pub fn uki_load_options(cmdline: &str) -> Result<Vec<u8>> {
    ensure_whatever!(
        cmdline.is_ascii() && !cmdline.contains('\0'),
        "UKI command line must be ASCII without embedded NUL"
    );
    let whitespace = |c: char| c <= '\u{20}' || c == '\u{7f}';
    let normalized: String = cmdline
        .trim_matches(whitespace)
        .chars()
        .map(|c| if whitespace(c) { ' ' } else { c })
        .collect();
    Ok(normalized
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test]
    fn uki_arguments_keep_kernel_and_init_namespaces_separate() {
        let config = crate::parse_text(
            "kernel { console=tty0,ttyS0; quiet }\ninit.systemd.unit=rescue.target",
        )
        .unwrap();
        assert_eq!(
            config.uki_cmdline("root=/dev/dm-0", "").unwrap(),
            "console=tty0 console=ttyS0 quiet root=/dev/dm-0 -- systemd.unit=rescue.target"
        );
        assert!(config.uki_cmdline("quiet -- init", "").is_err());
    }

    #[test]
    fn load_options_normalize_whitespace_and_include_utf16_nul() {
        assert_eq!(uki_load_options(" \tA\tB ").unwrap(), b"A\0 \0B\0\0\0");
        assert!(uki_load_options("a\0b").is_err());
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
}
