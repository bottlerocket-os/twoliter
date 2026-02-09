//! Pest parser for grub.cfg files.

use crate::error::Result;
use pest::iterators::Pair;
use pest::Parser;
use pest_derive::Parser;
use snafu::ResultExt;
use std::collections::HashMap;

#[derive(Parser)]
#[grammar = "parsers/grub.pest"]
struct GrubParser;

use Rule as GrubRule;

/// Resolve a quoted_string or variable pair, expanding variables from the map.
fn resolve_pair(pair: Pair<'_, GrubRule>, variables: &HashMap<String, String>) -> String {
    match pair.as_rule() {
        GrubRule::variable => {
            let var_name = pair
                .into_inner()
                .find_map(|p| {
                    if p.as_rule() == GrubRule::var_name {
                        Some(p.as_str())
                    } else {
                        p.into_inner()
                            .find(|inner| inner.as_rule() == GrubRule::var_name)
                            .map(|inner| inner.as_str())
                    }
                })
                .unwrap_or("");
            variables.get(var_name).cloned().unwrap_or_default()
        }
        GrubRule::quoted_string => {
            let mut result = String::from("\"");
            for inner in pair.into_inner() {
                match inner.as_rule() {
                    GrubRule::variable => result.push_str(&resolve_pair(inner, variables)),
                    GrubRule::string_text => result.push_str(inner.as_str()),
                    _ => {}
                }
            }
            result.push('"');
            result
        }
        GrubRule::keyvalue => {
            let mut result = String::new();
            for inner in pair.into_inner() {
                match inner.as_rule() {
                    GrubRule::key_part => {
                        result.push_str(inner.as_str());
                        result.push('=');
                    }
                    GrubRule::quoted_string => result.push_str(&resolve_pair(inner, variables)),
                    GrubRule::value_part => result.push_str(inner.as_str()),
                    _ => {}
                }
            }
            result
        }
        GrubRule::compound_word => {
            let mut result = String::new();
            for inner in pair.into_inner() {
                match inner.as_rule() {
                    GrubRule::variable => result.push_str(&resolve_pair(inner, variables)),
                    GrubRule::word_text => result.push_str(inner.as_str()),
                    _ => {}
                }
            }
            result
        }
        _ => pair.as_str().to_string(),
    }
}

/// Resolve a set statement value, which may be a quoted string with variables.
fn resolve_set_value(pair: Pair<'_, GrubRule>, variables: &HashMap<String, String>) -> String {
    match pair.as_rule() {
        GrubRule::quoted_string => {
            let mut result = String::new();
            for inner in pair.into_inner() {
                match inner.as_rule() {
                    GrubRule::variable => result.push_str(&resolve_pair(inner, variables)),
                    GrubRule::string_text => result.push_str(inner.as_str()),
                    _ => {}
                }
            }
            result
        }
        GrubRule::unquoted_set_value => pair.as_str().to_string(),
        _ => pair.as_str().to_string(),
    }
}

