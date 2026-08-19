//! Common error handling for ukisys.

use snafu::Whatever;

/// Result type for ukisys CLI-level operations.
pub type Result<T> = std::result::Result<T, Whatever>;
