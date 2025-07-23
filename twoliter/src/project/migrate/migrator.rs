pub use crate::project::migrate::ProjectMigrator;
use crate::project::migrate::V1ToV2Migrator;
use anyhow::{Context, Ok, Result};
use std::{
    any::Any,
    collections::{HashMap, HashSet, VecDeque},
};

pub(crate) struct MigrationRegistry {
    pub(crate) migrators: HashMap<(u32, u32), Box<dyn ProjectMigrator>>,
}

impl Default for MigrationRegistry {
    fn default() -> Self {
        let mut registry = MigrationRegistry::new(vec![]);
        registry.register((1, 2), Box::new(V1ToV2Migrator));
        registry
    }
}

impl MigrationRegistry {
    /// Create a new migration registry with all available migrators.
    fn new(migrators: Vec<Box<dyn ProjectMigrator>>) -> Self {
        // Convert the Vec to a HashMap indexed by (source_version, target_version)
        let registry_map = migrators
            .into_iter()
            .map(|migrator| {
                (
                    (migrator.current_version(), migrator.to_version()),
                    migrator,
                )
            })
            .collect();

        Self {
            migrators: registry_map,
        }
    }

    /// Register a new migrator.
    fn register(&mut self, key: (u32, u32), migrator: Box<dyn ProjectMigrator>) {
        self.migrators.insert(key, migrator);
    }

    /// Find a migrator that can handle the given version transition.
    fn find_migration_edge(
        &self,
        from_version: u32,
        to_version: u32,
    ) -> Option<&dyn ProjectMigrator> {
        self.migrators
            .get(&(from_version, to_version))
            .map(|m| m.as_ref())
    }

    /// Find the shortest migration path from source version to target version using BFS
    fn find_migration_path(&self, source: u32, target: u32) -> Option<Vec<(u32, u32)>> {
        if source == target {
            return Some(Vec::new());
        }

        let mut graph: HashMap<u32, Vec<u32>> = HashMap::new();
        for &(src, dst) in self.migrators.keys() {
            graph.entry(src).or_default().push(dst);
        }

        let mut queue = VecDeque::new();
        let mut visited = HashSet::new();
        let mut parent: HashMap<u32, u32> = HashMap::new();

        queue.push_back(source);
        visited.insert(source);

        while let Some(current) = queue.pop_front() {
            if current == target {
                return Some(self.reconstruct_path(&parent, source, target));
            }

            if let Some(neighbors) = graph.get(&current) {
                for &next in neighbors {
                    if !visited.contains(&next) {
                        visited.insert(next);
                        parent.insert(next, current);
                        queue.push_back(next);
                    }
                }
            }
        }
        None
    }

    /// Reconstructs the path from source to target using the parent map
    fn reconstruct_path(
        &self,
        parent: &HashMap<u32, u32>,
        source: u32,
        target: u32,
    ) -> Vec<(u32, u32)> {
        let mut path = Vec::new();
        let mut current = target;

        while current != source {
            let prev = *parent
                .get(&current)
                .expect("parents of the discovered path to be populated correctly");
            path.push((prev, current));
            current = prev;
        }

        path.reverse();
        path
    }

