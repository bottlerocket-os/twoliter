//! The command line GRUB starts a kernel with.

pub(crate) mod config;

use crate::bootconfig::Bootconfig;
use crate::cmdline::Cmdline;
use crate::error::{self, Result};
use crate::grub::config::GrubConfig;
use snafu::{ensure, OptionExt};
use std::{fmt, iter};

const KERNEL_PATH: &str = "()/vmlinuz";
const BOOT_IMAGE: &str = "BOOT_IMAGE=/vmlinuz";
const PARTUUID_PLACEHOLDER: &str = "PARTUUID=/PARTNROFF=";

/// The command line GRUB starts a kernel with, from the booting `linux` directive of a grub.cfg.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrubCmdline(Cmdline);

impl GrubCmdline {
    /// Read the command line from grub.cfg, with `boot_partuuid`, the BOOT partition's PARTUUID,
    /// filling in the `PARTUUID=/PARTNROFF=` placeholder.
    pub fn parse(grub_cfg: &[u8], boot_partuuid: &str) -> Result<Self> {
        let config = GrubConfig::parse(grub_cfg)?;
        let (kernel_path, arguments) = config
            .linux_arguments()?
            .split_first()
            .context(error::MissingGrubKernelPathSnafu)?;
        ensure!(
            kernel_path == KERNEL_PATH,
            error::InvalidGrubKernelPathSnafu {
                expected: KERNEL_PATH,
                actual: kernel_path.clone(),
            }
        );

        let partuuid = format!("PARTUUID={boot_partuuid}/PARTNROFF=");
        let mut arguments: Vec<String> = arguments
            .iter()
            .map(|argument| repair_quotes(&argument.replace(PARTUUID_PLACEHOLDER, &partuuid)))
            .collect();
        // GRUB writes BOOT_IMAGE=/vmlinuz with a trailing space ahead of the arguments (even if its empty).
        if arguments.is_empty() {
            arguments.push(String::new());
        }
        Ok(Self(Cmdline::split(
            iter::once(BOOT_IMAGE.to_string()).chain(arguments),
        )))
    }

    /// The kernel's EFI LoadOptions as GRUB sets them: UTF-16LE without a terminating NUL.
    pub fn load_options(&self) -> Vec<u8> {
        self.to_string()
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect()
    }

    /// The command line the kernel reports on `/proc/cmdline` once it adds the `bootconfig`
    /// parameters: `<kernel.*> BOOT_IMAGE=<path> <grub kernel args> -- <init.*> <grub init args>`.
    pub fn proc_cmdline(&self, bootconfig: &Bootconfig) -> String {
        self.0.clone().prepend(bootconfig.0.clone()).to_string()
    }
}

impl fmt::Display for GrubCmdline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case("key=value", "key=value" ; "no_quotes_unchanged")]
    #[test_case(r#"key="value with spaces""#, r#""key=value with spaces""# ; "quoted_value_with_spaces")]
    #[test_case(r#"foo=bar key="quoted value" baz=qux"#, r#"foo=bar "key=quoted value" baz=qux"# ; "mixed_quoted_and_unquoted")]
    fn test_repair_quotes(input: &str, expected: &str) {
        assert_eq!(repair_quotes(input), expected);
    }
}
