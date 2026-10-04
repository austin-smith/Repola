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
    let mut line = format!("git -C {}", quote_argument(&directory.to_string_lossy()));
    for argument in args {
        line.push(' ');
        line.push_str(&quote_argument(argument.as_ref()));
    }
    line
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

/// Leaves plain words bare and single-quotes anything else. PowerShell treats
/// typographic single quotes as quote characters too, so each one is doubled
/// like `'`. A bare `--` is quoted because Windows PowerShell drops it before
/// running a native command.
#[cfg(windows)]
pub(crate) fn quote_argument(value: &str) -> String {
    let plain = value != "--"
        && !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_./:\\".contains(&byte));
    if plain {
        return value.to_string();
    }
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
    }
}
