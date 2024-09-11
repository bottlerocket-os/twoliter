use oci_cli_wrapper::DockerArchitecture;
use serde::de::Error;
use serde::{Deserialize, Deserializer};
use std::fmt::{Display, Formatter};

#[derive(Deserialize, Debug)]
pub(crate) struct ManifestListView {
    pub manifests: Vec<ManifestView>,
}

#[derive(Deserialize, Debug, Clone)]
pub(crate) struct ManifestView {
    pub digest: String,
    pub platform: Option<Platform>,
}

#[derive(Deserialize, Debug, Clone)]
pub(crate) struct Platform {
    pub architecture: DockerArchitecture,
}

#[derive(Deserialize, Debug)]
pub(crate) struct IndexView {
    pub manifests: Vec<ManifestView>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct ManifestLayoutView {
    pub layers: Vec<Layer>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct Layer {
    pub digest: ContainerDigest,
}

#[derive(Debug)]
pub(crate) struct ContainerDigest(String);

impl<'de> Deserialize<'de> for ContainerDigest {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let digest = String::deserialize(deserializer)?;
        if !digest.starts_with("sha256:") {
            return Err(D::Error::custom(format!(
                "invalid digest detected in layer: {}",
                digest
            )));
        };
        Ok(Self(digest))
    }
}

impl Display for ContainerDigest {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.as_str())
    }
}
