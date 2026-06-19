/*!
# Background

This library parses a `Timestamp` from a string.

The string can be:

* an `RFC3339` formatted date / time
* a string with the form `"[in] <unsigned integer> <unit(s)>"` where 'in' is optional
   * `<unsigned integer>` may be any unsigned integer and
   * `<unit(s)>` may be either the singular or plural form of the following: `hour | hours`, `day | days`, `week | weeks`

Examples:

* `"in 1 hour"`
* `"in 2 hours"`
* `"in 6 days"`
* `"in 2 weeks"`
* `"1 hour"`
* `"7 days"`
*/

use jiff::{SignedDuration, Timestamp};
use snafu::{ensure, ResultExt};

mod error {
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub(super)))]
    pub enum Error {
        #[snafu(display("Date argument '{}' is invalid: {}", input, msg))]
        DateArgInvalid { input: String, msg: &'static str },

        #[snafu(display(
            "Date argument had count '{}' that failed to parse as integer: {}",
            input,
            source
        ))]
        DateArgCount {
            input: String,
            source: std::num::ParseIntError,
        },

        #[snafu(display("Integer '{}' is not convertable to a number of {}", integer, unit))]
        DateInt { integer: u64, unit: &'static str },

        #[snafu(display("Failed to parse '{}' as RFC 3339 timestamp: {}", input, source))]
        DateRfc3339 { input: String, source: jiff::Error },

        #[snafu(display("Failed to add offset to current time: {}", source))]
        DateAddOffset { source: jiff::Error },
    }
}
pub use error::Error;
type Result<T> = std::result::Result<T, error::Error>;

/// Parses a user-specified datetime, either in full RFC 3339 format, or a shorthand like "in 7
/// days" that's taken as an offset from the time the function is run.
pub fn parse_datetime(input: &str) -> Result<Timestamp> {
    // If the user gave an absolute date in a standard format, accept it.
    if let Ok(ts) = input.parse::<Timestamp>() {
        return Ok(ts);
    }

    let offset = parse_offset(input)?;

    let now = Timestamp::now();
    let then = now.checked_add(offset).context(error::DateAddOffsetSnafu)?;
    Ok(then)
}

/// Parses a user-specified datetime offset in the form of a shorthand like "in 7 days".
pub fn parse_offset(input: &str) -> Result<SignedDuration> {
    // Otherwise, pull apart a request like "in 5 days" to get an exact datetime.
    let mut parts: Vec<&str> = input.split_whitespace().collect();
    ensure!(
        parts.len() == 3 || parts.len() == 2,
        error::DateArgInvalidSnafu {
            input,
            msg: "expected RFC 3339, or something like 'in 7 days' or '7 days'"
        }
    );
    let unit_str = parts.pop().unwrap();
    let count_str = parts.pop().unwrap();

    // the prefix string 'in' is optional
    if let Some(prefix_str) = parts.pop() {
        ensure!(
            prefix_str == "in",
            error::DateArgInvalidSnafu {
                input,
                msg: "expected prefix 'in', something like 'in 7 days'",
            }
        );
    }

    let count: u32 = count_str
        .parse()
        .context(error::DateArgCountSnafu { input })?;

    let seconds_per_unit: i64 = match unit_str {
        "minute" | "minutes" => 60,
        "hour" | "hours" => 60 * 60,
        "day" | "days" => 24 * 60 * 60,
        "week" | "weeks" => 7 * 24 * 60 * 60,
        _ => {
            return error::DateArgInvalidSnafu {
                input,
                msg: "date argument's unit must be minutes/hours/days/weeks",
            }
            .fail();
        }
    };

    let unit_name = match unit_str {
        "minute" | "minutes" => "minutes",
        "hour" | "hours" => "hours",
        "day" | "days" => "days",
        "week" | "weeks" => "weeks",
        _ => unreachable!(),
    };

    let total_seconds = i64::from(count)
        .checked_mul(seconds_per_unit)
        .ok_or_else(|| error::Error::DateInt {
            integer: u64::from(count),
            unit: unit_name,
        })?;

    Ok(SignedDuration::from_secs(total_seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_acceptable_strings() {
        let inputs = vec![
            "in 0 hours",
            "in 1 hour",
            "in 5000000 hours",
            "in 0 days",
            "in 1 day",
            "in 1000 days",
            "in 0 weeks",
            "in 1 week",
            "in 100 weeks",
            "0 weeks",
            "1 week",
            "100 weeks",
        ];

        for input in inputs {
            assert!(parse_datetime(input).is_ok(), "expected '{input}' to parse");
        }
    }

    #[test]
    fn test_unacceptable_strings() {
        let inputs = vec!["in", "0 hou", "hours", "in 1 month"];

        for input in inputs {
            assert!(parse_datetime(input).is_err())
        }
    }

    #[test]
    fn test_offset_overflows_timestamp_range() {
        // jiff::Timestamp is bounded to year ±9999. Offsets that would push the
        // resulting timestamp beyond that range must error rather than panic.
        // (chrono accepted these because DateTime<Utc> extended to ±262,000 years.)
        for input in &["in 5000000 days", "in 5000000 weeks"] {
            assert!(
                matches!(parse_datetime(input), Err(Error::DateAddOffset { .. })),
                "expected '{input}' to fail with DateAddOffset"
            );
        }
    }
}