/// Parse grub.cfg and extract the kernel command line with variables resolved.
pub fn parse(grub_cfg: &[u8]) -> Result<String> {
    let text = std::str::from_utf8(grub_cfg).whatever_context("grub.cfg is not valid UTF-8")?;

    let pairs =
        GrubParser::parse(GrubRule::config, text).whatever_context("failed to parse grub.cfg")?;

    let mut variables: HashMap<String, String> = HashMap::new();
    let mut linux_args: Vec<String> = Vec::new();

    for pair in pairs.flatten() {
        match pair.as_rule() {
            GrubRule::set_stmt => {
                let mut inner = pair.into_inner();
                if let Some(name) = inner.next() {
                    if let Some(value) = inner.next() {
                        let resolved = resolve_set_value(value, &variables);
                        variables.insert(name.as_str().to_string(), resolved);
                    }
                }
            }
            GrubRule::command => {
                let mut inner = pair.into_inner();
                if let Some(cmd) = inner.next() {
                    if cmd.as_str() == "linux" {
                        for arg in inner {
                            linux_args.push(resolve_pair(arg, &variables));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    Ok(linux_args.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test]
    fn test_parse_full_config() {
        let grub_cfg = br#"
set default="0"
set dm_verity_root="root,,,ro,0 123"

menuentry "test" {
   linux ($root)/vmlinuz \
       console=tty0 \
       dm-mod.create="$dm_verity_root" \
       -- \
       systemd.log_target=journal
   initrd ($private)/bootconfig.data
}
"#;
        let cmdline = parse(grub_cfg).unwrap();
        assert!(cmdline.starts_with("()/vmlinuz"));
        assert!(cmdline.contains("console=tty0"));
        assert!(cmdline.contains("dm-mod.create=\"root,,,ro,0 123\""));
        assert!(cmdline.contains("-- systemd.log_target=journal"));
    }

    // Variable resolution tests
    #[test_case("set foo=bar\nlinux /vmlinuz $foo", "/vmlinuz bar" ; "bare_variable")]
    #[test_case("set foo=bar\nlinux /vmlinuz ${foo}", "/vmlinuz bar" ; "braced_variable")]
    #[test_case("linux /vmlinuz $unknown", "/vmlinuz " ; "unknown_variable")]
    #[test_case("set foo=\"bar\"\nset baz=\"I am a ${foo}\"\nlinux /vmlinuz $baz", "/vmlinuz I am a bar" ; "nested_variable")]
    #[test_case("set a=1\nset b=2\nlinux /vmlinuz $a $b", "/vmlinuz 1 2" ; "multiple_variables")]
    fn test_variable_resolution(input: &str, expected: &str) {
        assert_eq!(parse(input.as_bytes()).unwrap(), expected);
    }

    // Set statement tests
    #[test_case("set foo=bar\nlinux /vmlinuz $foo", "/vmlinuz bar" ; "unquoted_value")]
    #[test_case("set foo=\"bar\"\nlinux /vmlinuz $foo", "/vmlinuz bar" ; "quoted_value")]
    #[test_case("set foo=\"bar baz\"\nlinux /vmlinuz $foo", "/vmlinuz bar baz" ; "quoted_with_spaces")]
    fn test_set_statement(input: &str, expected: &str) {
        assert_eq!(parse(input.as_bytes()).unwrap(), expected);
    }

    // Linux command argument tests
    #[test_case("linux /vmlinuz console=tty0", "/vmlinuz console=tty0" ; "simple_arg")]
    #[test_case("linux /vmlinuz a b c", "/vmlinuz a b c" ; "multiple_args")]
    #[test_case("linux /vmlinuz key=\"value\"", "/vmlinuz key=\"value\"" ; "quoted_value_arg")]
    #[test_case("linux /vmlinuz --", "/vmlinuz --" ; "separator")]
    #[test_case("linux /vmlinuz a -- b", "/vmlinuz a -- b" ; "args_with_separator")]
    fn test_linux_args(input: &str, expected: &str) {
        assert_eq!(parse(input.as_bytes()).unwrap(), expected);
    }

    // Line continuation tests
    #[test_case("linux /vmlinuz \\\n  a \\\n  b", "/vmlinuz a b" ; "line_continuation")]
    #[test_case("set foo=bar\nlinux /vmlinuz \\\n  $foo", "/vmlinuz bar" ; "continuation_with_var")]
    fn test_line_continuation(input: &str, expected: &str) {
        assert_eq!(parse(input.as_bytes()).unwrap(), expected);
    }

    // Comment tests
    #[test_case("# comment\nlinux /vmlinuz a", "/vmlinuz a" ; "comment_line")]
    #[test_case("linux /vmlinuz a # inline comment\n", "/vmlinuz a" ; "inline_comment")]
    fn test_comments(input: &str, expected: &str) {
        assert_eq!(parse(input.as_bytes()).unwrap(), expected);
    }

    // Menuentry tests
    #[test_case("menuentry \"test\" {\n  linux /vmlinuz a\n}", "/vmlinuz a" ; "simple_menuentry")]
    #[test_case("menuentry \"test\" --unrestricted {\n  linux /vmlinuz a\n}", "/vmlinuz a" ; "menuentry_with_option")]
    fn test_menuentry(input: &str, expected: &str) {
        assert_eq!(parse(input.as_bytes()).unwrap(), expected);
    }

    // Kernel path with GRUB device syntax
    #[test_case("linux ($root)/vmlinuz a", "()/vmlinuz a" ; "grub_device_unset")]
    #[test_case("set root=hd0,gpt3\nlinux ($root)/vmlinuz a", "(hd0,gpt3)/vmlinuz a" ; "grub_device_set")]
    fn test_kernel_path(input: &str, expected: &str) {
        assert_eq!(parse(input.as_bytes()).unwrap(), expected);
    }
}
