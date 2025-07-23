//! Migration from schema version 1 to version 2.
//!
//! This migration adds the required `project-vendor` field to the Twoliter.toml file.
//! The default value "Bottlerocket" is used when migrating from v1 to v2.

use std::any::Any;

use anyhow::{Context, Result};

use crate::{
    project::migrate::{
        parser::{UnvalidatedProjectV1, UnvalidatedProjectV2},
        ProjectMigrator,
    },
    schema_version::SchemaVersion,
};

/// Migrator for upgrading from schema version 1 to version 2.
pub struct V1ToV2Migrator;

impl ProjectMigrator for V1ToV2Migrator {
    fn current_version(&self) -> u32 {
        SchemaVersion::<1>.get()
    }

    fn to_version(&self) -> u32 {
        SchemaVersion::<2>.get()
    }

    fn migrate(&self, input: &dyn Any) -> Result<Box<dyn Any>> {
        let source: &UnvalidatedProjectV1 = input
            .downcast_ref()
            .context("Wrong Twoliter.toml version, expected schema-version v1")?;

        let unvalidated_project_v2 = UnvalidatedProjectV2 {
            schema_version: SchemaVersion::<2>,
            release_version: source.release_version.to_owned(),
            sdk: source.sdk.to_owned(),
            vendor: source.vendor.to_owned(),
            kit: source.kit.to_owned(),
            project_vendor: String::from("Bottlerocket"),
        };

        Ok(Box::new(unvalidated_project_v2) as _)
    }
}
