//! Enhanced trace context system for operation correlation and structured logging.
//!
//! This module provides the core types for the enhanced tracing system,
//! including trace context, performance metrics, and error context.
//!
//! The TraceContext approach is the preferred and recommended method for
//! logging and tracing in cassi. It provides a comprehensive, structured way to
//! track operations, maintain context across async boundaries, and collect
//! performance metrics.
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;
use tracing::Span;
use uuid::Uuid;

static SESSION_ID: OnceCell<Uuid> = OnceCell::const_new();

/// Enhanced trace context for operation correlation and structured logging.
///
/// TraceContext provides a consistent way to track related operations
/// and maintain context across async boundaries. This is the recommended
/// approach for all logging and tracing in cassi.
///
/// Use the trace_context! macro to create instances with minimal boilerplate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceContext {
    /// Unique identifier for this operation session
    pub session_id: String,
    /// Correlation ID for related operations
    pub correlation_id: String,
    /// Operation type (create, store, fetch, etc.)
    pub operation: String,
    /// Target artifact ID (if applicable)
    pub artifact_id: Option<String>,
    /// User-provided labels for filtering and categorization
    pub labels: HashMap<String, String>,
    /// Timestamp when context was created
    pub created_at: SystemTime,
}

impl TraceContext {
    pub fn record(&self, span: &Span) {
        span.record("session_id", self.session_id.clone());
        span.record("correlation_id", self.correlation_id.clone());
        if let Some(artifact_id) = self.artifact_id.as_ref() {
            span.record("artifact_id", artifact_id.clone());
        }
        for (key, value) in self.labels.iter() {
            span.record(key.as_str(), value.clone());
        }
    }

    /// Create a new trace context for an operation.
    pub async fn new(operation: impl Into<String>) -> Self {
        let operation_str = operation.into();
        let session_id = SESSION_ID
            .get_or_init(async || Uuid::new_v4())
            .await
            .to_string();
        let ctx_id = Uuid::new_v4().to_string();
        let correlation_id = format!("{}-{}", operation_str, &ctx_id[..8]);

        Self {
            session_id,
            correlation_id,
            operation: operation_str,
            artifact_id: None,
            labels: HashMap::new(),
            created_at: SystemTime::now(),
        }
    }

    /// Create a child context for a related operation.
    pub fn child(&self, operation: impl Into<String>) -> Self {
        let operation_str = operation.into();
        let child_id = Uuid::new_v4().to_string();
        let correlation_id = format!("{}-{}", operation_str, &child_id[..8]);

        Self {
            session_id: self.session_id.clone(),
            correlation_id,
            operation: operation_str,
            artifact_id: self.artifact_id.clone(),
            labels: self.labels.clone(),
            created_at: SystemTime::now(),
        }
    }

    /// Set the artifact ID for this context.
    pub fn with_artifact_id(mut self, artifact_id: impl Into<String>) -> Self {
        self.artifact_id = Some(artifact_id.into());
        self
    }

    /// Add a label to this context.
    pub fn with_label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.insert(key.into(), value.into());
        self
    }

    /// Add multiple labels to this context.
    pub fn with_labels(mut self, labels: HashMap<String, String>) -> Self {
        self.labels.extend(labels);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Throughput(f64);

impl From<f64> for Throughput {
    fn from(value: f64) -> Self {
        Self(value)
    }
}

impl std::fmt::Display for Throughput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let round: u64 = self.0.ceil() as u64;
        if round > 1024 * 1024 * 1024 {
            f.write_fmt(format_args!("{}gb/s", round / (1024 * 1024 * 1024)))
        } else if round > 1024 * 1024 {
            f.write_fmt(format_args!("{}mb/s", round / (1024 * 1024)))
        } else if round > 1024 {
            f.write_fmt(format_args!("{}kb/s", round / 1024))
        } else {
            f.write_fmt(format_args!("{round}b/s"))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ByteCount(u64);

pub fn byte_count(value: impl Into<u64>) -> String {
    ByteCount(value.into()).to_string()
}

impl From<u64> for ByteCount {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl std::fmt::Display for ByteCount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0 > 1024 * 1024 * 1024 {
            f.write_fmt(format_args!("{}gb", self.0 / (1024 * 1024 * 1024)))
        } else if self.0 > 1024 * 1024 {
            f.write_fmt(format_args!("{}mb", self.0 / (1024 * 1024)))
        } else if self.0 > 1024 {
            f.write_fmt(format_args!("{}kb", self.0 / 1024))
        } else {
            f.write_fmt(format_args!("{}b", self.0))
        }
    }
}

/// Performance metrics collected during operations.
///
/// PerformanceMetrics provides standardized performance tracking
/// across all CAS operations. It integrates with the TraceContext approach
/// to provide comprehensive operational insights.
///
/// Use the trace_metrics! macro to create instances with minimal boilerplate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceMetrics {
    /// Operation start time (serialized as duration since epoch)
    #[serde(skip, default = "Instant::now")]
    pub start_time: Instant,
    /// Operation duration (set when operation completes)
    pub duration: Option<Duration>,
    /// Total bytes processed
    pub bytes_processed: ByteCount,
    /// Number of files processed
    pub files_processed: u32,
    /// Throughput in bytes per second
    pub throughput: Option<Throughput>,
    /// Cache hit ratio (0.0 to 1.0)
    pub cache_hit_ratio: Option<f64>,
    /// Number of network requests made
    pub network_requests: u32,
    /// Total network bytes transferred
    pub network_bytes: ByteCount,
}

