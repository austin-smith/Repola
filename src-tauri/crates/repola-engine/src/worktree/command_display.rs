//! Renders Git commands for people to review, copy, and run. Commands themselves
//! are always spawned without a shell; this only affects what is displayed.
//!
//! The engine runs on the machine that owns the repository, so quoting follows
//! the shell a user there pastes into: a POSIX shell on Unix and PowerShell on
//! Windows.

use std::path::Path;

/// `git -C <directory> <args…>`, with every argument quoted for display.
pub(crate) fn git_command_line<I, S>(directory: &Path, args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut arguments = vec!["-C".to_string(), directory.to_string_lossy().into_owned()];
    arguments.extend(
        args.into_iter()
            .map(|argument| argument.as_ref().to_string()),
    );
    shell_command_line(&arguments)
}

fn git_words(arguments: impl Iterator<Item = String>) -> String {
    std::iter::once("git".to_string())
        .chain(arguments)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(unix)]
fn shell_command_line(arguments: &[String]) -> String {
    git_words(arguments.iter().map(|argument| quote_argument(argument)))
}

/// Windows PowerShell 5.1 builds a native command's command line itself: it
/// drops an empty argument, passes an embedded `"` unescaped, and does not
/// escape the trailing backslash of an argument it quotes for whitespace. Every
/// version also takes an argument of exactly `--%` as its stop-parsing token.
/// A command with any of these goes through `Start-Process` instead, which every
/// version passes a command line, encoded here, unchanged.
#[cfg(windows)]
fn shell_command_line(arguments: &[String]) -> String {
    let misread = |argument: &String| {
        argument.is_empty()
            || argument == "--%"
            || argument.contains('"')
            || (argument.ends_with('\\') && argument.chars().any(char::is_whitespace))
    };
    if !arguments.iter().any(misread) {
        return git_words(arguments.iter().map(|argument| quote_argument(argument)));
    }
    let command_line = arguments
        .iter()
        .map(|argument| encode_argument(argument))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "Start-Process git -ArgumentList {} -NoNewWindow -Wait",
        single_quoted(&command_line)
    )
}

/// `value` encoded the way Windows programs decode their command line: quoted
/// when it is empty or holds whitespace or `"`, with each `"` escaped and the
/// backslashes before an escaped or closing `"` doubled.
#[cfg(windows)]
fn encode_argument(value: &str) -> String {
    if !value.is_empty() && !value.contains('"') && !value.chars().any(char::is_whitespace) {
        return value.to_string();
    }
    let mut encoded = String::from('"');
    let mut backslashes = 0;
    for character in value.chars() {
        match character {
            '\\' => backslashes += 1,
            '"' => {
                encoded.push_str(&"\\".repeat(backslashes * 2 + 1));
                encoded.push('"');
                backslashes = 0;
            }
            other => {
                encoded.push_str(&"\\".repeat(backslashes));
                encoded.push(other);
                backslashes = 0;
            }
        }
    }
    encoded.push_str(&"\\".repeat(backslashes * 2));
    encoded.push('"');
    encoded
}

