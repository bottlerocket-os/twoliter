//! Pest parser for grub.cfg files.

use crate::error::{self, Result};
use pest::iterators::{Pair, Pairs};
use pest::Parser;
use pest_derive::Parser;
use snafu::{OptionExt, ResultExt};
use std::collections::HashMap;

#[derive(Parser)]
#[grammar = "grub/grub.pest"]
struct GrubParser;

/// A grub.cfg's statements, with `$variables` resolved in document order.
#[derive(Debug)]
pub(crate) struct GrubConfig {
    statements: Vec<Statement>,
}

#[derive(Debug)]
enum Statement {
    Command(Command),
    /// The statements in a `menuentry` block.
    MenuEntry(Vec<Statement>),
}

/// A command and its resolved arguments.
#[derive(Debug)]
struct Command {
    name: String,
    arguments: Vec<String>,
}

impl GrubConfig {
    /// Parse grub.cfg, applying each `set` to everything that follows it.
    pub(crate) fn parse(grub_cfg: &[u8]) -> Result<Self> {
        let text =
            std::str::from_utf8(grub_cfg).context(error::InvalidUtf8Snafu { input: "grub.cfg" })?;
        let pairs = GrubParser::parse(Rule::config, text).context(error::GrubParseSnafu)?;
        let mut variables = HashMap::new();
        let statements = pairs
            .flat_map(Pair::into_inner)
            .flat_map(|pair| statement(pair, &mut variables))
            .collect();
        Ok(Self { statements })
    }

    /// Arguments of the `linux` command that boots: the last one in document order.
    pub(crate) fn linux_arguments(&self) -> Result<&[String]> {
        last_linux(&self.statements)
            .map(|command| command.arguments.as_slice())
            .context(error::MissingGrubLinuxDirectiveSnafu)
    }
}

/// The last `linux` command among `statements` and the entries nested in them.
fn last_linux(statements: &[Statement]) -> Option<&Command> {
    statements
        .iter()
        .rev()
        .find_map(|statement| match statement {
            Statement::Command(command) => (command.name == "linux").then_some(command),
            Statement::MenuEntry(body) => last_linux(body),
        })
}

/// Build the statement a pair holds, applying it to `variables` if it is a `set`.
fn statement(pair: Pair<'_, Rule>, variables: &mut HashMap<String, String>) -> Option<Statement> {
    match pair.as_rule() {
        Rule::set_stmt => {
            let mut inner = pair.into_inner();
            if let (Some(name), Some(value)) = (inner.next(), inner.next()) {
                let resolved = resolve_set_value(value, variables);
                variables.insert(name.as_str().to_string(), resolved);
            }
            None
        }
        Rule::command => {
            let mut inner = pair.into_inner();
            let name = inner.next()?.as_str().to_string();
            let arguments = inner
                .map(|argument| resolve_pair(argument, variables))
                .collect();
            Some(Statement::Command(Command { name, arguments }))
        }
        Rule::menuentry => Some(Statement::MenuEntry(
            pair.into_inner()
                .filter(|pair| pair.as_rule() == Rule::block)
                .flat_map(|block| statements(block.into_inner(), variables))
                .collect(),
        )),
        _ => None,
    }
}

fn statements(pairs: Pairs<'_, Rule>, variables: &mut HashMap<String, String>) -> Vec<Statement> {
    pairs
        .filter_map(|pair| statement(pair, variables))
        .collect()
}

/// Resolve a quoted_string, variable, keyvalue or compound_word pair, expanding variables.
fn resolve_pair(pair: Pair<'_, Rule>, variables: &HashMap<String, String>) -> String {
    match pair.as_rule() {
        Rule::variable => {
            let var_name = pair
                .into_inner()
                .find_map(|p| {
                    if p.as_rule() == Rule::var_name {
                        Some(p.as_str())
                    } else {
                        p.into_inner()
                            .find(|inner| inner.as_rule() == Rule::var_name)
                            .map(|inner| inner.as_str())
                    }
                })
                .unwrap_or("");
            variables.get(var_name).cloned().unwrap_or_default()
        }
        Rule::quoted_string => {
            let mut result = String::from("\"");
            for inner in pair.into_inner() {
                match inner.as_rule() {
                    Rule::variable => result.push_str(&resolve_pair(inner, variables)),
                    Rule::string_text => result.push_str(inner.as_str()),
                    _ => {}
                }
            }
            result.push('"');
            result
        }
        Rule::keyvalue => {
            let mut result = String::new();
            for inner in pair.into_inner() {
                match inner.as_rule() {
                    Rule::key_part => {
                        result.push_str(inner.as_str());
                        result.push('=');
                    }
                    Rule::quoted_string => result.push_str(&resolve_pair(inner, variables)),
                    Rule::value_part => result.push_str(inner.as_str()),
                    _ => {}
                }
            }
            result
        }
        Rule::compound_word => {
            let mut result = String::new();
            for inner in pair.into_inner() {
                match inner.as_rule() {
                    Rule::variable => result.push_str(&resolve_pair(inner, variables)),
                    Rule::word_text => result.push_str(inner.as_str()),
                    _ => {}
                }
            }
            result
        }
        _ => pair.as_str().to_string(),
    }
}

