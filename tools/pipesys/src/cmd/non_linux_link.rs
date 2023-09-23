use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

/// Retrieve a file descriptor from an abstract socket, and set up a
/// symlink to it that provides access to subsequent processes until
/// the symlink is removed.
#[derive(Debug, Parser)]
pub(crate) struct Link {
    /// Fetch the file descriptor for a path from this abstract socket.
    #[clap(long = "fd-socket")]
    fd_socket: String,

    /// Create this target path as a symlink to the file descriptor.
    #[clap(long = "target")]
    target: PathBuf,
}

impl Link {
    /// Fail loudly on non-Linux.
    pub(crate) async fn execute(&self) -> Result<()> {
        unimplemented!("pipesys does not support this operating system.")
    }
}
