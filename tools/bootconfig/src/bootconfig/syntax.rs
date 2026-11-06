//! Pest parser for bootconfig text.

use crate::error::{self, Result};
use pest::iterators::Pair;
use pest::Parser;
use pest_derive::Parser;
use snafu::ResultExt;

#[derive(Parser)]
#[grammar = "bootconfig/bootconfig.pest"]
struct BootconfigParser;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BootconfigEntry {
    pub(crate) name: String,
    pub(crate) elements: Vec<String>,
}

impl BootconfigEntry {
    pub(crate) fn is_flag(&self) -> bool {
        self.elements.is_empty()
    }

    pub(crate) fn get_setting(&self) -> String {
        self.elements
            .iter()
            .map(|element| element.trim_matches(['\x0b', '\x0c']))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Each array element, unquoted.
    pub(crate) fn values(&self) -> impl Iterator<Item = &str> {
        self.elements.iter().map(|element| unquote(element))
    }
}

/// Strip the quotes from one element of a value.
fn unquote(s: &str) -> &str {
    s.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| s.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(s)
}

/// Parse the encoded bootconfig output using the grammar defined in bootconfig.pest
pub(crate) fn parse_entries(text: &str) -> Result<Vec<BootconfigEntry>> {
    let pairs = BootconfigParser::parse(Rule::config, text).context(error::BootconfigParseSnafu)?;
    let mut entries = Vec::new();

    for pair in pairs {
        if pair.as_rule() == Rule::config {
            for child in pair.into_inner() {
                process_entry(child, "", &mut entries)?;
            }
        }
    }

    Ok(entries)
}

/// Process a pair or bool_key, adding to params with the given prefix.
fn process_entry(
    pair: Pair<'_, Rule>,
    prefix: &str,
    entries: &mut Vec<BootconfigEntry>,
) -> Result<()> {
    let mut inner = pair.clone().into_inner();

    let key = inner.next().map(|p| p.as_str()).unwrap_or("");
    let full_key = if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}.{key}")
    };

    match pair.as_rule() {
        Rule::pair => {
            let elements = inner
                .flat_map(Pair::into_inner)
                .map(|element| element.as_str().to_owned())
                .collect();
            entries.push(BootconfigEntry {
                name: full_key,
                elements,
            });
        }
        Rule::bool_key => {
            entries.push(BootconfigEntry {
                name: full_key,
                elements: Vec::new(),
            });
        }
        Rule::block => {
            for child in inner {
                process_entry(child, &full_key, entries)?;
            }
        }
        // Only `EOI`, which ends `config`; newlines and comments produce no pairs.
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test]
    fn test_parse_entries_keeps_every_namespace_and_entry() {
        let input = "kernel.x = 1\nother.y = 2\nkernel.x = 3";
        let entries = parse_entries(input).unwrap();
        let values: Vec<(&str, &str)> = entries
            .iter()
            .flat_map(|entry| {
                let name = entry.name.as_str();
                entry.values().map(move |value| (name, value))
            })
            .collect();
        assert_eq!(
            values,
            [("kernel.x", "1"), ("other.y", "2"), ("kernel.x", "3")]
        );
    }

    #[test_case("kernel.x = 1 # comment", "kernel.x", "1", &["1"] ; "a_trailing_comment")]
    #[test_case("kernel.x = a , 'b c';", "kernel.x", "a, 'b c'", &["a", "b c"] ; "an_array")]
    #[test_case("kernel.x = \"a,b\"", "kernel.x", "\"a,b\"", &["a,b"] ; "a_quoted_comma")]
    #[test_case("kernel { x = 1 }", "kernel.x", "1", &["1"] ; "a_block")]
    fn test_parse_entries_keeps_values_as_written(
        input: &str,
        name: &str,
        written: &str,
        values: &[&str],
    ) {
        let entries = parse_entries(input).unwrap();
        let [entry] = entries.as_slice() else {
            panic!("expected one entry, got {entries:?}");
        };
        assert_eq!(entry.name, name);
        assert_eq!(entry.get_setting(), written);
        assert_eq!(entry.values().collect::<Vec<_>>(), values);
    }
}
