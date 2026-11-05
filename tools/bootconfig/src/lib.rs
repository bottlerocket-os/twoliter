//! Bootconfig decoding and boot command-line construction shared by image assembly and PCR
//! prediction.

mod bootconfig;
mod cmdline;
mod error;
mod grub;
mod uki;

pub use bootconfig::{merge_parameters, Bootconfig};
pub use error::{Error, Result};
pub use grub::GrubCmdline;
pub use uki::UkiCmdline;
