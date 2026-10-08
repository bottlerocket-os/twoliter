//! Bootconfig decoding and boot command-line construction shared by image
//! assembly and PCR prediction. No shell evaluation or I/O occurs here.

mod command_line;
mod parsers;

pub use command_line::{format_params, predict_grub_cmdline, uki_load_options};

mod error {
    pub type Result<T> = std::result::Result<T, snafu::Whatever>;
}

use error::Result;
use pest::{iterators::Pair, Parser};
use pest_derive::Parser;
use snafu::{ensure_whatever, ResultExt};

// The trailer stores two little-endian u32 fields followed by the magic.
const BOOTCONFIG_MAGIC: &[u8] = b"#BOOTCONFIG\n";
const BOOTCONFIG_FIELD_SIZE: usize = size_of::<u32>();
const BOOTCONFIG_TRAILER_SIZE: usize = 2 * BOOTCONFIG_FIELD_SIZE + BOOTCONFIG_MAGIC.len();

#[derive(Parser)]
#[grammar = "parsers/bootconfig.pest"]
struct BootconfigParser;

/// One boot argument. `None` is a flag; `Some("")` is an explicit empty value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    pub name: String,
    pub value: Option<String>,
}

/// Parameters in Linux bootconfig tree traversal order, with namespaces removed.
#[derive(Debug, Default)]
pub struct BootConfig {
    pub kernel: Vec<Parameter>,
    pub init: Vec<Parameter>,
}

// Linux traverses keys depth first, in order of first insertion. Keeping this
// tree matters when dotted keys with a common prefix are not adjacent in text.
#[derive(Default)]
struct Node {
    values: Option<Vec<Option<String>>>,
    children: Vec<(String, Node)>,
}

impl Node {
    fn insert(&mut self, key: &str, values: Option<Vec<Option<String>>>) -> Result<()> {
        let (name, rest) = key.split_once('.').unwrap_or((key, ""));
        let index = match self.children.iter().position(|(k, _)| k == name) {
            Some(index) => index,
            None => {
                self.children.push((name.to_owned(), Node::default()));
                self.children.len() - 1
            }
        };
        let node = &mut self.children[index].1;
        if !rest.is_empty() {
            return node.insert(rest, values);
        }
        if let Some(values) = values {
            // Linux rejects duplicate value assignments. A later assignment to
            // a previously declared flag is allowed.
            ensure_whatever!(
                node.values.as_ref().is_none_or(|v| v == &[None]),
                "duplicate bootconfig assignment for '{key}'"
            );
            if node.values.is_none() || values != [None] {
                node.values = Some(values);
            }
        }
        Ok(())
    }

    fn collect(&self, prefix: &str, out: &mut Vec<Parameter>) {
        if self.values.is_none() && self.children.is_empty() && !prefix.is_empty() {
            out.push(Parameter {
                name: prefix.to_owned(),
                value: None,
            });
        }
        if let Some(values) = self
            .values
            .as_ref()
            .filter(|values| values.as_slice() != [None] || self.children.is_empty())
        {
            for value in values {
                out.push(Parameter {
                    name: prefix.to_owned(),
                    value: value.clone(),
                });
            }
        }
        for (name, child) in &self.children {
            let key = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}.{name}")
            };
            child.collect(&key, out);
        }
    }
}

