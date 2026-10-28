use snafu::Snafu;
use std::path::PathBuf;
use std::process::ExitStatus;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(super)))]
pub(crate) enum Error {
    #[snafu(display("Failed to create directory '{}': {}", path.display(), source))]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to generate the module-signing key ({status}): {stderr}"))]
    KeyGen { status: ExitStatus, stderr: String },

    #[snafu(display("Generating the module-signing key printed no {what}"))]
    KeyGenOutput { what: &'static str },

    #[snafu(display("Failed to run docker to generate the module-signing key: {}", source))]
    KeyGenStart { source: std::io::Error },

    #[snafu(display("Interrupted by a signal"))]
    Interrupted,

    #[snafu(display("Failed to lock '{}': {}", path.display(), source))]
    Lock { path: PathBuf, source: nix::Error },

    #[snafu(display("No command given after --"))]
    NoCommand,

    #[snafu(display(
        "No tmpfs for the module-signing key: XDG_RUNTIME_DIR is unset, not a private 0700 \
        directory, or not on tmpfs, and /dev/shm is not a tmpfs"
    ))]
    NoTmpfs,

    #[snafu(display(
        "'{}' must be a directory owned by UID {} with mode 0700; remove it or fix it",
        path.display(),
        uid
    ))]
    NotPrivate { path: PathBuf, uid: u32 },

    #[snafu(display("Failed to open '{}': {}", path.display(), source))]
    OpenDir {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display(
        "Failed to list the packages that set module-signing = true: {}",
        source
    ))]
    Packages { source: buildsys::manifest::Error },

    #[snafu(display("Failed to run '{}': {}", program, source))]
    Run {
        program: String,
        source: std::io::Error,
    },

    #[snafu(display("Failed to handle signals: {}", source))]
    Signals { source: std::io::Error },

    #[snafu(display("Failed to write '{}': {}", path.display(), source))]
    WriteKey {
        path: PathBuf,
        source: std::io::Error,
    },
}

pub(super) type Result<T> = std::result::Result<T, Error>;
