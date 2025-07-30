//! Custom field visitor for CAS operation logging.
//!
//! The CasVisitor implements field formatting for tracing events,
//! with specific handling for artifact IDs, operation context, and metadata
//! used in CAS operations. It formats different field types with distinct
//! visual representations to improve log readability.

use std::{collections::HashSet, fmt};

use owo_colors::{OwoColorize, Stream};
use tracing::field::{Field, Visit};
use tracing_subscriber::fmt::format::Writer;

/// Custom field visitor for structured field display in CAS operations.
///
/// The CasVisitor implements:
/// - Specific handling for artifact IDs and operation context
/// - Standardized formatting for timestamps and operation metadata
/// - Color differentiation for field names and values based on field type
/// - Hierarchical display of nested operation information
pub struct CasVisitor<'writer> {
    writer: Writer<'writer>,
    is_first_field: bool,
    result: fmt::Result,
    written: HashSet<String>,
}

impl<'writer> CasVisitor<'writer> {
    /// Create a new CasVisitor with the given writer.
    pub fn new(writer: Writer<'writer>) -> Self {
        Self {
            writer,
            is_first_field: true,
            result: Ok(()),
            written: HashSet::new(),
        }
    }

    /// Get a mutable reference to the writer.
    pub fn writer(&mut self) -> &mut Writer<'writer> {
        &mut self.writer
    }

    /// Write a field separator if this is not the first field.
    fn write_separator(&mut self) {
        if self.result.is_ok() {
            if self.is_first_field {
                self.is_first_field = false;
            } else {
                self.result = write!(self.writer, " ");
            }
        }
    }

    /// Format a field name with appropriate styling.
    fn format_field_name(&mut self, name: &str) {
        if self.result.is_ok() {
            self.result = write!(
                self.writer,
                "\n    {}=",
                name.if_supports_color(Stream::Stderr, |text| text.bold().blue().to_string())
            );
        }
    }

    /// Format a field value with special handling for known CAS field types.
    /// Returns whether the field was actually displayed (false if skipped).
    fn format_field_value(&mut self, field: &Field, value: &dyn fmt::Display) {
        if self.result.is_ok() {
            if self.written.contains(field.name()) {
                return;
            }
            self.written.insert(field.name().to_string());
            match field.name() {
                // Special formatting for artifact IDs
                "artifact_id" | "id" => {
                    // Clean up Option wrapper: convert 'Some("test_app:test")' to just 'test_app:test'
                    let value_str = format!("{value}");

                    // Skip printing artifact_id if it's None
                    if value_str == "None" {
                        // We do nothing, effectively skipping this field completely
                        return;
                    }

                    // We need to write a separator since we're going to output this field
                    self.write_separator();
                    self.format_field_name(field.name());

                    let cleaned_value =
                        if value_str.starts_with("Some(") && value_str.ends_with(")") {
                            // Extract content between quotes within Some()
                            if let Some(start) = value_str.find('"') {
                                if let Some(end) = value_str[start + 1..].find('"') {
                                    &value_str[start + 1..start + 1 + end]
                                } else {
                                    &value_str
                                }
                            } else {
                                // If no quotes found, try extracting what's between Some( and )
                                &value_str[5..value_str.len() - 1]
                            }
                        } else {
                            &value_str
                        };

                    self.result = write!(
                        self.writer,
                        "{}",
                        cleaned_value.if_supports_color(Stream::Stderr, |text| text
                            .green()
                            .bold()
                            .to_string())
                    );

                    // Return early as we've handled everything
                }
                // Special formatting for file paths
                "path" | "file_path" | "source_path" | "dest_path" => {
                    self.write_separator();
                    self.format_field_name(field.name());
                    self.result = write!(
                        self.writer,
                        "\"{}\"",
                        value.if_supports_color(Stream::Stderr, |text| text.cyan().to_string())
                    );
                }
                // Special formatting for hashes
                "hash" | "sha256" | "digest" => {
                    self.write_separator();
                    self.format_field_name(field.name());
                    let hash_str = format!("{value}");
                    let truncated = if hash_str.len() > 12 {
                        format!("{}...", &hash_str[..12])
                    } else {
                        hash_str
                    };
                    let colored_value =
                        truncated.if_supports_color(Stream::Stderr, |text| text.yellow());
                    self.result = write!(self.writer, "{colored_value}");
                }
                // Special formatting for sizes and counts
                "size" | "bytes" | "length" | "count" => {
                    self.write_separator();
                    self.format_field_name(field.name());
                    self.result = write!(
                        self.writer,
                        "{}",
                        value.if_supports_color(Stream::Stderr, |text| text.magenta().to_string())
                    );
                }
                // Special formatting for operation types
                "operation" | "op_type" | "backend" => {
                    self.write_separator();
                    self.format_field_name(field.name());
                    self.result = write!(
                        self.writer,
                        "{}",
                        value.if_supports_color(Stream::Stderr, |text| text
                            .bright_blue()
                            .to_string())
                    );
                }
                // Special formatting for durations and timing
                "elapsed" | "duration" | "timeout" => {
                    self.write_separator();
                    self.format_field_name(field.name());
                    self.result = write!(
                        self.writer,
                        "{}",
                        value.if_supports_color(Stream::Stderr, |text| text
                            .bright_cyan()
                            .to_string())
                    );
                }
                // Special formatting for error-related fields
                "error" | "err" | "failure" => {
                    self.write_separator();
                    self.format_field_name(field.name());
                    self.result = write!(
                        self.writer,
                        "\"{}\"",
                        value.if_supports_color(Stream::Stderr, |text| text.red().to_string())
                    );
                }
                // Special formatting for status fields
                "status" | "state" => {
                    self.write_separator();
                    self.format_field_name(field.name());
                    let status_str = format!("{value}");
                    match status_str.to_lowercase().as_str() {
                        "success" | "ok" | "complete" | "finished" => {
                            let colored =
                                status_str.if_supports_color(Stream::Stderr, |text| text.green());
                            self.result = write!(self.writer, "{colored}");
                        }
                        "error" | "failed" | "failure" => {
                            let colored =
                                status_str.if_supports_color(Stream::Stderr, |text| text.red());
                            self.result = write!(self.writer, "{colored}");
                        }
                        "warning" | "warn" => {
                            let colored =
                                status_str.if_supports_color(Stream::Stderr, |text| text.yellow());
                            self.result = write!(self.writer, "{colored}");
                        }
                        "pending" | "in_progress" | "running" => {
                            let colored =
                                status_str.if_supports_color(Stream::Stderr, |text| text.blue());
                            self.result = write!(self.writer, "{colored}");
                        }
                        _ => {
                            let colored =
                                status_str.if_supports_color(Stream::Stderr, |text| text.white());
                            self.result = write!(self.writer, "{colored}");
                        }
                    }
                }
                // Default formatting for other fields
                _ => {
                    self.write_separator();
                    self.format_field_name(field.name());
                    let value_str = format!("{value}");
                    // Check if the value looks like a string that should be quoted
                    if value_str.contains(' ')
                        || value_str.contains('\t')
                        || value_str.contains('\n')
                    {
                        self.result = write!(self.writer, "\"{value_str}\"");
                    } else {
                        self.result = write!(self.writer, "{value_str}");
                    }
                }
            }
        }
    }

    /// Handle the special "message" field which contains the main log message.
    fn handle_message_field(&mut self, value: &dyn fmt::Display) {
        if self.result.is_ok() {
            // The message field is the main content, so we don't format it as key=value
            // Instead, we just write it directly
            self.result = write!(self.writer, "{value}");
        }
    }
}