fn process(pair: Pair<'_, Rule>, prefix: &str, tree: &mut Node) -> Result<()> {
    match pair.as_rule() {
        Rule::pair | Rule::flag | Rule::block => {
            let rule = pair.as_rule();
            let mut inner = pair.into_inner();
            let name = inner.next().expect("grammar requires key").as_str();
            let key = if prefix.is_empty() {
                name.to_owned()
            } else {
                format!("{prefix}.{name}")
            };
            ensure_whatever!(key.len() < 256, "bootconfig key is too long");
            match rule {
                Rule::block => {
                    tree.insert(&key, None)?;
                    for child in inner {
                        process(child, &key, tree)?;
                    }
                }
                Rule::flag => tree.insert(&key, Some(vec![None]))?,
                Rule::pair => {
                    let values = inner
                        .map(|value| {
                            let value = value.as_str().trim();
                            let value = value
                                .strip_prefix('"')
                                .and_then(|v| v.strip_suffix('"'))
                                .or_else(|| {
                                    value.strip_prefix('\'').and_then(|v| v.strip_suffix('\''))
                                })
                                .unwrap_or(value);
                            Some(value.to_owned())
                        })
                        .collect();
                    tree.insert(&key, Some(values))?;
                }
                _ => unreachable!(),
            }
        }
        Rule::config => {
            for child in pair.into_inner() {
                process(child, prefix, tree)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Decode a standalone SDK-generated binary bootconfig.
///
/// Rejects invalid trailers, checksum, size, padding, non-ASCII text, unsupported
/// syntax and duplicate assignments. An initrd with a prepended payload is not a
/// standalone bootconfig and is rejected.
pub fn parse(data: &[u8]) -> Result<BootConfig> {
    // Require the complete trailer before reading its size, checksum, and magic.
    ensure_whatever!(
        data.len() >= BOOTCONFIG_TRAILER_SIZE,
        "bootconfig data too short"
    );
    let footer = data.len() - BOOTCONFIG_TRAILER_SIZE;
    ensure_whatever!(
        &data[footer + 2 * BOOTCONFIG_FIELD_SIZE..] == BOOTCONFIG_MAGIC,
        "invalid bootconfig magic"
    );
    let size = u32::from_le_bytes(
        data[footer..footer + BOOTCONFIG_FIELD_SIZE]
            .try_into()
            .unwrap(),
    ) as usize;
    let checksum = u32::from_le_bytes(
        data[footer + BOOTCONFIG_FIELD_SIZE..footer + 2 * BOOTCONFIG_FIELD_SIZE]
            .try_into()
            .unwrap(),
    );
    ensure_whatever!(
        size == footer && size > 0 && size <= 32767 && data.len().is_multiple_of(4),
        "invalid standalone bootconfig size or alignment"
    );
    let text_data = &data[..size];
    let sum = text_data
        .iter()
        .fold(0u32, |sum, b| sum.wrapping_add(u32::from(*b)));
    ensure_whatever!(checksum == sum, "bootconfig checksum mismatch");
    let end = text_data.iter().position(|b| *b == 0).unwrap_or(size);
    ensure_whatever!(
        end < size && size - end <= 4 && text_data[end..].iter().all(|b| *b == 0),
        "invalid bootconfig NUL terminator or padding"
    );
    parse_text(std::str::from_utf8(&text_data[..end]).whatever_context("invalid bootconfig UTF-8")?)
}

/// Parse the supported Linux bootconfig text syntax.
///
/// Returns an error on invalid text or unsupported assignments.
pub fn parse_text(text: &str) -> Result<BootConfig> {
    ensure_whatever!(
        text.len() < 32767
            && text
                .bytes()
                .all(|b| b.is_ascii_graphic() || b.is_ascii_whitespace()),
        "bootconfig must contain printable ASCII text"
    );
    // Bound recursive grammar processing before Pest builds the parse tree.
    // Quotes and comments may contain literal braces.
    let (mut quote, mut comment, mut depth) = (None, false, 0usize);
    for byte in text.bytes() {
        if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            }
        } else if comment {
            comment = byte != b'\n';
        } else {
            match byte {
                b'"' | b'\'' => quote = Some(byte),
                b'#' => comment = true,
                b'{' => {
                    depth += 1;
                    ensure_whatever!(depth <= 64, "bootconfig block nesting exceeds 64");
                }
                b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    let pairs = BootconfigParser::parse(Rule::config, text)
        .whatever_context("failed to parse bootconfig")?;
    let mut tree = Node::default();
    for pair in pairs {
        process(pair, "", &mut tree)?;
    }
    let mut config = BootConfig::default();
    for (name, child) in tree.children {
        match name.as_str() {
            "kernel" => child.collect("", &mut config.kernel),
            "init" => child.collect("", &mut config.init),
            _ => {}
        }
    }
    ensure_whatever!(
        config
            .kernel
            .iter()
            .chain(&config.init)
            .all(|p| !p.name.is_empty()),
        "bootconfig kernel and init must be namespaces"
    );
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_trailer_is_required_and_checked() {
        // Padded "kernel.x=1", size 12, byte-sum checksum 917, and magic.
        let data = b"kernel.x=1\0\0\x0c\0\0\0\x95\x03\0\0#BOOTCONFIG\n";
        assert_eq!(parse(data).unwrap().kernel[0].value.as_deref(), Some("1"));
        assert!(parse(&data[..BOOTCONFIG_TRAILER_SIZE - 1]).is_err());
        let mut corrupt = *data;
        corrupt[0] = b'K';
        assert!(parse(&corrupt).is_err());
    }

    #[test]
    fn text_preserves_tree_order_and_rejects_duplicate_values() {
        let config = parse_text("kernel.z.x=first\nkernel.a=middle\nkernel.z.y=last").unwrap();
        assert_eq!(
            format_params(&config.kernel).unwrap(),
            "z.x=first z.y=last a=middle "
        );
        assert!(parse_text("kernel.x=a\nkernel.x=b").is_err());
    }
}
