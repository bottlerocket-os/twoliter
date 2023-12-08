use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// We need to constrain the `Project` struct to a valid version. Unfortunately `serde` does not
/// have an after-deserialization validation hook, so we have this struct to limit the version to a
/// single acceptable value.
#[derive(Default, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct SchemaVersion<const N: u32>;

impl<const N: u32> SchemaVersion<N> {
    pub(crate) fn get(&self) -> u32 {
        N
    }

    pub(crate) fn get_static() -> u32 {
        N
    }
}

impl<const N: u32> From<SchemaVersion<N>> for u32 {
    fn from(value: SchemaVersion<N>) -> Self {
        value.get()
    }
}

impl<const N: u32> fmt::Debug for SchemaVersion<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        fmt::Debug::fmt(&self.get(), f)
    }
}

impl<const N: u32> fmt::Display for SchemaVersion<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        fmt::Display::fmt(&self.get(), f)
    }
}

impl<const N: u32> Serialize for SchemaVersion<N> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u32(self.get())
    }
}

impl<'de, const N: u32> Deserialize<'de> for SchemaVersion<N> {
    fn deserialize<D>(deserializer: D) -> Result<SchemaVersion<N>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value: u32 = Deserialize::deserialize(deserializer)?;
        if value != Self::get_static() {
            Err(Error::custom(format!(
                "Incorrect project schema_version: got '{}', expected '{}'",
                value,
                Self::get_static()
            )))
        } else {
            Ok(Self)
        }
    }
}
