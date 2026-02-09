//! AWS-specific functionality for PCR prediction.

pub mod ami;
mod uefi;
mod zlib_dict;

pub use uefi::parse_aws_uefi_data;
