//! Custom tracing formatter for CAS operations.
//!
//! The CasFormatter implements formatting for casi logging events,
//! with hierarchical span display, colored output based on terminal capabilities,
//! and field-specific formatting for CAS operations and metadata. It works
//! together with CasVisitor to create a consistent logging experience.

use super::visitor::CasVisitor;
use owo_colors::{OwoColorize, Stream};
use std::fmt;
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::{FormatFields, Writer};
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormattedFields};
use tracing_subscriber::registry::LookupSpan;

/// Custom tracing formatter for CAS operations.
///
/// The CasFormatter implements:
/// - Hierarchical span display with indentation
/// - Color differentiation for log levels with terminal capability detection
/// - Field-specific formatting for CAS operation metadata
/// - Standardized timestamp and field formatting
#[derive(Clone, Default)]
pub struct CasFormatter {
    /// Whether quiet mode is enabled
    quiet: bool,
}

impl CasFormatter {
    /// Create a new CasFormatter with the specified quiet mode setting
    pub fn new(quiet: bool) -> Self {
        Self { quiet }
    }

    /// Format the log level with color differentiation.
    fn format_level(&self, level: &tracing::Level, writer: &mut Writer<'_>) -> fmt::Result {
        match *level {
            tracing::Level::ERROR => {
                let colored = "ERROR".if_supports_color(Stream::Stderr, |text| text.red());
                write!(writer, "{colored}")
            }
            tracing::Level::WARN => {
                let colored = "WARN ".if_supports_color(Stream::Stderr, |text| text.yellow());
                write!(writer, "{colored}")
            }
            tracing::Level::INFO => {
                let colored = "INFO ".if_supports_color(Stream::Stderr, |text| text.green());
                write!(writer, "{colored}")
            }
            tracing::Level::DEBUG => {
                let colored = "DEBUG".if_supports_color(Stream::Stderr, |text| text.blue());
                write!(writer, "{colored}")
            }
            tracing::Level::TRACE => {
                let colored = "TRACE".if_supports_color(Stream::Stderr, |text| text.cyan());
                write!(writer, "{colored}")
            }
        }
    }

    /// Format the timestamp in a simplified, more readable format.
    fn format_timestamp(&self, writer: &mut Writer<'_>) -> fmt::Result {
        let now = chrono::Utc::now();
        let timestamp = now.format("%H:%M:%S%.3f UTC");
        write!(
            writer,
            "{}",
            timestamp
                .to_string()
                .if_supports_color(Stream::Stderr, |text| text.dimmed())
        )
    }

    /// Format span information with hierarchy and context.
    fn format_span_context<S, N>(
        &self,
        ctx: &FmtContext<'_, S, N>,
        writer: &mut Writer<'_>,
    ) -> fmt::Result
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
        N: for<'a> FormatFields<'a> + 'static,
    {
        if let Some(scope) = ctx.lookup_current() {
            let span_name = scope.name();
            write!(
                writer,
                "{} ",
                span_name.if_supports_color(Stream::Stderr, |text| text.bold().cyan().to_string())
            )?;
        }

        Ok(())
    }

    /// Format the target (module path) if enabled.
    fn format_target(
        &self,
        target: &str,
        writer: &mut Writer<'_>,
        show_target: bool,
    ) -> fmt::Result {
        if show_target && !target.is_empty() {
            let colored_target = target.if_supports_color(Stream::Stderr, |text| text.dimmed());
            write!(writer, "{colored_target}: ")?;
        }
        Ok(())
    }
}

impl<S, N> FormatEvent<S, N> for CasFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        // In quiet mode, only process ERROR level events
        // and skip all other events to make quiet mode truly minimal
        if self.quiet && !matches!(*event.metadata().level(), tracing::Level::ERROR) {
            return Ok(());
        }

        // Format timestamp
        self.format_timestamp(&mut writer)?;
        write!(writer, " ")?;

        // Format log level
        self.format_level(event.metadata().level(), &mut writer)?;
        write!(writer, " ")?;

        // Format span context
        self.format_span_context(ctx, &mut writer)?;

        // Format target (module path) - only show for DEBUG and TRACE levels
        let show_target = matches!(
            *event.metadata().level(),
            tracing::Level::DEBUG | tracing::Level::TRACE
        );
        self.format_target(event.metadata().target(), &mut writer, show_target)?;

        let mut visitor = CasVisitor::new(writer);
        event.record(&mut visitor);

        // We want to print out the parent spans fields with the vent
        let span = event
            .parent()
            .and_then(|id| ctx.span(id))
            .or_else(|| ctx.lookup_current());
        if let Some(span) = span {
            let ext = span.extensions();
            let fields = ext.get::<FormattedFields<N>>().unwrap();
            write!(visitor.writer(), " {fields}")?;
        }

        // Add newline
        writeln!(visitor.writer())?;

        Ok(())
    }
}

impl<'writer> FormatFields<'writer> for CasFormatter {
    fn format_fields<R: tracing_subscriber::field::RecordFields>(
        &self,
        writer: Writer<'writer>,
        fields: R,
    ) -> fmt::Result {
        let mut visitor = CasVisitor::new(writer);
        fields.record(&mut visitor);
        Ok(())
    }
}
