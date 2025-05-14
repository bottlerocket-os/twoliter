pub use include_env_compressed_macro::include_archive_from_env;

#[derive(Debug, Clone, Copy)]
pub struct Archive {
    kind: ArchiveKind,
    data: &'static [u8],
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ArchiveKind {
    Zstd,
    Uncompressed,
}

impl Archive {
    /// Creates an archive from presumably zstd-compressed data.
    ///
    /// This interface is intended to be used by the `include_archive_from_env!` macro.
    /// Will panic on read if data is not zstd-compressed.
    pub const fn zstd(data: &'static [u8]) -> Self {
        Self {
            kind: ArchiveKind::Zstd,
            data,
        }
    }

    /// Creates an archive from uncompressed data.
    ///
    /// This interface is intended to be used by the `include_archive_from_env!` macro.
    pub const fn uncompressed(data: &'static [u8]) -> Self {
        Self {
            kind: ArchiveKind::Uncompressed,
            data,
        }
    }

    pub fn kind(&self) -> ArchiveKind {
        self.kind
    }

    /// `Read` the data contained within this Archived, whether or not the source archive is
    /// compressed.
    pub fn reader(&self) -> Box<dyn std::io::Read + Send + Sync + 'static> {
        match self.kind {
            ArchiveKind::Zstd => Box::new(zstd::Decoder::new(self.data).unwrap()) as _,
            ArchiveKind::Uncompressed => Box::new(self.data) as _,
        }
    }
}