/// Resolve a set statement value, which may be a quoted string with variables.
fn resolve_set_value(pair: Pair<'_, Rule>, variables: &HashMap<String, String>) -> String {
    match pair.as_rule() {
        Rule::quoted_string => {
            let mut result = String::new();
            for inner in pair.into_inner() {
                match inner.as_rule() {
                    Rule::variable => result.push_str(&resolve_pair(inner, variables)),
                    Rule::string_text => result.push_str(inner.as_str()),
                    _ => {}
                }
            }
            result
        }
        Rule::unquoted_set_value => pair.as_str().to_string(),
        _ => pair.as_str().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;
    use test_case::test_case;

    fn linux_args(grub_cfg: &[u8]) -> String {
        GrubConfig::parse(grub_cfg)
            .unwrap()
            .linux_arguments()
            .unwrap()
            .join(" ")
    }

    #[test_case("set foo=bar\nlinux /vmlinuz $foo", "/vmlinuz bar" ; "bare_variable")]
    #[test_case("set foo=bar\nlinux /vmlinuz ${foo}", "/vmlinuz bar" ; "braced_variable")]
    #[test_case("linux /vmlinuz $unknown", "/vmlinuz " ; "unknown_variable")]
    #[test_case("set foo=\"bar\"\nset baz=\"I am a ${foo}\"\nlinux /vmlinuz $baz", "/vmlinuz I am a bar" ; "nested_variable")]
    #[test_case("set root=hd0,gpt3\nlinux ($root)/vmlinuz a", "(hd0,gpt3)/vmlinuz a" ; "variable_inside_a_word")]
    #[test_case("menuentry \"a\" {\n  set v=1\n}\nlinux /vmlinuz $v", "/vmlinuz 1" ; "set_inside_an_entry")]
    #[test_case("linux /vmlinuz $v\nset v=1", "/vmlinuz " ; "set_after_use")]
    fn test_variable_resolution(input: &str, expected: &str) {
        assert_eq!(linux_args(input.as_bytes()), expected);
    }

    #[test_case("linux /vmlinuz console=tty0", "/vmlinuz console=tty0" ; "simple_arg")]
    #[test_case("linux /vmlinuz key=\"value\"", "/vmlinuz key=\"value\"" ; "quoted_value_arg")]
    #[test_case("linux /vmlinuz \\\n  a \\\n  b", "/vmlinuz a b" ; "line_continuation")]
    #[test_case("linux /vmlinuz a # inline comment\n", "/vmlinuz a" ; "inline_comment")]
    #[test_case("menuentry \"test\" --unrestricted {\n  linux /vmlinuz a\n}", "/vmlinuz a" ; "menuentry_with_option")]
    fn test_linux_args(input: &str, expected: &str) {
        assert_eq!(linux_args(input.as_bytes()), expected);
    }

    #[test_case(b"menuentry \"a\" {\n  linux /a\n}\nmenuentry \"b\" {\n  linux /b\n}", "/b" ; "the_last_entry")]
    #[test_case(b"menuentry \"a\" {\n  linux /a\n}\nlinux /top", "/top" ; "a_top_level_line_after_an_entry")]
    #[test_case(b"menuentry \"a\" {\n  linux /a\n  linux /a2\n}", "/a2" ; "the_last_line_of_an_entry")]
    fn the_last_linux_in_document_order_boots(input: &[u8], expected: &str) {
        assert_eq!(linux_args(input), expected);
    }

    #[test]
    fn a_config_without_linux_is_rejected() {
        let config = GrubConfig::parse(b"set only=1\n").unwrap();
        assert!(matches!(
            config.linux_arguments(),
            Err(Error::MissingGrubLinuxDirective)
        ));
    }
}
