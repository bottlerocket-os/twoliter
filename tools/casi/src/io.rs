//! I/O utilities for content hashing and streaming operations.
//!
//! This module provides utilities for computing SHA-256 hashes of streaming
//! data and thread-safe wrappers for async readers and writers. All hashing
//! operations use the SHA-256 algorithm for content-addressable storage.
use crate::error::{self, Result};
use async_compression::tokio::{bufread as zstd_read, write as zstd_write};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use snafu::ResultExt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufRead, AsyncRead, AsyncWrite};
use tracing::{instrument, trace};
use tracing_indicatif::span_ext::IndicatifSpanExt;

/// Common trait for IO components that support progress tracking
trait ProgressTracker {
    /// Update the progress by the specified number of bytes
    fn update_progress(&mut self, bytes: u64);
}

struct Progress {
    span: Option<tracing::Span>,
    bytes: usize,
    start: Instant,
}

/// Computes SHA-256 hash and size of data from an async reader.
///
/// Reads all data from the provided reader in 8KB chunks, computing the SHA-256 hash
/// and tracking the total number of bytes processed. Returns the hex-encoded hash
/// and byte count for content-addressable storage operations.
#[instrument(level = "trace", skip(reader))]
pub async fn compute_sha256_hash<R: AsyncRead + Unpin>(reader: &mut R) -> Result<(String, u64)> {
    let mut hasher = AsyncSha256::new();
    tokio::io::copy(reader, &mut hasher)
        .await
        .context(error::HashingReadSnafu)?;
    let size = hasher.size();
    let hash = hasher.to_hash();
    trace!("SHA-256 hash computation completed, hash: {}", &hash[..16]);

    Ok((hash, size))
}

/// Thread-safe wrapper for async writers with Send + Sync guarantees.
///
/// Provides a cloneable wrapper around async writers that can be safely
/// shared across threads. Uses Arc<Mutex<>> for interior mutability
/// while maintaining async compatibility.
#[derive(Clone)]
pub struct Writer {
    /// Thread-safe interior containing the async writer
    inner: Arc<Mutex<InnerIO<dyn AsyncWrite>>>,
}

unsafe impl Send for Writer {}
unsafe impl Sync for Writer {}

/// Internal wrapper for pinned async I/O components with progress tracking.
struct InnerIO<T: ?Sized> {
    io_component: Pin<Box<T>>,
    progress: Progress,
}

impl<T: ?Sized> ProgressTracker for InnerIO<T> {
    fn update_progress(&mut self, bytes: u64) {
        self.progress.bytes += bytes as usize;
        if let Some(span) = self.progress.span.as_mut() {
            span.pb_inc(bytes);
        }
    }
}

impl Writer {
    /// Creates a new thread-safe writer wrapper.
    #[allow(clippy::arc_with_non_send_sync)] // Clippy does not detect the send_guard feature on parking_lot
    pub fn new(writer: impl tokio::io::AsyncWrite + 'static, span: Option<tracing::Span>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(InnerIO {
                io_component: Box::pin(writer),
                progress: Progress {
                    bytes: 0,
                    start: Instant::now(),
                    span,
                },
            })),
        }
    }

    /// Creates a new thread-safe writer with zstd encoding
    #[allow(clippy::arc_with_non_send_sync)] // Clippy does not detect the send_guard feature on parking_lot
    pub fn with_encode(
        writer: impl tokio::io::AsyncWrite + 'static,
        span: Option<tracing::Span>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(InnerIO {
                io_component: Box::pin(zstd_write::ZstdEncoder::new(writer)),
                progress: Progress {
                    bytes: 0,
                    start: Instant::now(),
                    span,
                },
            })),
        }
    }

    /// Creates a new thread-safe writer with zstd decoding
    #[allow(clippy::arc_with_non_send_sync)] // Clippy does not detect the send_guard feature on parking_lot
    pub fn with_decode(
        writer: impl tokio::io::AsyncWrite + 'static,
        span: Option<tracing::Span>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(InnerIO {
                io_component: Box::pin(zstd_write::ZstdDecoder::new(writer)),
                progress: Progress {
                    bytes: 0,
                    start: Instant::now(),
                    span,
                },
            })),
        }
    }

    pub fn progress_bytes(&self) -> usize {
        self.inner.lock().progress.bytes
    }

    pub fn elapsed(&self) -> Duration {
        self.inner.lock().progress.start.elapsed()
    }
}

