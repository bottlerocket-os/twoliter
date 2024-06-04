use anyhow::anyhow;
use serde::Deserialize;
use std::fmt::{Display, Formatter};

pub const EXTERNAL_KIT_DIRECTORY: &str = "build/external-kits";
pub const EXTERNAL_KIT_METADATA: &str = "build/external-kits/external-kit-metadata.json";

#[derive(Deserialize, Debug, Clone, PartialEq)]
pub enum DockerArchitecture {
    Amd64,
    Arm64,
}

impl TryFrom<&str> for DockerArchitecture {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        match value {
            "x86_64" | "amd64" => Ok(DockerArchitecture::Amd64),
            "aarch64" | "arm64" => Ok(DockerArchitecture::Arm64),
            _ => Err(anyhow!("invalid architecture '{}'", value)),
        }
    }
}

impl Display for DockerArchitecture {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Amd64 => "amd64",
            Self::Arm64 => "arm64",
        })
    }
}
