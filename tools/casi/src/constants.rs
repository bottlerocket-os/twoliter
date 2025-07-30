//! Constants used throughout the casi crate.
//!
//! This module centralizes magic numbers and commonly used values to improve
//! code maintainability and reduce the likelihood of errors.

/// Default buffer size for I/O operations (64KB)
pub const DEFAULT_BUFFER_SIZE: usize = 64 * 1024;

/// Buffer size for hashing operations (128MB)
/// Uses 8KB chunks which align with standard memory page sizes for optimal
/// performance when reading from files and network streams.
pub const HASH_BUFFER_SIZE: usize = 128 * 1024 * 1024;

/// Maximum number of multipart upload parts for S3
pub const MAX_MULTIPART_PARTS: usize = 10_000;

/// Minimum size for S3 multipart upload parts (5MB)
pub const MIN_MULTIPART_PART_SIZE: usize = 5 * 1024 * 1024;

/// Default compression level for Zstd
pub const DEFAULT_COMPRESSION_LEVEL: i32 = 3;

/// Maximum file name length for cross-platform compatibility
pub const MAX_FILENAME_LENGTH: usize = 255;

/// Default timeout for network operations (30 seconds)
pub const DEFAULT_NETWORK_TIMEOUT_SECS: u64 = 30;

/// Maximum retry attempts for transient failures
pub const MAX_RETRY_ATTEMPTS: usize = 3;

/// Default page size for listing operations
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// Maximum artifacts to display in table format
pub const MAX_TABLE_DISPLAY_ARTIFACTS: usize = 1000;

/// Hash truncation length for display purposes
pub const HASH_DISPLAY_LENGTH: usize = 16;

/// Schema version for OCI manifests
pub const OCI_MANIFEST_SCHEMA_VERSION: u32 = 2;

/// Default file permissions for created files (0o644)
pub const DEFAULT_FILE_PERMISSIONS: u32 = 0o644;

/// Default directory permissions for created directories (0o755)
pub const DEFAULT_DIR_PERMISSIONS: u32 = 0o755;