impl tokio::io::AsyncWrite for Writer {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::result::Result<usize, std::io::Error>> {
        let mut this = self.get_mut().inner.lock();
        match this.io_component.as_mut().poll_write(cx, buf) {
            Poll::Ready(Ok(size)) => {
                this.update_progress(size as u64);
                Poll::Ready(Ok(size))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), std::io::Error>> {
        self.get_mut()
            .inner
            .lock()
            .io_component
            .as_mut()
            .poll_flush(cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), std::io::Error>> {
        self.get_mut()
            .inner
            .lock()
            .io_component
            .as_mut()
            .poll_shutdown(cx)
    }
}

/// Thread-safe wrapper for async readers with Send + Sync guarantees.
///
/// Provides a cloneable wrapper around async readers that can be safely
/// shared across threads. Uses Arc<Mutex<>> for interior mutability
/// while maintaining async compatibility for storage backend operations.
#[derive(Clone)]
pub struct Reader {
    /// Thread-safe interior containing the async reader
    inner: Arc<Mutex<InnerIO<dyn AsyncRead>>>,
}

unsafe impl Send for Reader {}
unsafe impl Sync for Reader {}

impl Reader {
    /// Creates a new thread-safe reader wrapper.
    #[allow(clippy::arc_with_non_send_sync)] // Clippy does not detect the send_guard feature on parking_lot
    pub fn new(reader: impl AsyncRead + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(InnerIO {
                io_component: Box::pin(reader),
                progress: Progress {
                    bytes: 0,
                    start: Instant::now(),
                    span: None,
                },
            })),
        }
    }

    /// Creates a new thread-safe reader wrapper with zstd decoding
    #[allow(clippy::arc_with_non_send_sync)] // Clippy does not detect the send_guard feature on parking_lot
    pub fn with_decode(reader: impl AsyncBufRead + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(InnerIO {
                io_component: Box::pin(zstd_read::ZstdDecoder::new(reader)),
                progress: Progress {
                    bytes: 0,
                    start: Instant::now(),
                    span: None,
                },
            })),
        }
    }

    /// Creates a new thread-safe reader wrapper with zstd encoding
    #[allow(clippy::arc_with_non_send_sync)] // Clippy does not detect the send_guard feature on parking_lot
    pub fn with_encode(reader: impl AsyncBufRead + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(InnerIO {
                io_component: Box::pin(zstd_read::ZstdEncoder::new(reader)),
                progress: Progress {
                    bytes: 0,
                    start: Instant::now(),
                    span: None,
                },
            })),
        }
    }

    pub fn set_span(&self, span: tracing::Span) {
        self.inner.lock().progress.span = Some(span);
    }
}

impl tokio::io::AsyncRead for Reader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let mut this = self.get_mut().inner.lock();
        match this.io_component.as_mut().poll_read(cx, buf) {
            Poll::Ready(Ok(_)) => {
                if buf.remaining() == 0 {
                    this.update_progress(buf.filled().len() as u64);
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }
}

struct AsyncSha256 {
    digest: Sha256,
    size: u64,
}

impl AsyncSha256 {
    fn new() -> Self {
        Self {
            digest: Sha256::new(),
            size: 0,
        }
    }

    fn to_hash(&self) -> String {
        let result = self.digest.clone().finalize();
        hex::encode(result.as_slice())
    }

    fn size(&self) -> u64 {
        self.size
    }
}

impl AsyncWrite for AsyncSha256 {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<std::result::Result<usize, std::io::Error>> {
        let this = self.get_mut();
        this.digest.update(buf);
        this.size += buf.len() as u64;
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<std::result::Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<std::result::Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::HASH_BUFFER_SIZE;
    use std::io::Cursor;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn test_compute_sha256_hash_empty_input() {
        // Arrange
        let mut reader = Cursor::new(b"");

        // Act
        let (hash, size) = compute_sha256_hash(&mut reader).await.unwrap();

        // Assert
        assert_eq!(size, 0);
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[tokio::test]
    async fn test_compute_sha256_hash_known_input() {
        // Arrange
        let test_data = b"hello world";
        let mut reader = Cursor::new(test_data);

        // Act
        let (hash, size) = compute_sha256_hash(&mut reader).await.unwrap();

        // Assert
        assert_eq!(size, test_data.len() as u64);
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[tokio::test]
    async fn test_compute_sha256_hash_large_input() {
        // Arrange - Create data larger than buffer size
        let test_data = vec![0u8; HASH_BUFFER_SIZE * 2 + 100];
        let mut reader = Cursor::new(&test_data);

        // Act
        let (hash, size) = compute_sha256_hash(&mut reader).await.unwrap();

        // Assert
        assert_eq!(size, test_data.len() as u64);
        // Hash of all zeros with this length
        assert!(!hash.is_empty());
        assert_eq!(hash.len(), 64); // SHA-256 produces 64 hex characters
    }

    #[tokio::test]
    async fn test_reader_wrapper_functionality() {
        // Arrange
        let test_data = b"test data for reader";
        let cursor = Cursor::new(test_data);
        let mut reader = Reader::new(cursor);

        // Act
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer).await.unwrap();

        // Assert
        assert_eq!(buffer, test_data);
    }

    #[tokio::test]
    async fn test_reader_wrapper_clone() {
        // Arrange
        let test_data = b"cloneable data";
        let cursor = Cursor::new(test_data);
        let reader = Reader::new(cursor);

        // Act
        let cloned_reader = reader.clone();

        // Assert - Both readers should share the same Arc (this tests the Clone implementation)
        assert!(Arc::ptr_eq(&reader.inner, &cloned_reader.inner));
    }
}
