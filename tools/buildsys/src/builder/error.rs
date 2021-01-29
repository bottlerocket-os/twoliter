use snafu::Snafu;
use std::path::PathBuf;

#[derive(Debug, Snafu)]
#[snafu(visibility = "pub(super)")]
pub(crate) enum Error {
    #[snafu(display("Failed to start command: {}", source))]
    CommandStart { source: std::io::Error },

    #[snafu(display("Failed to execute command: 'docker {}'", args))]
    DockerExecution { args: String },

    #[snafu(display("Failed to change directory to '{}': {}", path.display(), source))]
    DirectoryChange {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to get filename for '{}'", path.display()))]
    BadFilename { path: PathBuf },

    #[snafu(display("Failed to create directory '{}': {}", path.display(), source))]
    DirectoryCreate {
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

    #[snafu(display("Missing environment variable '{}'", var))]
    Environment {
        var: String,
        source: std::env::VarError,
    },
}

pub(super) type Result<T> = std::result::Result<T, Error>;
