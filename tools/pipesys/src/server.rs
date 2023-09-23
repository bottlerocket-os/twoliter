use anyhow::{Context, Result};
use clap::Parser;
use log::warn;
use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use uds::{tokio::UnixSeqpacketListener, UnixSocketAddr};

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
    pub fn for_path<S, P>(socket: S, client_uid: u32, path: P) -> Self
    where
        S: AsRef<str>,
        P: AsRef<Path>,
    {
        let socket = socket.as_ref().to_string();
        let path = path.as_ref().into();

        Self {
            socket,
            client_uid,
            path,
        }
    }

    pub async fn serve(&self) -> Result<()> {
        let addr = UnixSocketAddr::from_abstract(self.socket.as_bytes())
            .with_context(|| format!("failed to create socket {}", self.socket))?;
        let mut listener = UnixSeqpacketListener::bind_addr(&addr)
            .with_context(|| format!("failed to bind to socket {}", self.socket))?;

        let f = OpenOptions::new()
            .create(false)
            .read(true)
            .write(false)
            .open(&self.path)
            .with_context(|| format!("could not open {}", self.path.display()))?;

        let fd = f.as_raw_fd();

        loop {
            let (mut conn, _) = listener.accept().await.with_context(|| {
                format!("failed to accept connection on socket {}", self.socket)
            })?;

            let peer_creds = conn.initial_peer_credentials().with_context(|| {
                format!(
                    "failed to obtain peer credentials on socket {}",
                    self.socket
                )
            })?;

            let peer_uid = peer_creds.euid();
            if peer_uid != self.client_uid {
                warn!("ignoring connection from peer with UID {}", peer_uid);
                continue;
            }

            let socket = self.socket.clone();
            let fds = vec![fd];
            tokio::spawn(async move {
                conn.send_fds(b"fds", &fds)
                    .await
                    .with_context(|| format!("failed to send file descriptors over {}", socket))
            });
        }
    }
}
