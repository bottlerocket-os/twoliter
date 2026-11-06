//! The command line in a UKI's `.cmdline` section.

use crate::bootconfig::Bootconfig;
use crate::cmdline::Cmdline;
use crate::error::{self, Result};
use snafu::{ensure, ResultExt};
use std::fmt;

/// The command line in a UKI's `.cmdline` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UkiCmdline(Cmdline);

impl UkiCmdline {
    /// Compose the command line to embed: the `bootconfig` parameters ahead of the image's base
    /// `kernel_args` and `init_args`, as the kernel would add them at boot.
    pub fn compose(bootconfig: &Bootconfig, kernel_args: &str, init_args: &str) -> Result<Self> {
        let base = Cmdline::parse(&format!("{kernel_args} -- {init_args}"));
        Self::checked(base.prepend(bootconfig.0.clone()))
    }

    /// Decode the command line a built UKI carries in its `.cmdline` section.
    pub fn decode(section: &[u8]) -> Result<Self> {
        // Sections are NUL-padded
        let end = section
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(section.len());
        let (text, padding) = section.split_at(end);
        ensure!(
            padding.iter().all(|byte| *byte == 0),
            error::DataAfterNulSnafu
        );
        let text = std::str::from_utf8(text).context(error::InvalidUtf8Snafu {
            input: "UKI command line",
        })?;
        let cmdline = Cmdline::parse(text);
        // Parsing collapses whitespace outside quotes, which systemd-stub passes on as written.
        ensure!(
            cmdline.to_string() == text,
            error::UncollapsedWhitespaceSnafu
        );
        Self::checked(cmdline)
    }

    /// The kernel's EFI LoadOptions as systemd-stub sets them: UTF-16LE with a terminating NUL.
    pub fn load_options(&self) -> Vec<u8> {
        self.to_string()
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect()
    }

    /// Accept a command line that systemd-stub passes on unchanged, that tools collapsing
    /// whitespace leave unchanged, and that fits the kernel.
    fn checked(cmdline: Cmdline) -> Result<Self> {
        let line = cmdline.to_string();
        ensure!(
            line.chars()
                .all(|character| character == ' ' || character.is_ascii_graphic()),
            error::UnprintableCharacterSnafu
        );
        // An unpaired quote keeps the whitespace after it, so it is the cause to report.
        ensure!(
            line.matches('"').count().is_multiple_of(2),
            error::UnpairedQuoteSnafu
        );
        ensure!(
            line.trim() == line && !line.contains("  "),
            error::UncollapsedWhitespaceSnafu
        );
        Ok(Self(cmdline))
    }
}

impl fmt::Display for UkiCmdline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
