use snafu::Snafu;
use std::path::PathBuf;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(super)))]
pub(crate) enum Error {
    #[snafu(display("Failed to create async runtime: {}", source))]
    AsyncRuntime { source: std::io::Error },

    #[snafu(display("Failed to read CA certificates bundle '{}'", ca_bundle_path.display()))]
    BadCaBundle { ca_bundle_path: PathBuf },

    #[snafu(display("Failed to get file name for '{}'", path.display()))]
    BadFilename { path: PathBuf },

    #[snafu(display("Failed to read repo root '{}'", root_json_path.display()))]
    BadRootJson { root_json_path: PathBuf },

    #[snafu(display("Failed to start command: {}", source))]
    CommandStart { source: std::io::Error },

    #[snafu(display("Failed to execute command: 'docker {}'", args))]
    DockerExecution { args: String },

    #[snafu(display(
        "The installed docker ('{}') does not meet the minimum version requirement ('{}')",
        installed_version,
        required_version
    ))]
    DockerVersionRequirement {
        installed_version: semver::Version,
        required_version: semver::VersionReq,
    },

    #[snafu(display("Failed to change directory to '{}': {}", path.display(), source))]
    DirectoryChange {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to get parent directory for '{}'", path.display()))]
    BadDirectory { path: PathBuf },

    #[snafu(display("Failed to create directory '{}': {}", path.display(), source))]
    DirectoryCreate {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to create directory '{}': {}", path.display(), source))]
    DirectoryRemove {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to read directory '{}': {}", path.display(), source))]
    DirectoryRead {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to walk directory to find marker files: {}", source))]
    DirectoryWalk { source: walkdir::Error },

    #[snafu(display("Failed to create file '{}': {}", path.display(), source))]
    FileCreate {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to remove file '{}': {}", path.display(), source))]
    FileRemove {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to rename file '{}' to '{}': {}", old_path.display(), new_path.display(), source))]
    FileRename {
        old_path: PathBuf,
        new_path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to create build arguments due to a dependency error: {source}"))]
    Graph { source: buildsys::manifest::Error },

    #[snafu(display("Missing environment variable '{}'", var))]
    Environment {
        var: String,
        source: std::env::VarError,
    },

    #[snafu(display("Failed to strip prefix '{}' from path '{}': {}", prefix.display(), path.display(), source))]
    StripPathPrefix {
        path: PathBuf,
        prefix: PathBuf,
        source: std::path::StripPrefixError,
    },

    #[snafu(display("Failed to parse variant: {source}"))]
    VariantParse {
        source: bottlerocket_variant::error::Error,
    },

    #[snafu(display("Failed to parse version string '{version_str}': {source}"))]
    VersionParse {
        source: semver::Error,
        version_str: String,
    },

    #[snafu(display("Repacking is not supported for image-format = \"eif\""))]
    EifRepackUnsupported,
}

pub(super) type Result<T> = std::result::Result<T, Error>;
