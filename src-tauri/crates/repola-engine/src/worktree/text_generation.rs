//! Provider-independent selected-change preparation and response validation.
use super::models::{GenerateCommitMessageRequest, GeneratedCommitMessage, TextGenerationStatus};
use super::working_copy::{selected_commit_context, SelectedCommitContext};
use crate::preferences::TextGenerationProvider;
use serde::Deserialize;

const MAX_PROMPT_PATCH_CHARS: usize = 60_000;
const MAX_FILE_SUMMARY_CHARS: usize = 20_000;
const MAX_ADDITIONAL_INSTRUCTIONS_CHARS: usize = 20_000;
const MAX_BODY_CHARS: usize = 16_000;
pub(super) const OUTPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "subject": { "type": "string", "minLength": 1, "maxLength": 72 },
    "body": { "type": "string", "maxLength": 16000 }
  },
  "required": ["subject", "body"],
  "additionalProperties": false
}"#;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitMessage {
    subject: String,
    body: String,
}

fn commit_message_prompt(context: &SelectedCommitContext) -> String {
    let mut instructions = vec![
        "Follow the repository's established commit message style when examples are available."
            .to_string(),
    ];
    if !context.recent_subjects.is_empty() {
        instructions.push(format!(
            "Recent commit subjects from this repository:\n{}",
            context.recent_subjects.join("\n")
        ));
    }
    if !context.repository_instructions.is_empty() {
        instructions.push(context.repository_instructions.clone());
    }
    format!(
        "You write concise git commit messages.\n\
Return a JSON object with keys: subject, body.\n\
Rules:\n\
- subject must be imperative, <= 72 chars, and no trailing period\n\
- body can be empty string or short bullet points\n\
- capture the primary user-visible or developer-visible change\n\
\n\
Additional instructions:\n{}\n\
\n\
Branch: {}\n\
\n\
Selected files:\n{}\n\
\n\
Selected patch:\n{}",
        truncate_with_marker(
            &instructions.join("\n\n"),
            MAX_ADDITIONAL_INSTRUCTIONS_CHARS,
            "additional instructions"
        ),
        context.branch.as_deref().unwrap_or("(detached)"),
        truncate_with_marker(&context.changed_files, MAX_FILE_SUMMARY_CHARS, "file list"),
        truncate_with_marker(&context.patch, MAX_PROMPT_PATCH_CHARS, "patch"),
    )
}

pub(super) fn truncate_with_marker(value: &str, maximum: usize, label: &str) -> String {
    if value.chars().count() <= maximum {
        return value.to_string();
    }
    let prefix: String = value.chars().take(maximum).collect();
    format!("{prefix}\n\n[{label} truncated by Repola]")
}

fn parse_generated_message(raw: &str) -> Result<GeneratedCommitMessage, String> {
    let decoded: CommitMessage = serde_json::from_str(raw.trim()).map_err(|_| {
        "The provider returned an invalid commit message. Generate it again.".to_string()
    })?;
    let subject = decoded
        .subject
        .trim()
        .trim_end_matches('.')
        .trim()
        .to_string();
    if subject.is_empty() {
        return Err("The provider returned an empty commit subject. Generate it again.".into());
    }
    let body = decoded.body.trim().to_string();
    if subject.chars().count() > 72
        || subject.chars().any(char::is_control)
        || body.chars().count() > MAX_BODY_CHARS
        || body
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(
            "The provider returned an invalid or oversized commit message. Generate it again."
                .into(),
        );
    }
    Ok(GeneratedCommitMessage { subject, body })
}

pub fn text_generation_status(provider: TextGenerationProvider) -> TextGenerationStatus {
    match provider {
        TextGenerationProvider::Codex => super::codex::status(),
        TextGenerationProvider::Claude => super::claude::status(),
    }
}

pub fn generate_commit_message(
    request: GenerateCommitMessageRequest,
) -> Result<GeneratedCommitMessage, String> {
    generate_with(&request, |prompt| {
        let selection = request.text_generation_selection.as_ref();
        match selection.map(|value| value.provider).unwrap_or_default() {
            TextGenerationProvider::Codex => super::codex::generate(prompt, selection),
            TextGenerationProvider::Claude => super::claude::generate(prompt, selection),
        }
    })
}

