//! Common error handling for pcrsys.

use snafu::Whatever;

/// Result type for pcrsys operations.
pub type Result<T> = std::result::Result<T, Whatever>;