impl PerformanceMetrics {
    pub fn record(&self, span: &Span) {
        if let Some(duration) = self.duration.as_ref() {
            span.record("duration_ms", duration.as_millis());
        }
        if self.bytes_processed.0 > 0 {
            span.record("bytes_processed", self.bytes_processed.to_string());
        }
        if self.files_processed > 0 {
            span.record("files_processed", self.files_processed);
        }
        if let Some(throughput) = self.throughput.as_ref() {
            span.record("throughput_bps", throughput.to_string());
        }
        if let Some(cache_hit) = self.cache_hit_ratio {
            span.record("cache_hit_ratio", cache_hit);
        }
        if self.network_requests > 0 {
            span.record("network_requests", self.network_requests);
        }
        if self.network_bytes.0 > 0 {
            span.record("network_bytes", self.network_bytes.to_string());
        }
    }
    /// Create new performance metrics with current timestamp.
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            duration: None,
            bytes_processed: 0.into(),
            files_processed: 0,
            throughput: None,
            cache_hit_ratio: None,
            network_requests: 0,
            network_bytes: 0.into(),
        }
    }

    /// Mark the operation as complete and calculate final metrics.
    pub fn complete(&mut self) {
        let duration = self.start_time.elapsed();
        self.duration = Some(duration);

        // Calculate throughput if we have duration and bytes
        if duration.as_secs_f64() > 0.0 && self.bytes_processed.0 > 0 {
            self.throughput = Some(Throughput(
                self.bytes_processed.0 as f64 / duration.as_secs_f64(),
            ));
        }
    }

    /// Add bytes to the processed count.
    pub fn add_bytes(&mut self, bytes: u64) {
        self.bytes_processed.0 += bytes;
    }

    /// Increment the file count.
    pub fn add_file(&mut self) {
        self.files_processed += 1;
    }

    /// Add a network request to the metrics.
    pub fn add_network_request(&mut self, bytes: u64) {
        self.network_requests += 1;
        self.network_bytes.0 += bytes;
    }

    /// Set the cache hit ratio.
    pub fn set_cache_hit_ratio(&mut self, ratio: f64) {
        self.cache_hit_ratio = Some(ratio.clamp(0.0, 1.0));
    }

    /// Get the current duration since start.
    pub fn current_duration(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Get the current throughput based on elapsed time.
    pub fn current_throughput(&self) -> Option<f64> {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        if elapsed > 0.0 && self.bytes_processed.0 > 0 {
            Some(self.bytes_processed.0 as f64 / elapsed)
        } else {
            None
        }
    }
}

impl Default for PerformanceMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Rich error context for debugging and analysis.
///
/// ErrorContext provides structured error information that helps
/// with debugging and error pattern analysis. When combined with TraceContext
/// in the trace_error! macro, it creates comprehensive error logs with full context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorContext {
    /// Error category for classification
    pub category: ErrorCategory,
    /// Error code for programmatic handling
    pub error_code: String,
    /// Human-readable error message
    pub message: String,
    /// Stack trace (if available)
    pub stack_trace: Option<String>,
    /// Recovery attempts made
    pub recovery_attempts: Vec<RecoveryAttempt>,
    /// Related operations that might have contributed
    pub related_operations: Vec<String>,
    /// Additional context fields
    pub context: HashMap<String, String>,
}