/// Leaves plain words bare and single-quotes anything else, closing and
/// reopening the quotes around an embedded `'`.
#[cfg(unix)]
pub(crate) fn quote_argument(value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_./:@+,".contains(&byte));
    if plain {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Leaves plain words bare and single-quotes anything else. PowerShell parses
/// a bare word starting with `-` as a parameter, which it can split at a `.`
/// or join to the next argument after a `:`, and Windows PowerShell drops a
/// bare `--`; so only letters, digits, and `-` may follow a leading `-`.
#[cfg(windows)]
pub(crate) fn quote_argument(value: &str) -> String {
    let plain = match value.strip_prefix('-') {
        Some(rest) => {
            rest.bytes().any(|byte| byte.is_ascii_alphanumeric())
                && rest
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        }
        None => {
            !value.is_empty()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_./:\\".contains(&byte))
        }
    };
    if plain {
        return value.to_string();
    }
    single_quoted(value)
}

/// `value` in single quotes. PowerShell treats typographic single quotes as
/// quote characters too, so each one is doubled like `'`.
#[cfg(windows)]
fn single_quoted(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for character in value.chars() {
        if matches!(
            character,
            '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}'
        ) {
            quoted.push(character);
        }
        quoted.push(character);
    }
    quoted.push('\'');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn posix_quoting_leaves_plain_words_and_quotes_the_rest() {
        assert_eq!(quote_argument("feature/one"), "feature/one");
        assert_eq!(quote_argument("--"), "--");
        assert_eq!(quote_argument("-D"), "-D");
        assert_eq!(quote_argument(""), "''");
        assert_eq!(quote_argument("two words"), "'two words'");
        assert_eq!(quote_argument("it's"), r"'it'\''s'");
        assert_eq!(quote_argument("$HOME"), "'$HOME'");
        assert_eq!(quote_argument("~/repo"), "'~/repo'");
        assert_eq!(quote_argument("=ls"), "'=ls'");
        assert_eq!(quote_argument("a;b"), "'a;b'");
        assert_eq!(
            quote_argument("--force-with-lease=refs/heads/x:abc"),
            "'--force-with-lease=refs/heads/x:abc'"
        );
    }

    #[cfg(unix)]
    #[test]
    fn posix_command_lines_quote_the_directory_and_each_argument() {
        assert_eq!(
            git_command_line(Path::new("/work/my repo"), ["branch", "-d", "--", "it's"]),
            r"git -C '/work/my repo' branch -d -- 'it'\''s'"
        );
    }

    #[cfg(windows)]
    #[test]
    fn powershell_quoting_leaves_plain_words_and_quotes_the_rest() {
        assert_eq!(quote_argument("feature/one"), "feature/one");
        assert_eq!(quote_argument(r"C:\work\repo"), r"C:\work\repo");
        assert_eq!(quote_argument("--"), "'--'");
        assert_eq!(quote_argument("-D"), "-D");
        assert_eq!(quote_argument(""), "''");
        assert_eq!(quote_argument("two words"), "'two words'");
        assert_eq!(quote_argument("it's"), "'it''s'");
        assert_eq!(quote_argument("it\u{2019}s"), "'it\u{2019}\u{2019}s'");
        assert_eq!(quote_argument("$env:HOME"), "'$env:HOME'");
        assert_eq!(quote_argument("a`b"), "'a`b'");
        assert_eq!(quote_argument("@(x)"), "'@(x)'");
        assert_eq!(quote_argument("--no-deref"), "--no-deref");
        assert_eq!(quote_argument("-foo.bar"), "'-foo.bar'");
        assert_eq!(quote_argument("-foo:"), "'-foo:'");
        assert_eq!(quote_argument("-"), "'-'");
        assert_eq!(
            quote_argument("--force-with-lease=refs/heads/x:abc"),
            "'--force-with-lease=refs/heads/x:abc'"
        );
    }

    #[cfg(windows)]
    #[test]
    fn powershell_command_lines_quote_the_directory_and_each_argument() {
        assert_eq!(
            git_command_line(
                Path::new(r"C:\work\my repo"),
                ["branch", "-d", "--", "it's"]
            ),
            r"git -C 'C:\work\my repo' branch -d '--' 'it''s'"
        );
        assert_eq!(
            git_command_line(
                Path::new(r"C:\work\my repo"),
                ["config", "--add", "branch.it's.x", ""]
            ),
            r#"Start-Process git -ArgumentList '-C "C:\work\my repo" config --add branch.it''s.x ""' -NoNewWindow -Wait"#
        );
        assert_eq!(
            git_command_line(Path::new(r"C:\repo"), ["branch", "--", r#"a"b"#, r"a b\"]),
            r#"Start-Process git -ArgumentList '-C C:\repo branch -- a\"b "a b\\"' -NoNewWindow -Wait"#
        );
    }

    /// Runs each displayed command in Windows PowerShell 5.1 and in PowerShell 7,
    /// which must both pass Git exactly the arguments it gets when spawned
    /// directly. Git's trace records them, whatever the console encoding.
    #[cfg(windows)]
    #[test]
    fn both_powershells_pass_git_exactly_the_displayed_arguments() {
        use crate::worktree::command;
        use std::ffi::OsStr;

        let values = [
            "",
            "\"",
            "a\"b",
            "\\\"",
            "a\\\\\"b",
            "\"a b\"",
            "a\"b c\"d e",
            "a b\\",
            "a b\\\\",
            "\\\\server\\share x\\",
            "a\\b c",
            "a\u{a0}b\\",
            "--%",
            "--",
            "-",
            "-D",
            "--no-deref",
            "-foo.bar",
            "-foo:",
            "-foo:bar",
            "-a1.5",
            "-5",
            "0123",
            "1kb",
            "1e3",
            "0x10",
            ".5",
            "two words",
            "a  b",
            "a\tb",
            "it's",
            "it\u{2019}s",
            "\u{2018}q\u{2019}",
            "\u{201A}\u{201B}",
            "$env:HOME",
            "a`b",
            "@(x)",
            "a;b",
            "a|b",
            "a&b",
            "<x>",
            "{x}",
            "(x)",
            "#x",
            "a#b",
            "a,b",
            "~",
            "~/x",
            "%PATH%",
            "a=b",
            "feature/one",
            r"C:\work\repo",
            "refs/heads/x:abc",
            "na\u{ef}ve \u{65e5}\u{672c}",
        ];
        let temp = tempfile::tempdir().expect("temp directory");
        let directory = temp.path().join("it's \u{2018}a\u{2019} $repo");
        std::fs::create_dir(&directory).expect("directory");
        let traced = |trace: &Path| {
            let text = std::fs::read(trace).unwrap_or_else(|error| panic!("{trace:?}: {error}"));
            String::from_utf8_lossy(&text)
                .lines()
                .find_map(|line| {
                    line.split_once("trace: built-in: ")
                        .map(|(_, argv)| argv.to_string())
                })
                .unwrap_or_else(|| panic!("{trace:?} records no command"))
        };
        let direct: Vec<String> = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let trace = temp.path().join(format!("direct-{index}.trace"));
                let output = command::git_at_with_env(
                    &directory,
                    ["rev-parse", "--sq-quote", value],
                    [("GIT_TRACE", trace.as_os_str())],
                )
                .expect("git");
                assert!(output.status.success(), "{value:?}");
                traced(&trace)
            })
            .collect();

        for shell in ["powershell", "pwsh"] {
            let mut script = String::from('\u{feff}');
            for (index, value) in values.iter().enumerate() {
                let trace = temp.path().join(format!("{shell}-{index}.trace"));
                script.push_str(&format!(
                    "$env:GIT_TRACE = {}\n{}\n",
                    single_quoted(&trace.to_string_lossy()),
                    git_command_line(&directory, ["rev-parse", "--sq-quote", value])
                ));
            }
            let path = temp.path().join(format!("{shell}.ps1"));
            std::fs::write(&path, script).expect("script");
            let output = command::output(
                shell,
                [
                    OsStr::new("-NoLogo"),
                    OsStr::new("-NoProfile"),
                    OsStr::new("-NonInteractive"),
                    OsStr::new("-ExecutionPolicy"),
                    OsStr::new("Bypass"),
                    OsStr::new("-File"),
                    path.as_os_str(),
                ],
            )
            .unwrap_or_else(|error| panic!("{shell}: {error}"));
            assert!(
                output.status.success(),
                "{shell}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            for (index, value) in values.iter().enumerate() {
                assert_eq!(
                    traced(&temp.path().join(format!("{shell}-{index}.trace"))),
                    direct[index],
                    "{shell} misread {value:?} from {}",
                    git_command_line(&directory, ["rev-parse", "--sq-quote", value])
                );
            }
        }
    }
}
