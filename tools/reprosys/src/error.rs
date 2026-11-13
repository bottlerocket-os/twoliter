//! Common error handling for reprosys.

use snafu::Whatever;

/// Result type for reprosys operations.
pub type Result<T> = std::result::Result<T, Whatever>;
