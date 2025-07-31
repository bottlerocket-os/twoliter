use crate::project::{Image, ValidIdentifier, Vendor};
use crate::schema_version::SchemaVersion;
use anyhow::{Context, Result};
use maplit::btreemap;
use serde::Deserialize;
use std::any::Any;
use std::collections::BTreeMap;
use std::sync::LazyLock;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct UnvalidatedProjectV1 {
    #[expect(dead_code)]
    pub(crate) schema_version: SchemaVersion<1>,
    pub(crate) release_version: String,
    pub(crate) sdk: Option<Image>,
    pub(crate) vendor: Option<BTreeMap<ValidIdentifier, Vendor>>,
    pub(crate) kit: Option<Vec<Image>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct UnvalidatedProjectV2 {
    pub(crate) schema_version: SchemaVersion<2>,
    pub(crate) release_version: String,
    pub(crate) project_vendor: String,
    pub(crate) sdk: Option<Image>,
    pub(crate) vendor: Option<BTreeMap<ValidIdentifier, Vendor>>,
    pub(crate) kit: Option<Vec<Image>>,
}

pub(crate) type UnvalidatedProject = UnvalidatedProjectV2;

/// Detects the schema version from a TOML string
pub fn detect_schema_version(content: &str) -> Result<u32> {
    #[derive(Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct MaybeVersionedProject {
        schema_version: i32,
    }
    let project: MaybeVersionedProject =
        toml::from_str(content).context("Failed to extract schema version")?;
    Ok(project.schema_version as u32)
}

/// Maps schema versions to their parsing functions
type ProjectParser = Box<dyn Fn(&str) -> Result<Box<dyn Any>> + Sync + Send>;
type ProjectParserMap = BTreeMap<u32, ProjectParser>;

fn parse_v1(content: &str) -> anyhow::Result<Box<dyn Any>> {
    let project: UnvalidatedProjectV1 =
        toml::from_str(content).context("Failed to parse TOML as UnvalidatedProjectV1")?;
    Ok(Box::new(project))
}

fn parse_v2(content: &str) -> anyhow::Result<Box<dyn Any>> {
    let project: UnvalidatedProjectV2 =
        toml::from_str(content).context("Failed to parse TOML as UnvalidatedProjectV2")?;
    Ok(Box::new(project))
}

static SCHEMA_VERSION_PARSERS: LazyLock<ProjectParserMap> = LazyLock::new(|| {
    btreemap! {
        1 => Box::new(parse_v1) as ProjectParser,
        2 => Box::new(parse_v2) as ProjectParser,
    }
});

/// Parses a TOML string into the appropriate project type based on schema version
pub fn parse_project_toml(content: &str) -> Result<Box<dyn Any>> {
    let version = detect_schema_version(content).unwrap_or(0);

    if let Some(parser) = SCHEMA_VERSION_PARSERS.get(&version) {
        parser(content)
    } else {
        anyhow::bail!("Unknown schema version: {version}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_schema_version_v1() {
        // Given A v1 project configuration
        let v1_content = r#"
            schema-version = 1
            release-version = "1.0.0"
        "#;

        // When The schema version is detected
        let result = detect_schema_version(v1_content);

        // Then The correct version is returned
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 1);
    }

    #[test]
    fn test_detect_schema_version_v2() {
        // Given A v2 project configuration
        let v2_content = r#"
            schema-version = 2
            release-version = "1.0.0"
            project-vendor = "CustomVendor"
        "#;

        // When The schema version is detected
        let result = detect_schema_version(v2_content);

        // Then The correct version is returned
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 2);
    }

    #[test]
    fn test_detect_schema_version_invalid() {
        // Given Invalid TOML content
        let invalid_content = r#"
            schema-version = "not a number"
            release-version = "1.0.0"
        "#;

        // When The schema version is detected
        let result = detect_schema_version(invalid_content);

        // Then An error is returned
        assert!(result.is_err());
    }

    #[test]
    fn test_detect_schema_version_missing() {
        // Given TOML content without schema version
        let missing_version = r#"
            release-version = "1.0.0"
        "#;

        // When The schema version is detected
        let result = detect_schema_version(missing_version);

        // Then An error is returned
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_project_toml_v1() {
        // Given A v1 project configuration
        let v1_content = r#"
            schema-version = 1
            release-version = "1.0.0"
        "#;

        // When The project is parsed
        let result = parse_project_toml(v1_content);

        // Then The correct type is returned
        assert!(result.is_ok());
        let project = result.unwrap();
        let v1_project = project.downcast_ref::<UnvalidatedProjectV1>();
        assert!(v1_project.is_some());
        let v1_project = v1_project.unwrap();
        assert_eq!(v1_project.schema_version.get(), 1);
        assert_eq!(v1_project.release_version, "1.0.0");
    }

    #[test]
    fn test_parse_project_toml_v2() {
        // Given A v2 project configuration
        let v2_content = r#"
            schema-version = 2
            release-version = "1.0.0"
            project-vendor = "CustomVendor"
        "#;

        // When The project is parsed
        let result = parse_project_toml(v2_content);

        // Then The correct type is returned
        assert!(result.is_ok());
        let project = result.unwrap();
        let v2_project = project.downcast_ref::<UnvalidatedProjectV2>();
        assert!(v2_project.is_some());
        let v2_project = v2_project.unwrap();
        assert_eq!(v2_project.schema_version.get(), 2);
        assert_eq!(v2_project.release_version, "1.0.0");
        assert_eq!(v2_project.project_vendor, "CustomVendor");
    }

    #[test]
    fn test_parse_project_toml_unknown_version() {
        // Given A project with an unsupported schema version
        let unknown_content = r#"
            schema-version = 99
            release-version = "1.0.0"
        "#;

        // When The project is parsed
        let result = parse_project_toml(unknown_content);

        // Then An error is returned
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown schema version"));
    }

    #[test]
    fn test_parse_project_toml_invalid_v1() {
        // Given Invalid v1 project configuration
        let invalid_v1 = r#"
            schema-version = 1
            release-version = 42  # Should be a string
        "#;

        // When The project is parsed
        let result = parse_project_toml(invalid_v1);

        // Then An error is returned
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_project_toml_invalid_v2() {
        // Given Invalid v2 project configuration
        let invalid_v2 = r#"
            schema-version = 2
            release-version = "1.0.0"
            # Missing required project-vendor field
        "#;

        // When The project is parsed
        let result = parse_project_toml(invalid_v2);

        // Then An error is returned
        assert!(result.is_err());
    }
}
