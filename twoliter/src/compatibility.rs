//! This module contains version numbers which allow Twoliter to detect compatibility with its
//! own artifacts.

/// Defines the latest schema version of Twoliter.toml supported by twoliter
pub const LATEST_TWOLITER_PROJECT_SCHEMA_VERSION: u32 = 2;

pub const SUPPORTED_TWOLITER_LOCK_SCHEMA_VERSION: u32 = 2;

/// Defines the kit metadata version supported by twoliter.
///
/// The kit metadata version is embeddeded in a label within the OCI image's configuration blob,
/// with the value stored at that label including the kit metadata itself.
pub const SUPPORTED_KIT_METADATA_VERSION: &str = "v3";
