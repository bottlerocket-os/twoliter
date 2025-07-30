//! Console logging system for casi.
//!
//! This module implements structured logging
//! for CAS operations using tracing and owo-colors crates.
//! The system provides clear feedback for operations
//! while separating data output from logging information.
//!
//! ## Core Components
//!
//! - [`LogCoordinator`] - Central coordinator for logging initialization
//! - [`LogVerbosity`] - Verbosity level configuration for different logging targets
//! - [`TraceContext`] - Enhanced trace context for operation correlation
//! - [`TraceEvent`] - Structured event types for consistent logging
//!
//! ## Usage
//!
//! ```rust
//! use casi::logging::{LogCoordinator, LogVerbosity};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Initialize logging system
//! let log_manager = LogCoordinator::init(LogVerbosity::Info, false).await?;
//!
//! // Check verbosity level
//! println!("Verbosity: {:?}", log_manager.verbosity());
//! println!("Quiet mode: {}", log_manager.is_quiet());
//!
//! println!("✓ Logging system initialized");
//! # Ok(())
//! # }
//! ```

pub mod context;
pub mod coordinator;
pub mod formatter;
pub mod macros;
pub mod visitor;

// Re-export main types for convenience
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub use context::{
    ByteCount, ErrorCategory, ErrorContext, PerformanceMetrics, Throughput, TraceContext,
    byte_count,
};
pub use coordinator::LogCoordinator;
pub use formatter::CasFormatter;

/// Verbosity levels for logging output.
///
/// Controls the level of detail in log messages and which events are displayed.
/// Higher verbosity levels include all messages from lower levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum LogVerbosity {
    /// Only errors and critical information
    Error,
    /// Warnings and errors
    Warn,
    /// Standard informational messages (default)
    #[default]
    Info,
    /// Detailed debugging information
    Debug,
    /// Comprehensive trace information including internal operations
    Trace,
    /// All events including internal operations and memory allocations
    All,
}

impl LogVerbosity {
    /// Convert to tracing filter string for subscriber configuration
    pub fn as_filter_string(&self) -> &'static str {
        match self {
            LogVerbosity::Error => "error",
            LogVerbosity::Warn => "warn",
            LogVerbosity::Info => "info",
            LogVerbosity::Debug => "debug",
            LogVerbosity::Trace => "trace",
            LogVerbosity::All => "trace", // Use trace as the highest tracing level
        }
    }

    /// Get a human-readable description of the verbosity level
    pub fn description(&self) -> &'static str {
        match self {
            LogVerbosity::Error => "Only critical errors and failures",
            LogVerbosity::Warn => "Warnings and errors",
            LogVerbosity::Info => "Standard operational information",
            LogVerbosity::Debug => "Detailed debugging information",
            LogVerbosity::Trace => "Comprehensive trace information",
            LogVerbosity::All => "All events including internal operations",
        }
    }

    /// Check if this verbosity level includes another level
    pub fn includes(&self, other: LogVerbosity) -> bool {
        use LogVerbosity::*;
        match self {
            Error => matches!(other, Error),
            Warn => matches!(other, Error | Warn),
            Info => matches!(other, Error | Warn | Info),
            Debug => matches!(other, Error | Warn | Info | Debug),
            Trace => matches!(other, Error | Warn | Info | Debug | Trace),
            All => true,
        }
    }
}

/// Result of an operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationResult {
    /// Whether the operation was successful
    pub success: bool,
    /// Result message or summary
    pub message: Option<String>,
    /// Output data from the operation
    pub data: HashMap<String, serde_json::Value>,
    /// Warnings encountered during the operation
    pub warnings: Vec<String>,
}

impl OperationResult {
    /// Create a successful result.
    pub fn success() -> Self {
        Self {
            success: true,
            message: None,
            data: HashMap::new(),
            warnings: Vec::new(),
        }
    }

    /// Create a successful result with a message.
    pub fn success_with_message(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: Some(message.into()),
            data: HashMap::new(),
            warnings: Vec::new(),
        }
    }

    /// Create a failure result.
    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: Some(message.into()),
            data: HashMap::new(),
            warnings: Vec::new(),
        }
    }

    /// Add data to the result.
    pub fn with_data(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.data.insert(key.into(), value);
        self
    }

    /// Add a warning to the result.
    pub fn with_warning(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }
}

#[macro_export]
macro_rules! success {
    (fields($($field: ident = $value: expr),*), $message: literal, $($arg: expr),*) => {
        tracing::info!(
            $($field = $value),*,
            "{}",
            format!($message, $($arg),*).if_supports_color(owo_colors::Stream::Stderr, |text| text.bold().bright_green().to_string())
        )
    };
    (fields($($field: ident = $value: expr),*), $message: literal) => {
        tracing::info!(
            $($field = $value),*,
            "{}",
            format!($message).if_supports_color(owo_colors::Stream::Stderr, |text| text.bold().bright_green().to_string())
        )
    };
}

#[macro_export]
macro_rules! failure {
    (fields($($field: ident = $value: expr),*), $message: literal, $($arg: expr),*) => {
        tracing::error!(
            $($field = $value),*,
            "{}",
            format!($message, $($arg),*).if_supports_color(owo_colors::Stream::Stderr, |text| text.bold().bright_red().to_string())
        )
    };
    (fields($($field: ident = $value: expr),*), $message: literal) => {
        tracing::error!(
            $($field = $value),*,
            "{}",
            format!($message).if_supports_color(owo_colors::Stream::Stderr, |text| text.bold().bright_red().to_string())
        )
    };
}
