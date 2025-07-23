//! Migration system for Twoliter project files.
//!
//! This module provides functionality to automatically migrate Twoliter.toml files between different schema versions,
//! ensuring backward compatibility when new fields are added or the schema changes.

use crate::{
    compatibility::SUPPORTED_TWOLITER_LOCK_SCHEMA_VERSION,
    project::migrate::{
        implementations::V1ToV2Migrator,
        migrator::MigrationRegistry,
        parser::{detect_schema_version, parse_project_toml, UnvalidatedProject},
    },
};
use anyhow::{anyhow, Context, Result};
use std::any::Any;
mod implementations;
mod migrator;
// pub becase project/mod.rs also need this
pub(crate) mod parser;

/// Trait for migrating project configurations between schema versions.
pub trait ProjectMigrator: Send + Sync {
    /// The source schema version this migrator handles.
    fn current_version(&self) -> u32;

    /// The target schema version this migrator produces.
    fn to_version(&self) -> u32;

    /// Perform the migration from source to target version.
    fn migrate(&self, input: &dyn Any) -> Result<Box<dyn Any>>;
}

pub fn migrate_project_content(content: &str) -> Result<UnvalidatedProject> {
    // Create the default registry
    let registry = MigrationRegistry::default();
    // Call the private implementation
    migrate_project_content_with_registry(content, Some(&registry))
}

/// Migrate project content to the target version.
/// This is the main function used by the project loading system to automatically
/// handle migration when loading Twoliter.toml files.
fn migrate_project_content_with_registry(
    content: &str,
    registry: Option<&migrator::MigrationRegistry>,
) -> Result<UnvalidatedProject> {
    // Detect the current schema version
    let current_version =
        detect_schema_version(content).context("Failed to detect schema version")?;

    // If already at the latest version, just parse and return
    if current_version == SUPPORTED_TWOLITER_LOCK_SCHEMA_VERSION {
        return parse_project_toml(content)?
            .downcast::<UnvalidatedProject>()
            .map(|boxed| *boxed)
            .map_err(|e| anyhow!("Failed to downcast: {:?}", e));
    }

    // Parse the content into the appropriate type based on current version
    let parsed_content =
        parser::parse_project_toml(content).context("Failed to parse project content")?;

    // Use provided registry
    let registry = registry.ok_or(anyhow!("Registry is not provided"))?;

    // Migrate the content to the latest version
    registry
        .migrate_project(
            parsed_content,
            current_version,
            SUPPORTED_TWOLITER_LOCK_SCHEMA_VERSION,
        )
        .with_context(|| {
            format!(
                "Failed to migrate project from version {current_version} to version {SUPPORTED_TWOLITER_LOCK_SCHEMA_VERSION}"
            )
        })?
        .downcast::<UnvalidatedProject>()
        .map(|boxed| *boxed)
        .map_err(|e| anyhow!("Failed to downcast: {:?}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    
    
    

    #[test]
    fn test_migrate_project_content_v1_to_v2() {
        // Given A v1 project configuration
        let v1_content = r#"
            schema-version = 1
            release-version = "1.0.0"
        "#;

        // When The migration is performed
        let result = migrate_project_content(v1_content);

        // Then The project is successfully migrated to v2
        assert!(result.is_ok());
        let migrated = result.unwrap();
        assert_eq!(migrated.schema_version.get(), 2);
        assert_eq!(migrated.release_version, "1.0.0");
        assert_eq!(migrated.project_vendor, "Bottlerocket");
    }

    #[test]
    fn test_migrate_project_content_already_v2() {
        // Given A v2 project configuration
        let v2_content = r#"
            schema-version = 2
            release-version = "1.0.0"
            project-vendor = "CustomVendor"
        "#;

        // When The migration is performed
        let result = migrate_project_content(v2_content);

        // Then The project is returned as-is
        assert!(result.is_ok());
        let migrated = result.unwrap();
        assert_eq!(migrated.schema_version.get(), 2);
        assert_eq!(migrated.release_version, "1.0.0");
        assert_eq!(migrated.project_vendor, "CustomVendor");
    }

    #[test]
    fn test_migrate_project_content_invalid_version() {
        // Given A project with an unsupported schema version
        let invalid_content = r#"
            schema-version = 99
            release-version = "1.0.0"
        "#;

        // When The migration is attempted
        let result = migrate_project_content(invalid_content);

        // Then An error is returned
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Failed"));
    }

    #[test]
    fn test_migrate_project_content_invalid_toml() {
        // Given Invalid TOML content
        let invalid_toml = r#"
            schema-version = 1
            release-version = "1.0.0
        "#;

        // When The migration is attempted
        let result = migrate_project_content(invalid_toml);

        // Then An error is returned
        assert!(result.is_err());
    }

    #[test]
    fn test_migrate_project_content_with_registry_no_registry() {
        // Given Valid content but no registry
        let content = r#"
            schema-version = 1
            release-version = "1.0.0"
        "#;

        // When Migration is attempted without a registry
        let result = migrate_project_content_with_registry(content, None);

        // Then An error is returned
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Registry is not provided"));
    }
}
