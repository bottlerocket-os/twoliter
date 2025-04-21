use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::ops::Deref;
use std::path::{Path, PathBuf};

/// Represents data that has been loaded from the filesystem at a given path.
///
/// Useful for adding path information to error messages.
/// Implements `Deref<Target=T>` for ergonomic purposes.
pub enum FromPath<T> {
    FromPath { data: T, path: PathBuf },
    NoPath(T),
}

impl<T> FromPath<T> {
    pub fn new_from_path(data: T, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        Self::FromPath { path, data }
    }

    pub fn new(data: T) -> Self {
        Self::NoPath(data)
    }

    pub fn data(&self) -> &T {
        match self {
            Self::FromPath { data, .. } => data,
            Self::NoPath(data) => data,
        }
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::FromPath { path, .. } => Some(path),
            Self::NoPath(_) => None,
        }
    }

    pub fn display_path(&self) -> String {
        match self {
            Self::FromPath { path, .. } => path.display().to_string(),
            Self::NoPath(_) => "<no path>".to_string(),
        }
    }
}

impl<T> Deref for FromPath<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.data()
    }
}

impl<T: Debug> Debug for FromPath<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FromPath { path, data } => f
                .debug_struct("FromPath")
                .field("path", path)
                .field("data", data)
                .finish(),
            Self::NoPath(data) => f.debug_tuple("FromPath").field(data).finish(),
        }
    }
}

impl<T: Clone> Clone for FromPath<T> {
    fn clone(&self) -> Self {
        match self {
            Self::FromPath { path, data } => Self::FromPath {
                path: path.clone(),
                data: data.clone(),
            },
            Self::NoPath(data) => Self::NoPath(data.clone()),
        }
    }
}

impl<T: Serialize> Serialize for FromPath<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.data().serialize(serializer)
    }
}

impl<'de, T> FromPath<T>
where
    T: Deserialize<'de>,
{
    pub fn deserialize_from_path<D>(
        path: impl Into<PathBuf>,
        deserializer: D,
    ) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let path = path.into();
        let data = T::deserialize(deserializer)?;
        Ok(Self::FromPath { path, data })
    }
}

impl<T: Default> Default for FromPath<T> {
    fn default() -> Self {
        Self::NoPath(T::default())
    }
}

impl<T> From<T> for FromPath<T> {
    fn from(value: T) -> Self {
        Self::NoPath(value)
    }
}