impl ErrorContext {
    /// Create a new error context.
    pub fn new(
        category: ErrorCategory,
        error_code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            category,
            error_code: error_code.into(),
            message: message.into(),
            stack_trace: None,
            recovery_attempts: Vec::new(),
            related_operations: Vec::new(),
            context: HashMap::new(),
        }
    }

    /// Add a recovery attempt to the error context.
    pub fn add_recovery_attempt(&mut self, attempt: RecoveryAttempt) {
        self.recovery_attempts.push(attempt);
    }

    /// Add a related operation ID.
    pub fn add_related_operation(&mut self, operation_id: impl Into<String>) {
        self.related_operations.push(operation_id.into());
    }

    /// Add context information.
    pub fn add_context(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.context.insert(key.into(), value.into());
    }

    /// Set the stack trace.
    pub fn with_stack_trace(mut self, stack_trace: impl Into<String>) -> Self {
        self.stack_trace = Some(stack_trace.into());
        self
    }
}

/// Error categories for classification and filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorCategory {
    /// Network-related errors (timeouts, connection failures)
    Network,
    /// File system errors (permissions, disk space, I/O)
    FileSystem,
    /// Authentication and authorization errors
    Authentication,
    /// Input validation and format errors
    Validation,
    /// Internal application errors and bugs
    Internal,
    /// Configuration and setup errors
    Configuration,
    /// Resource exhaustion (memory, disk, limits)
    Resource,
}

impl ErrorCategory {
    /// Get a human-readable description of the error category.
    pub fn description(&self) -> &'static str {
        match self {
            ErrorCategory::Network => "Network communication error",
            ErrorCategory::FileSystem => "File system operation error",
            ErrorCategory::Authentication => "Authentication or authorization error",
            ErrorCategory::Validation => "Input validation error",
            ErrorCategory::Internal => "Internal application error",
            ErrorCategory::Configuration => "Configuration error",
            ErrorCategory::Resource => "Resource exhaustion error",
        }
    }
}

/// Information about a recovery attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryAttempt {
    /// Type of recovery attempted
    pub recovery_type: String,
    /// Timestamp of the attempt
    pub attempted_at: SystemTime,
    /// Whether the recovery was successful
    pub successful: bool,
    /// Additional details about the attempt
    pub details: Option<String>,
}

impl RecoveryAttempt {
    /// Create a new recovery attempt record.
    pub fn new(recovery_type: impl Into<String>, successful: bool) -> Self {
        Self {
            recovery_type: recovery_type.into(),
            attempted_at: SystemTime::now(),
            successful,
            details: None,
        }
    }

    /// Add details to the recovery attempt.
    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_trace_context_creation() {
        let ctx = TraceContext::new("store").await;

        assert_eq!(ctx.operation, "store");
        assert!(ctx.artifact_id.is_none());
        assert!(ctx.labels.is_empty());
        assert!(!ctx.session_id.is_empty());
        assert!(!ctx.correlation_id.is_empty());
    }

    #[tokio::test]
    async fn test_trace_context_child() {
        let parent = TraceContext::new("store")
            .await
            .with_artifact_id("test-artifact")
            .with_label("env", "test");

        let child = parent.child("compress");

        assert_eq!(child.session_id, parent.session_id);
        assert_ne!(child.correlation_id, parent.correlation_id);
        assert_eq!(child.operation, "compress");
        assert_eq!(child.artifact_id, parent.artifact_id);
        assert_eq!(child.labels, parent.labels);
    }

    #[test]
    fn test_performance_metrics() {
        let mut metrics = PerformanceMetrics::new();

        metrics.add_bytes(1024);
        metrics.add_file();
        metrics.add_network_request(512);

        assert_eq!(metrics.bytes_processed.0, 1024);
        assert_eq!(metrics.files_processed, 1);
        assert_eq!(metrics.network_requests, 1);
        assert_eq!(metrics.network_bytes.0, 512);

        // Test completion
        std::thread::sleep(std::time::Duration::from_millis(10));
        metrics.complete();

        assert!(metrics.duration.is_some());
        assert!(metrics.throughput.is_some());
    }

    #[test]
    fn test_error_context() {
        let mut error_ctx =
            ErrorContext::new(ErrorCategory::Network, "TIMEOUT", "Connection timed out");

        error_ctx.add_recovery_attempt(RecoveryAttempt::new("retry", false));
        error_ctx.add_related_operation("store-abc123");
        error_ctx.add_context("endpoint", "s3.amazonaws.com");

        assert_eq!(error_ctx.category, ErrorCategory::Network);
        assert_eq!(error_ctx.error_code, "TIMEOUT");
        assert_eq!(error_ctx.recovery_attempts.len(), 1);
        assert_eq!(error_ctx.related_operations.len(), 1);
        assert_eq!(error_ctx.context.len(), 1);
    }

    #[test]
    fn test_error_category_description() {
        assert_eq!(
            ErrorCategory::Network.description(),
            "Network communication error"
        );
        assert_eq!(
            ErrorCategory::FileSystem.description(),
            "File system operation error"
        );
    }
}
