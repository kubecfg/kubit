use std::{iter::Sum, ops};

/// A shell script. I renders with a shebang header and sets the strict evaluation flags.
/// Can be combined with other scripts.
pub struct Script(String);

impl Script {
    pub fn from_str(str: &str) -> Self {
        Self(str.to_string())
    }

    pub fn from_vec(tokens: Vec<String>) -> Self {
        Self(
            tokens
                .iter()
                .map(quoted)
                .collect::<Vec<_>>()
                .join(" \\\n    "),
        )
    }

    pub fn subshell(&self) -> Self {
        Self(format!("({})", self.0))
    }
}

// Quote all strings expect for explicit bash variable references and
// redirection.
fn quoted(src: &String) -> String {
    if src.starts_with("${") {
        format!(r#""{src}""#)
    } else if src.starts_with('>') {
        src.to_string()
    } else {
        yash_quote::quoted(src).to_string()
    }
}

impl std::fmt::Display for Script {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "#!/bin/bash")?;
        writeln!(f, "set -euo pipefail")?;
        writeln!(f)?;
        write!(f, "{}", self.0)?;
        Ok(())
    }
}

impl ops::Add<Script> for Script {
    type Output = Script;

    fn add(self, rhs: Script) -> Self::Output {
        Script(format!("{}\n{}", self.0, rhs.0))
    }
}

impl ops::BitOr<Script> for Script {
    type Output = Script;

    fn bitor(self, rhs: Script) -> Self::Output {
        Script(format!("{} \\\n| {}", self.0, rhs.0))
    }
}

impl Sum for Script {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        match iter.reduce(|lhs, rhs| lhs + rhs) {
            Some(script) => script,
            None => Self::from_vec(vec![]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script() {
        let script = Script::from_vec(
            ["echo", "foo", "bar", "baz qux"]
                .into_iter()
                .map(|x| x.to_string())
                .collect(),
        );
        let expected = r#"#!/bin/bash
set -euo pipefail

echo \
    foo \
    bar \
    'baz qux'"#;
        assert_eq!(format!("{script}"), expected);
    }

    #[test]
    fn test_add() {
        let script_a = Script::from_vec(
            ["echo", "foo", "bar"]
                .into_iter()
                .map(|x| x.to_string())
                .collect(),
        );
        let script_b = Script::from_vec(
            ["echo", "baz qux"]
                .into_iter()
                .map(|x| x.to_string())
                .collect(),
        );
        let combined = script_a + script_b;

        let expected = r#"#!/bin/bash
set -euo pipefail

echo \
    foo \
    bar
echo \
    'baz qux'"#;
        assert_eq!(format!("{combined}"), expected);
    }

    #[test]
    fn test_sum() {
        let scripts = vec![
            Script::from_vec(
                ["echo", "foo", "bar"]
                    .into_iter()
                    .map(|x| x.to_string())
                    .collect(),
            ),
            Script::from_vec(
                ["echo", "baz qux"]
                    .into_iter()
                    .map(|x| x.to_string())
                    .collect(),
            ),
        ];

        let combined: Script = scripts.into_iter().sum();

        let expected = r#"#!/bin/bash
set -euo pipefail

echo \
    foo \
    bar
echo \
    'baz qux'"#;
        assert_eq!(format!("{combined}"), expected);
    }

    #[test]
    fn test_variable() {
        let script = Script::from_vec(
            ["echo", "quote_$_me", "${dont_quote_me}"]
                .into_iter()
                .map(|x| x.to_string())
                .collect(),
        );
        let expected = r#"#!/bin/bash
set -euo pipefail

echo \
    'quote_$_me' \
    "${dont_quote_me}""#;
        assert_eq!(format!("{script}"), expected);
    }

    #[test]
    fn test_pipe() {
        let left = Script::from_vec(["echo", "foo"].into_iter().map(|x| x.to_string()).collect());
        let right = Script::from_vec(["wc", "-c"].into_iter().map(|x| x.to_string()).collect());

        let script = left | right;
        let expected = r#"#!/bin/bash
set -euo pipefail

echo \
    foo \
| wc \
    -c"#;
        assert_eq!(format!("{script}"), expected);
    }

    #[test]
    fn test_redirect() {
        let script = Script::from_vec(
            ["echo", "foobar", ">", "/tmp/test"]
                .into_iter()
                .map(|x| x.to_string())
                .collect(),
        );
        let expected = r#"#!/bin/bash
set -euo pipefail

echo \
    foobar \
    > \
    /tmp/test"#;
        assert_eq!(format!("{script}"), expected);
    }

    #[test]
    fn test_quoted() {
        let tests = [
            (&String::from("${MY_VAR}"), format!("\"{}\"", "${MY_VAR}")),
            (&String::from(">"), ">".to_string()),
            (&String::from("hello"), "hello".to_string()),
        ];

        for (input, expected) in tests {
            assert_eq!(quoted(input), expected);
        }
    }
}
