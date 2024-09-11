//! ImageTool enablement library implements a standardized way of calling commandline container image
//! tools for interacting primarily with kit images in a container registry.
//!
//! Current two tools are supported:
//! * crane, gcrane, krane
//!     Crane provides a more direct interaction with the container registry,
//!     allowing us to query image information in the registry without having to pull the full image to
//!     disk. It also does not require a daemon to operate and has optimizations for pulling large images to disk
//! * docker
//!     Docker can perform all interactions we need with several caveats that make it less efficient than
//!     crane. The image needs to be pulled locally in order for docker to inspect the manifest and extract
//!     metadata. In addition, in order to operate with OCI image format, the containerd-snapshotter
//!     feature has to be enabled in the docker daemon
use std::fmt::{Display, Formatter};
use std::{collections::HashMap, env, path::Path};

use async_trait::async_trait;
use cli::CommandLine;
use crane::CraneCLI;
use docker::DockerCLI;
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use snafu::ResultExt;
use which::which;

mod cli;
mod crane;
mod docker;

#[derive(Debug)]
pub struct ImageTool {
    image_tool_impl: Box<dyn ImageToolImpl>,
}

impl ImageTool {
    /// Uses the container tool specified by the given tool name.
    ///
    /// The specified tool must be present in the unix search path.
    fn from_tool_name(tool_name: &str) -> Result<Self> {
        let image_tool_impl: Box<dyn ImageToolImpl> = match tool_name {
            "docker" => Box::new(DockerCLI {
                cli: CommandLine {
                    path: which("docker").context(error::NotFoundSnafu { name: "docker" })?,
                },
            }),
            tool @ ("crane" | "gcrane" | "krane") => Box::new(CraneCLI {
                cli: CommandLine {
                    path: which(tool).context(error::NotFoundSnafu { name: tool })?,
                },
            }),
            _ => return error::UnsupportedSnafu { name: tool_name }.fail(),
        };

        Ok(Self { image_tool_impl })
    }

    /// Auto-selects the container tool based on unix search path.
    ///
    /// Uses `crane` if available, falling back to `docker` otherwise.
    fn from_unix_search_path() -> Result<Self> {
        let crane = which("krane").or(which("gcrane")).or(which("crane"));
        let image_tool_impl: Box<dyn ImageToolImpl> = if let Ok(path) = crane {
            Box::new(CraneCLI {
                cli: CommandLine { path },
            })
        } else {
            Box::new(DockerCLI {
                cli: CommandLine {
                    path: which("docker").context(error::NoneFoundSnafu)?,
                },
            })
        };

        Ok(Self { image_tool_impl })
    }

    /// Auto-select the container tool to use by environment variable
    /// and-or auto detection.
    ///
    /// If TWOLITER_KIT_IMAGE_TOOL environment variable is set, uses that value.
    /// Valid values are:
    /// * docker
    /// * crane | gcrane | krane
    ///
    /// Otherwise, searches $PATH, using `crane` if available and falling back to docker otherwise.
    pub fn from_environment() -> Result<Self> {
        if let Ok(name) = env::var("TWOLITER_KIT_IMAGE_TOOL") {
            Self::from_tool_name(&name)
        } else {
            Self::from_unix_search_path()
        }
    }

    pub fn new(image_tool_impl: Box<dyn ImageToolImpl>) -> Self {
        Self { image_tool_impl }
    }

    /// Pull an image archive to disk
    pub async fn pull_oci_image(&self, path: &Path, uri: &str) -> Result<()> {
        self.image_tool_impl.pull_oci_image(path, uri).await
    }

    /// Fetch the image config
    pub async fn get_config(&self, uri: &str) -> Result<ConfigView> {
        self.image_tool_impl.get_config(uri).await
    }

    /// Fetch the manifest
    pub async fn get_manifest(&self, uri: &str) -> Result<Vec<u8>> {
        let manifest_bytes = self.image_tool_impl.get_manifest(uri).await?;
        let manifest_object: serde_json::Value =
            serde_json::from_slice(&manifest_bytes).context(error::ManifestDeserializeSnafu)?;

        let mut canonicalized_manifest = Vec::new();
        let mut ser = serde_json::Serializer::with_formatter(
            &mut canonicalized_manifest,
            CanonicalFormatter::new(),
        );

        manifest_object
            .serialize(&mut ser)
            .context(error::ManifestCanonicalizeSnafu)?;

        Ok(canonicalized_manifest)
    }