impl<'writer> Visit for CasVisitor<'writer> {
    fn record_f64(&mut self, field: &Field, value: f64) {
        if field.name() == "message" {
            self.write_separator();
            self.handle_message_field(&value);
        } else {
            // Note: Don't call write_separator() here as format_field_value
            // handles artifact_id specially and may skip writing anything
            self.format_field_value(field, &value);
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if field.name() == "message" {
            self.write_separator();
            self.handle_message_field(&value);
        } else {
            // Note: Don't call write_separator() here as format_field_value
            // handles artifact_id specially and may skip writing anything
            self.format_field_value(field, &value);
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "message" {
            self.write_separator();
            self.handle_message_field(&value);
        } else {
            // Note: Don't call write_separator() here as format_field_value
            // handles artifact_id specially and may skip writing anything
            self.format_field_value(field, &value);
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name() == "message" {
            self.write_separator();
            self.handle_message_field(&value);
        } else {
            // For boolean values, handle specially since we don't call format_field_value
            if field.name() == "artifact_id" || field.name() == "id" {
                // Skip if it's an artifact_id field (though this shouldn't happen for booleans)
                return;
            }

            self.write_separator();
            self.format_field_name(field.name());
            if self.result.is_ok() {
                if value {
                    let colored_value =
                        "true".if_supports_color(Stream::Stderr, |text| text.green());
                    self.result = write!(self.writer, "{colored_value}");
                } else {
                    let colored_value =
                        "false".if_supports_color(Stream::Stderr, |text| text.red());
                    self.result = write!(self.writer, "{colored_value}");
                }
            }
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.write_separator();
            self.handle_message_field(&value);
        } else {
            // Note: Don't call write_separator() here as format_field_value
            // handles artifact_id specially and may skip writing anything
            self.format_field_value(field, &value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.write_separator();
            self.handle_message_field(&format_args!("{value:?}"));
        } else {
            // Note: Don't call write_separator() here as format_field_value
            // handles artifact_id specially and may skip writing anything
            self.format_field_value(field, &format_args!("{value:?}"));
        }
    }
}
