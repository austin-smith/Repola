use crate::protocol::{AgentError, AgentResult};

const SECRET_KEYS: &[&str] = &[
    "authorization",
    "access_token",
    "api_key",
    "apikey",
    "client_secret",
    "password",
    "private_key",
    "refresh_token",
    "secret",
    "token",
];

pub(crate) fn redact_agent_result(mut result: AgentResult) -> AgentResult {
    match &mut result {
        AgentResult::Commit { result } => result.hook_output = redact(&result.hook_output),
        AgentResult::Synchronize { result } => result.output = redact(&result.output),
        AgentResult::RepositoryOperation { result } => result.output = redact(&result.output),
        AgentResult::RepositoryOperationMutation { result } => {
            result.output = redact(&result.output)
        }
        AgentResult::HistoryMutation { result } => result.output = redact(&result.output),
        AgentResult::TagMutation { result } => result.output = redact(&result.output),
        AgentResult::WorktreeCreated { result } => result.output = redact(&result.output),
        AgentResult::StashMutation { result } => result.output = redact(&result.output),
        AgentResult::BranchDeletion { result } => {
            for step in [&mut result.local, &mut result.remote]
                .into_iter()
                .flatten()
            {
                step.output = redact(&step.output);
                step.warning = step.warning.as_deref().map(redact);
            }
        }
        _ => {}
    }
    result
}

pub(crate) fn redact_agent_error(mut error: AgentError) -> AgentError {
    error.summary = redact(&error.summary);
    error.detail = error.detail.map(|detail| redact(&detail));
    error
}

pub(crate) fn redact(input: &str) -> String {
    let mut output = redact_url_credentials(input);
    output = redact_assignments(&output);
    redact_known_tokens(&output)
}

fn redact_url_credentials(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut remaining = input;
    while let Some(scheme) = remaining.find("://") {
        let authority_start = scheme + 3;
        let authority_end = remaining[authority_start..]
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '/' | '?' | '#')
            })
            .map(|offset| authority_start + offset)
            .unwrap_or(remaining.len());
        let authority = &remaining[authority_start..authority_end];
        if let Some(at) = authority.rfind('@') {
            output.push_str(&remaining[..authority_start]);
            output.push_str("[REDACTED]@");
            output.push_str(&authority[at + 1..]);
            remaining = &remaining[authority_end..];
        } else {
            output.push_str(&remaining[..authority_end]);
            remaining = &remaining[authority_end..];
        }
    }
    output.push_str(remaining);
    output
}

fn redact_assignments(input: &str) -> String {
    input
        .split_inclusive('\n')
        .map(redact_line_assignments)
        .collect()
}

fn redact_line_assignments(line: &str) -> String {
    let mut output = line.to_string();
    let mut search_from = 0;

    loop {
        let lower = output.to_ascii_lowercase();
        let Some((key_start, value_start, key)) = next_secret(&lower, search_from) else {
            break;
        };

        let content_end = output.trim_end_matches(['\r', '\n']).len();
        let (replacement_start, value_end) = if key == "authorization" {
            (value_start, content_end)
        } else {
            value_bounds(&output, value_start, content_end)
        };

        output.replace_range(replacement_start..value_end, "[REDACTED]");
        search_from = replacement_start + "[REDACTED]".len();
        if search_from <= key_start {
            break;
        }
    }

    output
}

fn next_secret(lower: &str, search_from: usize) -> Option<(usize, usize, &'static str)> {
    SECRET_KEYS
        .iter()
        .flat_map(|key| ["=", ":"].map(move |separator| (*key, separator)))
        .filter_map(|(key, separator)| {
            let needle = format!("{key}{separator}");
            lower[search_from..]
                .find(&needle)
                .map(|offset| (search_from + offset, needle.len(), key))
        })
        .min_by_key(|(start, _, _)| *start)
        .map(|(start, needle_len, key)| {
            let mut value_start = start + needle_len;
            while lower.as_bytes().get(value_start) == Some(&b' ') {
                value_start += 1;
            }
            (start, value_start, key)
        })
}

fn value_bounds(line: &str, value_start: usize, content_end: usize) -> (usize, usize) {
    let bytes = line.as_bytes();
    let quote = bytes
        .get(value_start)
        .copied()
        .filter(|byte| matches!(byte, b'\'' | b'"'));
    let replacement_start = value_start + usize::from(quote.is_some());
    let value_end = line[replacement_start..content_end]
        .char_indices()
        .find(|(_, character)| match quote {
            Some(quote) => *character as u32 == u32::from(quote),
            None => character.is_whitespace() || matches!(character, '&' | ',' | ';'),
        })
        .map(|(offset, _)| replacement_start + offset)
        .unwrap_or(content_end);
    (replacement_start, value_end)
}

fn redact_known_tokens(input: &str) -> String {
    let mut output = input.to_string();
    for prefix in [
        "github_pat_",
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "glpat-",
    ] {
        while let Some(start) = output.find(prefix) {
            let end = output[start..]
                .find(|character: char| {
                    character.is_whitespace() || matches!(character, '"' | '\'' | ',' | ';')
                })
                .map(|offset| start + offset)
                .unwrap_or(output.len());
            output.replace_range(start..end, "[REDACTED]");
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_credentials_without_hiding_hosts_or_useful_diagnostics() {
        let input = "fatal: https://alice:secret@example.com/org/repo.git\nAuthorization: Bearer abc123\nTOKEN=xyz API_KEY='also-secret' command failed\nghp_abcdefghijklmnopqrstuvwxyz";
        let redacted = redact(input);
        assert!(redacted.contains("https://[REDACTED]@example.com/org/repo.git"));
        assert!(redacted.contains("Authorization: [REDACTED]"));
        assert!(redacted.contains("TOKEN=[REDACTED]"));
        assert!(redacted.contains("API_KEY='[REDACTED]'"));
        assert!(!redacted.contains("alice:secret"));
        assert!(!redacted.contains("abc123"));
        assert!(!redacted.contains("also-secret"));
        assert!(!redacted.contains("ghp_"));
    }
}
