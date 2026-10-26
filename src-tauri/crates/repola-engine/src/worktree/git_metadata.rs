/// Git administration files terminate a path with LF or CRLF. Spaces are
/// part of that path, including trailing spaces on platforms that allow them.
pub(crate) fn metadata_line(value: &str) -> &str {
    value
        .strip_suffix("\r\n")
        .or_else(|| value.strip_suffix('\n'))
        .unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_only_the_line_terminator_and_preserves_exact_path_spaces() {
        for (value, expected) in [
            (
                "gitdir: folder with spaces \n",
                "gitdir: folder with spaces ",
            ),
            ("../parent with spaces \r\n", "../parent with spaces "),
            (" path without terminator ", " path without terminator "),
        ] {
            assert_eq!(metadata_line(value), expected);
        }
    }
}
