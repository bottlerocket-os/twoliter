use anyhow::Result;
use clap::Parser;
use std::path::{Path, PathBuf};

/// Serve the file descriptor for a path over an abstract UNIX domain socket.
#[derive(Clone, Debug, Parser)]
pub struct Server {
    /// Listen on this abstract socket.
    #[clap(long = "socket")]
    socket: String,

    /// Expect clients with this UID.
    #[clap(long = "client-uid")]
    client_uid: u32,

    /// Send file descriptor for this path.
    #[clap(long = "path")]
    path: PathBuf,
}

impl Server {
    pub fn for_path<S, P>(_: S, _: u32, _: P) -> Self
    where
        S: AsRef<str>,
        P: AsRef<Path>,
    {
        unimplemented!("pipesys is not supported on this operating system");
    }

    pub async fn serve(&self) -> Result<()> {
        unimplemented!("pipesys is not supported on this operating system");
    }
}