fn generate_with(
    request: &GenerateCommitMessageRequest,
    generate: impl FnOnce(&str) -> Result<String, String>,
) -> Result<GeneratedCommitMessage, String> {
    if crate::operation::current_operation().is_cancelled() {
        return Err("Commit-message generation was cancelled.".into());
    }
    let context = selected_commit_context(request)?;
    let prompt = commit_message_prompt(&context);
    let raw = generate(&prompt)?;
    if crate::operation::current_operation().is_cancelled() {
        return Err("Commit-message generation was cancelled.".into());
    }
    let generated = parse_generated_message(&raw)?;
    if selected_commit_context(request)? != context {
        return Err(
            "The included changes changed while writing. Generate the message again.".into(),
        );
    }
    Ok(generated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, GenerateCommitMessageRequest) {
        use super::super::models::{CommitFileSelection, WorkingCopyRequest};
        use super::super::{command, working_copy::working_copy_snapshot};
        let directory = tempfile::Builder::new()
            .prefix("repola generation ; ")
            .tempdir()
            .unwrap();
        for args in [
            vec!["init"],
            vec!["config", "core.autocrlf", "false"],
            vec!["config", "user.name", "Repola Test"],
            vec!["config", "user.email", "repola@example.invalid"],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            command::successful_git_at(directory.path(), args).unwrap();
        }
        std::fs::write(directory.path().join("file.txt"), "before\n").unwrap();
        command::successful_git_at(directory.path(), ["add", "."]).unwrap();
        command::successful_git_at(directory.path(), ["commit", "-m", "base"]).unwrap();
        std::fs::write(directory.path().join("file.txt"), "after\n").unwrap();
        let path = directory.path().to_string_lossy().into_owned();
        let snapshot = working_copy_snapshot(WorkingCopyRequest {
            repository_path: path.clone(),
            worktree_path: path.clone(),
        })
        .unwrap();
        let change = &snapshot.changes[0];
        let request = GenerateCommitMessageRequest {
            repository_path: path.clone(),
            worktree_path: path,
            expected_head: snapshot.head,
            included_changes: vec![CommitFileSelection {
                path: change.path.clone(),
                previous_path: change.previous_path.clone(),
                expected_index_status: change.index_status.clone(),
                expected_worktree_status: change.worktree_status.clone(),
                include_all: true,
                hunks: Vec::new(),
            }],
            amend: false,
            text_generation_selection: None,
        };
        (directory, request)
    }

    #[test]
    fn generation_revalidates_content_even_when_git_status_does_not_change() {
        let (directory, request) = fixture();
        let result = generate_with(&request, |prompt| {
            assert!(prompt.contains("+after"));
            std::fs::write(directory.path().join("file.txt"), "newer\n").unwrap();
            Ok(r#"{"subject":"change file","body":""}"#.into())
        });
        assert!(result.unwrap_err().contains("included changes changed"));
    }

    #[test]
    fn failed_and_cancelled_generations_cannot_return_a_suggestion() {
        let (_directory, request) = fixture();
        assert!(generate_with(&request, |_| Err("provider failed".into())).is_err());
        let token = crate::operation::OperationToken::new();
        let result = crate::operation::with_operation(token.clone(), || {
            generate_with(&request, |_| {
                token.cancel();
                Ok(r#"{"subject":"change file","body":""}"#.into())
            })
        });
        assert!(result.unwrap_err().contains("cancelled"));
    }

    #[test]
    fn successful_generation_leaves_the_real_index_unchanged() {
        let (directory, request) = fixture();
        let index = directory.path().join(".git/index");
        let before = std::fs::read(&index).unwrap();
        let object_paths = || {
            walkdir::WalkDir::new(directory.path().join(".git/objects"))
                .into_iter()
                .map(|entry| entry.unwrap().into_path())
                .collect::<std::collections::BTreeSet<_>>()
        };
        let objects_before = object_paths();
        let result = generate_with(&request, |_| {
            Ok(r#"{"subject":"change file","body":""}"#.into())
        })
        .unwrap();
        assert_eq!(result.subject, "change file");
        assert_eq!(std::fs::read(index).unwrap(), before);
        assert_eq!(object_paths(), objects_before);
    }
    #[test]
    fn prompt_matches_the_agreed_wording_with_real_context() {
        let prompt = commit_message_prompt(&SelectedCommitContext {
            tree_oid: "test-tree".into(),
            branch: Some("improve-navigation".into()),
            changed_files: "M\tsrc/app.rs".into(),
            patch: "+new navigation".into(),
            recent_subjects: vec!["polish repository navigation".into()],
            repository_instructions: "Local AGENTS.md:\nUse lowercase commit subjects.".into(),
        });
        assert_eq!(
            prompt,
            "You write concise git commit messages.
Return a JSON object with keys: subject, body.
Rules:
- subject must be imperative, <= 72 chars, and no trailing period
- body can be empty string or short bullet points
- capture the primary user-visible or developer-visible change

Additional instructions:
Follow the repository's established commit message style when examples are available.

Recent commit subjects from this repository:
polish repository navigation

Local AGENTS.md:
Use lowercase commit subjects.

Branch: improve-navigation

Selected files:
M\tsrc/app.rs

Selected patch:
+new navigation"
        );
    }

    #[test]
    fn prompt_handles_missing_guidance_and_bounds_unicode_instructions() {
        let mut context = SelectedCommitContext {
            tree_oid: "test-tree".into(),
            branch: None,
            changed_files: "A\tfile".into(),
            patch: "+new file".into(),
            recent_subjects: vec![],
            repository_instructions: String::new(),
        };
        let prompt = commit_message_prompt(&context);
        assert!(prompt.contains("Branch: (detached)"));
        assert!(!prompt.contains("Recent commit subjects"));
        assert!(!prompt.contains("Local AGENTS.md"));
        context.repository_instructions = "é".repeat(MAX_ADDITIONAL_INSTRUCTIONS_CHARS + 1);
        let prompt = commit_message_prompt(&context);
        let instructions = prompt
            .split_once("Additional instructions:\n")
            .unwrap()
            .1
            .split_once("\n\nBranch:")
            .unwrap()
            .0;
        let content = instructions
            .strip_suffix("\n\n[additional instructions truncated by Repola]")
            .unwrap();
        assert_eq!(content.chars().count(), MAX_ADDITIONAL_INSTRUCTIONS_CHARS);
        assert!(prompt.ends_with("Selected patch:\n+new file"));
    }

    #[test]
    fn generation_loads_worktree_guidance_for_the_selected_provider() {
        let (directory, mut request) = fixture();
        std::fs::write(
            directory.path().join("AGENTS.md"),
            "Use lowercase subjects.",
        )
        .unwrap();
        std::fs::write(directory.path().join("CLAUDE.md"), "Use short bodies.").unwrap();
        generate_with(&request, |prompt| {
            assert!(prompt.contains("Recent commit subjects from this repository:\nbase"));
            assert!(prompt.contains("Local AGENTS.md:\nUse lowercase subjects."));
            assert!(!prompt.contains("Local CLAUDE.md"));
            Ok(r#"{"subject":"change file","body":""}"#.into())
        })
        .unwrap();
        request.text_generation_selection = Some(crate::preferences::TextGenerationSelection {
            provider: TextGenerationProvider::Claude,
            model: None,
            reasoning_effort: None,
        });
        generate_with(&request, |prompt| {
            assert!(prompt.contains("Local AGENTS.md:\nUse lowercase subjects."));
            assert!(prompt.contains("Local CLAUDE.md:\nUse short bodies."));
            Ok(r#"{"subject":"change file","body":""}"#.into())
        })
        .unwrap();
    }

    #[test]
    fn generation_revalidates_repository_guidance() {
        let (directory, request) = fixture();
        std::fs::write(directory.path().join("AGENTS.md"), "Use short subjects.").unwrap();
        let result = generate_with(&request, |_| {
            std::fs::write(
                directory.path().join("AGENTS.md"),
                "Use conventional commits.",
            )
            .unwrap();
            Ok(r#"{"subject":"change file","body":""}"#.into())
        });
        assert!(result.unwrap_err().contains("changed while writing"));
    }

    #[test]
    fn generated_messages_are_bounded_and_normalized() {
        let result = parse_generated_message(
            r#"{"subject":"  improve commit generation.", "body":"  Details.  "}"#,
        )
        .expect("valid response");
        assert_eq!(result.subject, "improve commit generation");
        assert_eq!(result.body, "Details.");
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            serde_json::json!({"subject": "improve commit generation", "body": "Details."})
        );
        let schema: serde_json::Value = serde_json::from_str(OUTPUT_SCHEMA).unwrap();
        assert_eq!(schema["required"], serde_json::json!(["subject", "body"]));
    }

    #[test]
    fn malformed_or_empty_results_fail_instead_of_inventing_copy() {
        assert!(parse_generated_message("not json").is_err());
        assert!(parse_generated_message(r#"{"summary":"old fields","description":""}"#).is_err());
        assert!(parse_generated_message(r#"{"subject":" ","body":"body"}"#).is_err());
        assert!(parse_generated_message(r#"{"subject":"first\nsecond","body":""}"#).is_err());
        assert!(parse_generated_message(
            &serde_json::json!({"subject": "x".repeat(73), "body": ""}).to_string()
        )
        .is_err());
    }
}
