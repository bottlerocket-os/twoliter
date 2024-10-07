use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use pentacle::SealOptions;

const COMPRESSED_KRANE_BIN: &[u8] = include_bytes!(env!("KRANE_GZ_PATH"));

lazy_static::lazy_static! {
    pub static ref KRANE: Krane = Krane::seal().unwrap();
}

#[derive(Debug)]
pub struct Krane {
    // Hold the file in memory to keep the fd open
    _sealed_binary: File,
    path: PathBuf,
}

impl Krane {
    fn seal() -> Result<Krane> {
        let mut krane_reader = GzDecoder::new(COMPRESSED_KRANE_BIN);

        let sealed_binary = SealOptions::new()
            .close_on_exec(false)
            .executable(true)
            .copy_and_seal(&mut krane_reader)
            .context("Failed to write krane binary to sealed anonymous file")?;

        let fd = sealed_binary.as_raw_fd();
        let pid = std::process::id();
        let path = PathBuf::from(format!("/proc/{pid}/fd/{fd}"));

        Ok(Krane {
            _sealed_binary: sealed_binary,
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::process::Command;

    #[test]
    fn test_krane_runs() {
        let status = Command::new(KRANE.path())
            .arg("--help")
            .output()
            .expect("failed to run krane");

        assert_eq!(status.status.code().unwrap(), 0);
    }
}
