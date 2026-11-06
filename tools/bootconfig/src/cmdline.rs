use std::fmt;

/// Ends the arguments the kernel parses; the rest go to init.
const SEPARATOR: &str = "--";

/// A kernel command line: the arguments the kernel parses, then, after the first `--`, the ones
/// it passes to init. Each element is one argument, and arguments render separated by a space.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Cmdline {
    kernel: Vec<String>,
    init: Vec<String>,
}

impl Cmdline {
    /// Split a command line into arguments as the kernel's `next_arg()` does: at whitespace
    /// outside double quotes, keeping the quotes in the argument.
    pub(crate) fn parse(text: &str) -> Self {
        let mut arguments = Vec::new();
        let mut argument = String::new();
        let mut in_quote = false;
        for character in text.chars() {
            if is_space(character) && !in_quote {
                if !argument.is_empty() {
                    arguments.push(std::mem::take(&mut argument));
                }
                continue;
            }
            if character == '"' {
                in_quote = !in_quote;
            }
            argument.push(character);
        }
        if !argument.is_empty() {
            arguments.push(argument);
        }
        Self::split(arguments)
    }

    pub(crate) fn new(kernel: Vec<String>, init: Vec<String>) -> Self {
        Self { kernel, init }
    }

    /// Split arguments at the first `--`.
    pub(crate) fn split(arguments: impl IntoIterator<Item = String>) -> Self {
        let mut arguments = arguments.into_iter();
        let kernel = arguments
            .by_ref()
            .take_while(|argument| argument != SEPARATOR)
            .collect();
        Self::new(kernel, arguments.collect())
    }

    /// Put `prefix` ahead of this command line as the kernel's `setup_command_line()` adds
    /// bootconfig: its kernel arguments first, and its init arguments right after the `--`.
    pub(crate) fn prepend(self, prefix: Cmdline) -> Self {
        let mut kernel = prefix.kernel;
        kernel.extend(self.kernel);
        let mut init = prefix.init;
        init.extend(self.init);
        Self::new(kernel, init)
    }
}

impl fmt::Display for Cmdline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let separator = (!self.init.is_empty()).then_some(SEPARATOR);
        let parts: Vec<&str> = self
            .kernel
            .iter()
            .map(String::as_str)
            .chain(separator)
            .chain(self.init.iter().map(String::as_str))
            .collect();
        f.write_str(&parts.join(" "))
    }
}

/// The ASCII characters the kernel's `isspace()` accepts.
fn is_space(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r')
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case("a -- b", "k -- i", "k a -- i b" ; "the_prefix_leads_each_side")]
    #[test_case("a", "k -- i", "k a -- i" ; "init_arguments_add_a_separator")]
    #[test_case("a", "k", "k a" ; "no_separator_is_added_without_init_arguments")]
    #[test_case("a -- b -- c", "k -- i", "k a -- i b -- c" ; "only_the_first_separator_splits")]
    fn test_prepend(base: &str, prefix: &str, expected: &str) {
        let base = Cmdline::parse(base);
        let prefix = Cmdline::parse(prefix);
        assert_eq!(base.prepend(prefix).to_string(), expected);
    }

    #[test_case(r#"a dm="x y" -- b"#, &[r#"a"#, r#"dm="x y""#], &["b"] ; "a_quoted_space_stays_in_its_argument")]
    #[test_case(r#"dm="a -- b" -- x"#, &[r#"dm="a -- b""#], &["x"] ; "a_quoted_separator_does_not_split")]
    #[test_case(" a \t b\n", &["a", "b"], &[] ; "runs_of_whitespace_separate_arguments")]
    #[test_case(r#"a "b c"#, &["a", r#""b c"#], &[] ; "an_unpaired_quote_runs_to_the_end")]
    #[test_case("--", &[], &[] ; "a_lone_separator_parses_away")]
    #[test_case("", &[], &[] ; "nothing")]
    fn test_parse(text: &str, kernel: &[&str], init: &[&str]) {
        let to_strings = |arguments: &[&str]| arguments.iter().map(|a| a.to_string()).collect();
        assert_eq!(
            Cmdline::parse(text),
            Cmdline::new(to_strings(kernel), to_strings(init))
        );
    }
}
