//! The parameters a binary bootconfig adds to the kernel command line.

mod binary;
mod merge;
pub(crate) mod syntax;

pub use merge::merge_parameters;

use crate::cmdline::Cmdline;
use crate::error::{self, Result};
use snafu::ensure;

/// The `kernel.*` and `init.*` parameters a binary bootconfig adds to the command line at boot:
/// one per array element, and a flag's bare name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bootconfig(pub(crate) Cmdline);

impl Bootconfig {
    /// Decode a binary bootconfig, such as `bootconfig.data` or an initrd's trailer.
    pub fn decode(data: &[u8]) -> Result<Self> {
        let text = binary::extract_text(data)?;
        parse_text(text).map(Self)
    }
}

/// Parse bootconfig text into the kernel.* and init.* parameters the kernel adds to its command
/// line: one per array element, and a flag's bare name.
fn parse_text(text: &str) -> Result<Cmdline> {
    let (mut kernel, mut init) = (Vec::new(), Vec::new());
    for entry in syntax::parse_entries(text)? {
        let (arguments, name) = if let Some(name) = entry.name.strip_prefix("kernel.") {
            (&mut kernel, name)
        } else if let Some(name) = entry.name.strip_prefix("init.") {
            (&mut init, name)
        } else {
            continue;
        };
        if entry.is_flag() {
            arguments.push(argument(name, None)?);
        }
        for value in entry.values() {
            arguments.push(argument(name, Some(value))?);
        }
    }
    Ok(Cmdline::new(kernel, init))
}

/// Render one parameter as the kernel's `xbc_snprint_cmdline()` does: `key=value`, quoted when
/// the value holds whitespace, or the bare `key` of a flag, which has no value.
fn argument(name: &str, value: Option<&str>) -> Result<String> {
    let Some(value) = value else {
        return Ok(name.to_string());
    };
    ensure!(!value.contains('"'), error::UnsupportedQuoteSnafu { name });
    Ok(if value.contains([' ', '\t', '\r', '\n']) {
        format!("{name}=\"{value}\"")
    } else {
        format!("{name}={value}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;
    use test_case::test_case;

    #[test_case(
        "kernel.foo = bar\ninit.baz = qux\nother.x = 1",
        "foo=bar -- baz=qux"
        ; "the_namespaces_go_on_their_side_of_the_separator"
    )]
    // The kernel emits one argument per element.
    #[test_case("kernel.mods = a, b, c", "mods=a mods=b mods=c" ; "an_array")]
    #[test_case("kernel {\n  sub {\n    foo = bar\n  }\n}", "sub.foo=bar" ; "a_nested_block")]
    // The image builder generates this when no package contributed a drop-in.
    #[test_case("kernel {}\ninit {}", "" ; "empty_blocks")]
    #[test_case("kernel.msg = \"a b\"", "msg=\"a b\"" ; "a_value_holding_a_space")]
    #[test_case("kernel.nosmt\nkernel.foo = \"\"", "nosmt foo=" ; "a_flag_unlike_an_empty_value")]
    #[test_case("init.splash", "-- splash" ; "a_flag")]
    #[test_case("init {\n  splash\n}" , "-- splash" ; "a_flag_in_a_block")]
    fn test_parse_text_renders_the_kernel_parameters(input: &str, expected: &str) {
        assert_eq!(parse_text(input).unwrap().to_string(), expected);
    }

    #[test]
    fn test_parse_text_syntax_error_keeps_the_diagnostic() {
        let error = parse_text("kernel.x = \"unterminated").unwrap_err();
        assert!(matches!(error, Error::BootconfigParse { .. }));
        assert!(error.to_string().contains("kernel.x"));
    }
}