    /// Push a single-arch image in oci archive format
    pub async fn push_oci_archive(&self, path: &Path, uri: &str) -> Result<()> {
        self.image_tool_impl.push_oci_archive(path, uri).await
    }

    /// Push the multi-arch kit manifest list
    pub async fn push_multi_platform_manifest(
        &self,
        platform_images: Vec<(DockerArchitecture, String)>,
        uri: &str,
    ) -> Result<()> {
        self.image_tool_impl
            .push_multi_platform_manifest(platform_images, uri)
            .await
    }
}

#[async_trait]
pub trait ImageToolImpl: std::fmt::Debug + Send + Sync + 'static {
    /// Pull an image archive to disk
    async fn pull_oci_image(&self, path: &Path, uri: &str) -> Result<()>;
    /// Fetch the image config
    async fn get_config(&self, uri: &str) -> Result<ConfigView>;
    /// Fetch the manifest
    async fn get_manifest(&self, uri: &str) -> Result<Vec<u8>>;
    /// Push a single-arch image in oci archive format
    async fn push_oci_archive(&self, path: &Path, uri: &str) -> Result<()>;
    /// Push the multi-arch kit manifest list
    async fn push_multi_platform_manifest(
        &self,
        platform_images: Vec<(DockerArchitecture, String)>,
        uri: &str,
    ) -> Result<()>;
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum DockerArchitecture {
    Amd64,
    Arm64,
}

impl TryFrom<&str> for DockerArchitecture {
    type Error = error::Error;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        match value {
            "x86_64" | "amd64" => Ok(DockerArchitecture::Amd64),
            "aarch64" | "arm64" => Ok(DockerArchitecture::Arm64),
            _ => Err(error::Error::InvalidArchitecture {
                value: value.to_string(),
            }),
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

#[derive(Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
struct ImageView {
    config: ConfigView,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct ConfigView {
    pub labels: HashMap<String, String>,
}

pub type Result<T> = std::result::Result<T, error::Error>;

pub mod error {
    use std::path::PathBuf;

    use snafu::Snafu;

    #[derive(Snafu, Debug)]
    #[snafu(visibility(pub(super)))]
    pub enum Error {
        #[snafu(display("Failed to extract archive: {source}"))]
        ArchiveExtract { source: std::io::Error },

        #[snafu(display("Failed to read archive: {source}"))]
        ArchiveRead { source: std::io::Error },

        #[snafu(display("Failed to execute image tool, {message}: {source}"))]
        CommandFailed {
            message: String,
            source: std::io::Error,
        },

        #[snafu(display("Failed to deserialize image config: {source}"))]
        ConfigDeserialize { source: serde_json::Error },

        #[snafu(display("Failed to create temporary directory for crane push: {source}"))]
        CraneTemp { source: std::io::Error },

        #[snafu(display("Failed to create temporary directory for docker save: {source}"))]
        DockerTemp { source: std::io::Error },

        #[snafu(display("invalid architecture '{value}'"))]
        InvalidArchitecture { value: String },

        #[snafu(display("Failed to deserialize image manifest: {source}"))]
        ManifestDeserialize { source: serde_json::Error },

        #[snafu(display("Failed to canonicalize image manifest: {source}"))]
        ManifestCanonicalize { source: serde_json::Error },

        #[snafu(display("No digest returned by `docker load`"))]
        NoDigest,

        #[snafu(display(
            "Unable to find any supported container image tool, please install docker or crane: {}",
            source
        ))]
        NoneFound { source: which::Error },

        #[snafu(display(
            "Unable to find a container image tool by name '{}' in current environment",
            name
        ))]
        NotFound { name: String, source: which::Error },

        #[snafu(display("Failed to run operation with image tool: {message}\n command: {} {}", program.display(), args.join(" ")))]
        OperationFailed {
            message: String,
            program: PathBuf,
            args: Vec<String>,
        },

        #[snafu(display("Failed to parse kit filename: {}", source))]
        Regex { source: regex::Error },

        #[snafu(display("Unsupported container image tool '{}'", name))]
        Unsupported { name: String },
    }
}
