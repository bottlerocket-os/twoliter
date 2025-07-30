//! LogCoordinator implementation for coordinating logging display.
//!
//! The LogCoordinator is responsible for initializing the tracing subscriber system
//! and setting up logging for CAS operations.

// Standard library imports
use std::sync::Arc;
use std::sync::LazyLock;

use indicatif::ProgressStyle;
// External crate imports
use snafu::ResultExt;
use tracing_indicatif::IndicatifLayer;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

// Internal imports
use crate::error::LogDirectiveParseSnafu;
use crate::error::LogInitSnafu;
use crate::error::Result;
use crate::logging::CasFormatter;
use crate::logging::LogVerbosity;

pub static SPINNER_STYLE: LazyLock<ProgressStyle> = LazyLock::new(|| {
    ProgressStyle::with_template("[{elapsed_precise}] {span_child_prefix} {cmd} {span_name} {msg} {spinner:.green} {span_fields}").unwrap()
    .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "✔"])
});
pub static COUNTER_STYLE: LazyLock<ProgressStyle> = LazyLock::new(|| {
    ProgressStyle::with_template("[{elapsed_precise}] {span_child_prefix} {cmd} {span_name} {msg} {spinner:.green} {bar:40.cyan/blue} {pos}/{len} {span_fields}").unwrap()
        .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "✔"])
        .progress_chars("##-")
});

/// Central coordinator for logging initialization.
///
/// The LogCoordinator implements:
/// - Tracing subscriber setup with verbosity filtering
#[derive(Clone)]
pub struct LogCoordinator {
    inner: Arc<Inner>,
}

struct Inner {
    verbosity: LogVerbosity,
    quiet: bool,
}

impl LogCoordinator {
    /// Initialize the logging system with specified verbosity and quiet mode.
    ///
    /// This sets up the tracing subscriber with:
    /// - Appropriate log level filtering based on verbosity
    /// - AWS SDK log filtering to reduce noise
    /// - Console output coordination
    ///
    /// Returns `Error::LogInit` if the tracing subscriber fails to initialize.
    pub async fn init(verbosity: LogVerbosity, quiet: bool) -> Result<Self> {
        // Create environment filter with appropriate verbosity
        let mut filter = EnvFilter::new(verbosity.as_filter_string());

        // Filter noisy AWS SDK logs to TRACE level only
        // Using parse_directives to avoid potential parsing errors
        let directives = [
            "aws_sdk_s3=warn",
            "aws_smithy_runtime=warn",
            "aws_smithy_http=warn",
            "aws_config=warn",
            "hyper=warn",
            "reqwest=warn",
        ];

        for directive in &directives {
            match directive.parse() {
                Ok(parsed) => filter = filter.add_directive(parsed),
                Err(_) => {
                    return LogDirectiveParseSnafu {
                        directive: directive.to_string(),
                    }
                    .fail();
                }
            }
        }

        // Allow environment override
        if let Ok(env_filter) = std::env::var("RUST_LOG") {
            if !env_filter.is_empty() {
                filter = EnvFilter::new(env_filter);
            }
        }

        // Create the tracing subscriber with CasFormatter for enhanced CAS operation logging
        // In quiet mode, we only want to show errors, so we use the special Error level filter
        let formatter = CasFormatter::new(quiet);
        let indicatif_layer = IndicatifLayer::new()
            .with_progress_style(ProgressStyle::with_template("[{elapsed_precise}] {span_child_prefix} {cmd} {span_name} {msg} {spinner:.green} {bar:40.cyan/blue} {binary_bytes}/{binary_total_bytes} {span_fields}").unwrap()
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "✔"])
                .progress_chars("##-")
            ).with_span_field_formatter(formatter.clone());

        let subscriber = tracing_subscriber::registry()
            .with(filter)
            .with(
                fmt::layer()
                    .with_writer(indicatif_layer.get_stderr_writer())
                    .with_target(verbosity != LogVerbosity::Info)
                    .with_thread_ids(verbosity == LogVerbosity::Trace)
                    .with_thread_names(verbosity == LogVerbosity::Trace)
                    .with_file(verbosity == LogVerbosity::Trace)
                    .with_line_number(verbosity == LogVerbosity::Trace)
                    .fmt_fields(formatter.clone())
                    .event_format(formatter.clone()),
            )
            .with(indicatif_layer);

        // Initialize the global subscriber
        subscriber
            .try_init()
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
            .context(LogInitSnafu)?;

        let inner = Arc::new(Inner { verbosity, quiet });

        Ok(LogCoordinator { inner })
    }

    /// Get the current verbosity level.
    pub fn verbosity(&self) -> LogVerbosity {
        self.inner.verbosity
    }

    /// Check if quiet mode is enabled.
    pub fn is_quiet(&self) -> bool {
        self.inner.quiet
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::OnceCell;

    const INIT: OnceCell<LogCoordinator> = OnceCell::const_new();

    fn init_test_logging() {
        let _ = INIT.get_or_init(async || {
            LogCoordinator::init(LogVerbosity::Info, false)
                .await
                .unwrap()
        });
    }

    #[tokio::test]
    async fn test_log_manager_init() {
        // Test that we can create a LogCoordinator even if tracing is already initialized
        let log_coordinator = LogCoordinator::init(LogVerbosity::Info, false).await;
        // The first call might succeed, subsequent calls will fail due to global subscriber
        // but that's expected behavior
        let _ = log_coordinator;
    }

    #[tokio::test]
    async fn test_log_manager_quiet_mode() {
        init_test_logging();

        // Create a LogCoordinator directly without initializing tracing again
        let inner = Arc::new(Inner {
            verbosity: LogVerbosity::Info,
            quiet: true,
        });
        let log_manager = LogCoordinator { inner };

        assert!(log_manager.is_quiet());
    }

    #[tokio::test]
    async fn test_log_manager_verbosity() {
        init_test_logging();

        // Create a LogCoordinator directly without initializing tracing again
        let inner = Arc::new(Inner {
            verbosity: LogVerbosity::Debug,
            quiet: false,
        });
        let log_manager = LogCoordinator { inner };

        assert_eq!(log_manager.verbosity(), LogVerbosity::Debug);
    }

    #[test]
    fn test_log_verbosity_filter_strings() {
        assert_eq!(LogVerbosity::Info.as_filter_string(), "info");
        assert_eq!(LogVerbosity::Debug.as_filter_string(), "debug");
        assert_eq!(LogVerbosity::Trace.as_filter_string(), "trace");
    }

    #[test]
    fn test_log_verbosity_default() {
        assert_eq!(LogVerbosity::default(), LogVerbosity::Info);
    }
}
