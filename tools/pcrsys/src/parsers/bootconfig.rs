//! Pest parser for bootconfig binary and text format.

use crate::error::Result;
use pest::iterators::Pair;
use pest::Parser;
use pest_derive::Parser;
use snafu::{whatever, ResultExt};

/// Parsed bootconfig parameters split by prefix.
#[derive(Debug, Default)]
pub struct BootconfigParams {
    /// kernel.* parameters (prefix stripped).
    pub kernel: Vec<(String, String)>,
    /// init.* parameters (prefix stripped).
    pub init: Vec<(String, String)>,
}

#[derive(Parser)]
#[grammar = "parsers/bootconfig.pest"]
struct BootconfigParser;

use Rule as BootconfigRule;

const BOOTCONFIG_MAGIC: &[u8] = b"#BOOTCONFIG\n";

/// Extract and validate bootconfig text from binary format.
///
/// Format: `[text][padding][size:u32 LE][checksum:u32 LE][magic:12 bytes]`
fn extract_text(data: &[u8]) -> Result<&str> {
    if data.len() < 20 {
        whatever!("bootconfig data too short");
    }

    let magic_start = data.len() - 12;
    if &data[magic_start..] != BOOTCONFIG_MAGIC {
        whatever!("invalid bootconfig magic");
    }

    let size_start = magic_start - 8;
    let size = u32::from_le_bytes(
        data[size_start..size_start + 4]
            .try_into()
            .whatever_context("failed to read size")?,
    ) as usize;

    let checksum = u32::from_le_bytes(
        data[size_start + 4..size_start + 8]
            .try_into()
            .whatever_context("failed to read checksum")?,
    );

    if size > size_start {
        whatever!("bootconfig size {} exceeds data length", size);
    }

    let text_data = &data[..size];
    let computed: u32 = text_data
        .iter()
        .fold(0u32, |acc, &b| acc.wrapping_add(b as u32));
    if computed != checksum {
        whatever!(
            "bootconfig checksum mismatch: expected {}, got {}",
            checksum,
            computed
        );
    }

    // Find the actual text end (before any null padding that might be included in size)
    let text_end = text_data.iter().position(|&b| b == 0).unwrap_or(size);
    std::str::from_utf8(&text_data[..text_end])
        .whatever_context("bootconfig text is not valid UTF-8")
}

/// Extract value text, handling quoted strings and arrays.
fn extract_value(pair: Pair<'_, BootconfigRule>) -> String {
    let mut values = Vec::new();
    for item in pair.into_inner() {
        let s = item.as_str();
        // Strip quotes from strings
        let v = s
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| s.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
            .unwrap_or(s);
        values.push(v);
    }
    values.join(",")
}

/// Process a pair or bool_key, adding to params with the given prefix.
fn process_entry(pair: Pair<'_, BootconfigRule>, prefix: &str, params: &mut BootconfigParams) {
    match pair.as_rule() {
        BootconfigRule::pair => {
            let mut inner = pair.into_inner();
            let key = inner.next().map(|p| p.as_str()).unwrap_or("");
            let full_key = if prefix.is_empty() {
                key.to_string()
            } else {
                format!("{prefix}.{key}")
            };
            let value = inner.next().map(extract_value).unwrap_or_default();

            if let Some(rest) = full_key.strip_prefix("kernel.") {
                params.kernel.push((rest.to_string(), value));
            } else if let Some(rest) = full_key.strip_prefix("init.") {
                params.init.push((rest.to_string(), value));
            }
        }
        BootconfigRule::bool_key => {
            let key = pair.into_inner().next().map(|p| p.as_str()).unwrap_or("");
            let full_key = if prefix.is_empty() {
                key.to_string()
            } else {
                format!("{prefix}.{key}")
            };

            if let Some(rest) = full_key.strip_prefix("kernel.") {
                params.kernel.push((rest.to_string(), String::new()));
            } else if let Some(rest) = full_key.strip_prefix("init.") {
                params.init.push((rest.to_string(), String::new()));
            }
        }
        BootconfigRule::block => {
            let mut inner = pair.into_inner();
            let block_name = inner.next().map(|p| p.as_str()).unwrap_or("");
            let new_prefix = if prefix.is_empty() {
                block_name.to_string()
            } else {
                format!("{prefix}.{block_name}")
            };
            for child in inner {
                process_entry(child, &new_prefix, params);
            }
        }
        _ => {}
    }
}

/// Parse bootconfig text and extract kernel.* and init.* parameters.
fn parse_text(text: &str) -> Result<BootconfigParams> {
    let pairs = BootconfigParser::parse(BootconfigRule::config, text)
        .whatever_context("failed to parse bootconfig")?;

    let mut params = BootconfigParams::default();

    for pair in pairs {
        if pair.as_rule() == BootconfigRule::config {
            for child in pair.into_inner() {
                process_entry(child, "", &mut params);
            }
        }
    }

    Ok(params)
}

/// Parse bootconfig.data binary format and extract parameters.
pub fn parse(data: &[u8]) -> Result<BootconfigParams> {
    let text = extract_text(data)?;
    parse_text(text)
}