    /// Migrate a project file from a version to the next version.
    ///
    /// This method will find the appropriate migration path and apply all necessary
    /// transformations to get from the source version to the target version.
    pub fn migrate_project(
        &self,
        mut content: Box<dyn Any>,
        source: u32,
        target: u32,
    ) -> Result<Box<dyn Any>> {
        if source == target {
            return Ok(content);
        }

        let path = self.find_migration_path(source, target).with_context(|| {
            format!("Failed to find migration path from v{source} to v{target}")
        })?;

        for (from_version, to_version) in path {
            let migrator = self
                .find_migration_edge(from_version, to_version)
                .with_context(|| {
                    format!("Failed to find migrator from v{from_version} to v{to_version}")
                })?;

            content = migrator.migrate(content.as_ref()).with_context(|| {
                format!("Failed to migrate from v{from_version} to v{to_version}")
            })?;
        }

        Ok(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use mockall::predicate::*;
    use mockall::*;
    use std::any::Any;

    // Create a mock for the ProjectMigrator trait
    mock! {
        pub ProjectMigrator {}
        impl ProjectMigrator for ProjectMigrator {
            fn current_version(&self) -> u32;
            fn to_version(&self) -> u32;
            fn migrate(&self, input: &dyn Any) -> Result<Box<dyn Any>>;
        }
        impl Clone for ProjectMigrator {
            fn clone(&self) -> Self;
        }
    }

    #[test]
    fn test_registry_creation() {
        // Given A list of mock migrators
        let mut mock1 = MockProjectMigrator::new();
        mock1.expect_current_version().return_const(1u32);
        mock1.expect_to_version().return_const(2u32);

        let mut mock2 = MockProjectMigrator::new();
        mock2.expect_current_version().return_const(2u32);
        mock2.expect_to_version().return_const(3u32);

        let migrators: Vec<Box<dyn ProjectMigrator>> = vec![
            Box::new(mock1),
            Box::new(mock2),
        ];

        // When A registry is created
        let registry = MigrationRegistry::new(migrators);

        // Then The registry contains the migrators
        assert_eq!(registry.migrators.len(), 2);
        assert!(registry.migrators.contains_key(&(1, 2)));
        assert!(registry.migrators.contains_key(&(2, 3)));
    }

    #[test]
    fn test_register_migrator() {
        // Given An empty registry
        let mut registry = MigrationRegistry::new(vec![]);

        // And A mock migrator
        let mut mock = MockProjectMigrator::new();
        mock.expect_current_version().return_const(1u32);
        mock.expect_to_version().return_const(2u32);

        // When The migrator is registered
        registry.register((1, 2), Box::new(mock));

        // Then The registry contains the migrator
        assert_eq!(registry.migrators.len(), 1);
        assert!(registry.migrators.contains_key(&(1, 2)));
    }

    #[test]
    fn test_find_migration_edge() {
        // Given A registry with a mock migrator
        let mut registry = MigrationRegistry::new(vec![]);

        let mut mock = MockProjectMigrator::new();
        mock.expect_current_version().return_const(1u32);
        mock.expect_to_version().return_const(2u32);

        registry.register((1, 2), Box::new(mock));

        // When Finding a migration edge
        let edge = registry.find_migration_edge(1, 2);

        // Then The correct migrator is found
        assert!(edge.is_some());
        assert_eq!(edge.unwrap().current_version(), 1);
        assert_eq!(edge.unwrap().to_version(), 2);

        // And Non-existent edges return None
        assert!(registry.find_migration_edge(2, 3).is_none());
    }

    #[test]
    fn test_find_migration_path_direct() {
        // Given A registry with a direct path
        let mut registry = MigrationRegistry::new(vec![]);

        let mut mock = MockProjectMigrator::new();
        mock.expect_current_version().return_const(1u32);
        mock.expect_to_version().return_const(2u32);

        registry.register((1, 2), Box::new(mock));

        // When Finding a migration path
        let path = registry.find_migration_path(1, 2);

        // Then The correct path is found
        assert!(path.is_some());
        let path = path.unwrap();
        assert_eq!(path.len(), 1);
        assert_eq!(path[0], (1, 2));
    }

    #[test]
    fn test_find_migration_path_multi_step() {
        // Given A registry with multiple steps
        let mut registry = MigrationRegistry::new(vec![]);

        let mut mock1 = MockProjectMigrator::new();
        mock1.expect_current_version().return_const(1u32);
        mock1.expect_to_version().return_const(2u32);

        let mut mock2 = MockProjectMigrator::new();
        mock2.expect_current_version().return_const(2u32);
        mock2.expect_to_version().return_const(3u32);

        let mut mock3 = MockProjectMigrator::new();
        mock3.expect_current_version().return_const(3u32);
        mock3.expect_to_version().return_const(4u32);

        registry.register((1, 2), Box::new(mock1));
        registry.register((2, 3), Box::new(mock2));
        registry.register((3, 4), Box::new(mock3));

        // When Finding a migration path
        let path = registry.find_migration_path(1, 4);

        // Then The correct path is found
        assert!(path.is_some());
        let path = path.unwrap();
        assert_eq!(path.len(), 3);
        assert_eq!(path[0], (1, 2));
        assert_eq!(path[1], (2, 3));
        assert_eq!(path[2], (3, 4));
    }

    #[test]
    fn test_find_migration_path_no_path() {
        // Given A registry with disconnected paths
        let mut registry = MigrationRegistry::new(vec![]);

        let mut mock1 = MockProjectMigrator::new();
        mock1.expect_current_version().return_const(1u32);
        mock1.expect_to_version().return_const(2u32);

        let mut mock2 = MockProjectMigrator::new();
        mock2.expect_current_version().return_const(3u32);
        mock2.expect_to_version().return_const(4u32);

        registry.register((1, 2), Box::new(mock1));
        registry.register((3, 4), Box::new(mock2));

        // When Finding a migration path between disconnected versions
        let path = registry.find_migration_path(1, 4);

        // Then No path is found
        assert!(path.is_none());
    }

    #[test]
    fn test_migrate_project_same_version() {
        // Given A registry and content with the same source and target version
        let registry = MigrationRegistry::new(vec![]);
        let content: Box<dyn Any> = Box::new(42u32);

        // When Migrating to the same version
        let result = registry.migrate_project(content, 1, 1);

        // Then The content is returned unchanged
        assert!(result.is_ok());
        let migrated = result.unwrap();
        assert_eq!(*migrated.downcast::<u32>().unwrap(), 42);
    }

    #[test]
    fn test_migrate_project_direct_path() {
        // Given A registry with a direct migration path
        let mut registry = MigrationRegistry::new(vec![]);

        let mut mock = MockProjectMigrator::new();
        mock.expect_current_version().return_const(1u32);
        mock.expect_to_version().return_const(2u32);
        mock.expect_migrate()
            .with(predicate::always())
            .times(1)
            .returning(|_| Ok(Box::new(43u32)));

        registry.register((1, 2), Box::new(mock));
        let content: Box<dyn Any> = Box::new(42u32);

        // When Migrating along the path
        let result = registry.migrate_project(content, 1, 2);

        // Then The content is migrated correctly
        assert!(result.is_ok());
        let migrated = result.unwrap();
        assert_eq!(*migrated.downcast::<u32>().unwrap(), 43);
    }

    #[test]
    fn test_migrate_project_multi_step() {
        // Given A registry with a multi-step migration path
        let mut registry = MigrationRegistry::new(vec![]);

        let mut mock1 = MockProjectMigrator::new();
        mock1.expect_current_version().return_const(1u32);
        mock1.expect_to_version().return_const(2u32);
        mock1.expect_migrate()
            .with(predicate::always())
            .times(1)
            .returning(|_| Ok(Box::new(43u32)));

        let mut mock2 = MockProjectMigrator::new();
        mock2.expect_current_version().return_const(2u32);
        mock2.expect_to_version().return_const(3u32);
        mock2.expect_migrate()
            .with(predicate::always())
            .times(1)
            .returning(|_| Ok(Box::new(44u32)));

        registry.register((1, 2), Box::new(mock1));
        registry.register((2, 3), Box::new(mock2));
        let content: Box<dyn Any> = Box::new(42u32);

        // When Migrating along the path
        let result = registry.migrate_project(content, 1, 3);

        // Then The content is migrated correctly through all steps
        assert!(result.is_ok());
        let migrated = result.unwrap();
        assert_eq!(*migrated.downcast::<u32>().unwrap(), 44);
    }

    #[test]
    fn test_migrate_project_no_path() {
        // Given A registry with no path between versions
        let registry = MigrationRegistry::new(vec![]);
        let content: Box<dyn Any> = Box::new(42u32);

        // When Attempting to migrate
        let result = registry.migrate_project(content, 1, 2);

        // Then An error is returned
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Failed to find migration path"));
    }

    #[test]
    fn test_migrate_project_migration_error() {
        // Given A registry with a migrator that returns an error
        let mut registry = MigrationRegistry::new(vec![]);

        let mut mock = MockProjectMigrator::new();
        mock.expect_current_version().return_const(1u32);
        mock.expect_to_version().return_const(2u32);
        mock.expect_migrate()
            .with(predicate::always())
            .times(1)
            .returning(|_| Err(anyhow!("Migration failed")));

        registry.register((1, 2), Box::new(mock));
        let content: Box<dyn Any> = Box::new(42u32);

        let result = registry.migrate_project(content, 1, 2);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Failed"));
    }
}