/// Format bootconfig parameters as kernel command line fragment.
///
/// Uses xbc_snprint_cmdline() rules: `key=value ` with trailing space,
/// values with whitespace are quoted.
pub fn format_params(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(k, v)| {
            if v.contains(char::is_whitespace) {
                format!("{k}=\"{v}\" ")
            } else {
                format!("{k}={v} ")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    fn make_bootconfig(text: &str) -> Vec<u8> {
        let text_bytes = text.as_bytes();
        let size = text_bytes.len() as u32;
        let checksum: u32 = text_bytes.iter().map(|&b| b as u32).sum();

        let padding = (4 - (text_bytes.len() % 4)) % 4;
        let mut data = text_bytes.to_vec();
        data.extend(vec![0u8; padding]);
        data.extend(size.to_le_bytes());
        data.extend(checksum.to_le_bytes());
        data.extend(BOOTCONFIG_MAGIC);
        data
    }

    #[test]
    fn test_extract_text_valid() {
        let text = "kernel.foo = bar\ninit.baz = qux\n";
        let data = make_bootconfig(text);
        assert_eq!(extract_text(&data).unwrap(), text);
    }

    #[test]
    fn test_extract_text_with_null_padding() {
        // Simulate bootconfig where size includes null padding bytes
        let text = "kernel.foo = bar\n";
        let text_bytes = text.as_bytes();
        let padding = 3; // pad to 4-byte alignment
        let size_with_padding = (text_bytes.len() + padding) as u32;
        let checksum: u32 = text_bytes.iter().map(|&b| b as u32).sum();

        let mut data = text_bytes.to_vec();
        data.extend(vec![0u8; padding]);
        data.extend(size_with_padding.to_le_bytes());
        data.extend(checksum.to_le_bytes());
        data.extend(BOOTCONFIG_MAGIC);

        // Should extract just the text without null bytes
        assert_eq!(extract_text(&data).unwrap(), text);
    }

    #[test_case(|d| { let i = d.len() - 1; d[i] = b'X'; } ; "invalid_magic")]
    #[test_case(|d| { let i = d.len() - 16; d[i] = 0xFF; } ; "bad_checksum")]
    fn test_extract_text_errors(corrupt: fn(&mut Vec<u8>)) {
        let mut data = make_bootconfig("test");
        corrupt(&mut data);
        assert!(extract_text(&data).is_err());
    }

    #[test_case("kernel.FOO = bar", "FOO", "bar" ; "unquoted")]
    #[test_case("kernel.FOO = \"bar\"", "FOO", "bar" ; "double_quoted")]
    #[test_case("kernel.FOO = 'bar'", "FOO", "bar" ; "single_quoted")]
    #[test_case("kernel.MULTI = \"with spaces\"", "MULTI", "with spaces" ; "with_spaces")]
    #[test_case("kernel.FOO = bar;", "FOO", "bar" ; "semicolon_terminated")]
    fn test_parse_text_kernel(input: &str, key: &str, value: &str) {
        let params = parse_text(input).unwrap();
        assert_eq!(params.kernel, vec![(key.to_string(), value.to_string())]);
    }

    #[test_case("init.BAZ = qux", "BAZ", "qux" ; "init_simple")]
    fn test_parse_text_init(input: &str, key: &str, value: &str) {
        let params = parse_text(input).unwrap();
        assert_eq!(params.init, vec![(key.to_string(), value.to_string())]);
    }

    #[test]
    fn test_parse_text_array() {
        let params = parse_text("kernel.mods = a, b, c").unwrap();
        assert_eq!(
            params.kernel,
            vec![("mods".to_string(), "a,b,c".to_string())]
        );
    }

    #[test]
    fn test_parse_text_block() {
        let input = "kernel {\n  foo = bar\n  baz = qux\n}";
        let params = parse_text(input).unwrap();
        assert_eq!(params.kernel.len(), 2);
        assert_eq!(params.kernel[0], ("foo".to_string(), "bar".to_string()));
        assert_eq!(params.kernel[1], ("baz".to_string(), "qux".to_string()));
    }

    #[test]
    fn test_parse_text_nested_block() {
        let input = "kernel {\n  sub {\n    foo = bar\n  }\n}";
        let params = parse_text(input).unwrap();
        assert_eq!(
            params.kernel,
            vec![("sub.foo".to_string(), "bar".to_string())]
        );
    }

    #[test]
    fn test_parse_text_bool_key() {
        let params = parse_text("init.splash").unwrap();
        assert_eq!(params.init, vec![("splash".to_string(), String::new())]);
    }

    #[test]
    fn test_parse_text_bool_key_in_block() {
        let input = "init {\n  splash\n  quiet\n}";
        let params = parse_text(input).unwrap();
        assert_eq!(params.init.len(), 2);
        assert_eq!(params.init[0], ("splash".to_string(), String::new()));
        assert_eq!(params.init[1], ("quiet".to_string(), String::new()));
    }

    #[test]
    fn test_parse_text_ignores_comments() {
        let params = parse_text("# comment\nkernel.FOO = bar").unwrap();
        assert_eq!(params.kernel.len(), 1);
    }

    #[test]
    fn test_parse_text_kernel_docs_example() {
        // Example from kernel docs
        let input = r#"
kernel {
    root = 01234567-89ab-cdef-0123-456789abcd
}
init {
    splash
}
"#;
        let params = parse_text(input).unwrap();
        assert_eq!(
            params.kernel,
            vec![(
                "root".to_string(),
                "01234567-89ab-cdef-0123-456789abcd".to_string()
            )]
        );
        assert_eq!(params.init, vec![("splash".to_string(), String::new())]);
    }

    #[test_case(&[("FOO", "bar")], "FOO=bar " ; "simple")]
    #[test_case(&[("MULTI", "with spaces")], "MULTI=\"with spaces\" " ; "quoted")]
    #[test_case(&[("A", "1"), ("B", "2")], "A=1 B=2 " ; "multiple")]
    fn test_format_params(input: &[(&str, &str)], expected: &str) {
        let params: Vec<_> = input
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(format_params(&params), expected);
    }
}
