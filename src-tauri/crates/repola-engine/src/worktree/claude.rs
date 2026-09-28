//! Claude Code's non-interactive structured-output adapter.
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use super::command::{self, CommandError};
use super::model_catalog::manifest;
use super::models::{
    ReasoningEffort, TextGenerationModel, TextGenerationStatus, TextGenerationStatusKind,
};
use super::text_generation::OUTPUT_SCHEMA;
use crate::preferences::{TextGenerationProvider, TextGenerationSelection};

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(20);
const GENERATION_TIMEOUT: Duration = Duration::from_secs(180);
// Version, help, and authentication are checked sequentially, including before generation.
pub(super) const STATUS_BUDGET: Duration = DISCOVERY_TIMEOUT.saturating_mul(3);
pub(super) const GENERATION_BUDGET: Duration = STATUS_BUDGET.saturating_add(GENERATION_TIMEOUT);
const REQUIRED_FLAGS: &[&str] = &[
    "--restricted",
    "--safe-mode",
    "--tools",
    "--disallowedTools",
    "--strict-mcp-config",
    "--mcp-config",
    "--no-session-persistence",
    "--no-chrome",
    "--json-schema",
    "--output-format",
    "--model",
    "--effort",
];

fn probe(directory: &Path, args: &[&str]) -> Result<std::process::Output, CommandError> {
    command::output_at_with_input_timeout(directory, "claude", args, b"", DISCOVERY_TIMEOUT)
}

fn cli_version(output: &str) -> Option<[u32; 3]> {
    let version = output.split_whitespace().next()?;
    let parts = version
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    parts.try_into().ok()
}

fn compatible_models(version: [u32; 3]) -> Vec<TextGenerationModel> {
    let policy = &manifest().claude;
    policy
        .models
        .iter()
        .filter(|model| version >= model.minimum_cli_version)
        .map(|model| TextGenerationModel {
            model: model.id.clone(),
            display_name: model.label.clone(),
            description: String::new(),
            is_default: model.id == policy.default_model,
            recommended_for_commit_messages: model.recommended,
            upgrade: None,
            default_reasoning_effort: model.default_reasoning.clone(),
            supported_reasoning_efforts: model
                .reasoning
                .iter()
                .map(|effort| ReasoningEffort {
                    reasoning_effort: effort.clone(),
                    description: String::new(),
                })
                .collect(),
        })
        .collect()
}

pub(super) fn status() -> TextGenerationStatus {
    let mut result = TextGenerationStatus {
        status: TextGenerationStatusKind::Unavailable,
        detail: None,
        version: None,
        models: Vec::new(),
        recommended_selection: None,
    };
    let Ok(temporary) = tempfile::tempdir() else {
        return result;
    };
    let output = match probe(temporary.path(), &["--version"]) {
        Ok(output) if output.status.success() => output,
        Err(CommandError::Launch { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            result.status = TextGenerationStatusKind::NotInstalled;
            return result;
        }
        _ => return result,
    };
    let version_text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let Some(version) = cli_version(&version_text) else {
        return result;
    };
    result.version = Some(version_text);
    let Ok(help) = probe(temporary.path(), &["--help"]) else {
        return result;
    };
    if !help.status.success() {
        return result;
    }
    if version < [2, 1, 248] || !supports_isolation(&String::from_utf8_lossy(&help.stdout)) {
        result.status = TextGenerationStatusKind::UpdateRequired;
        return result;
    }
    let Ok(auth) = probe(temporary.path(), &["auth", "status", "--json"]) else {
        return result;
    };
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct AuthStatus {
        logged_in: bool,
    }
    let Ok(auth_status) = serde_json::from_slice::<AuthStatus>(&auth.stdout) else {
        return result;
    };
    if !auth_status.logged_in {
        result.status = TextGenerationStatusKind::SignedOut;
        return result;
    }
    if !auth.status.success() {
        return result;
    }
    result.models = compatible_models(version);
    result.recommended_selection =
        result
            .models
            .iter()
            .find(|model| model.is_default)
            .map(|model| TextGenerationSelection {
                provider: TextGenerationProvider::Claude,
                model: Some(model.model.clone()),
                reasoning_effort: model.default_reasoning_effort.clone(),
            });
    if result.recommended_selection.is_some() {
        result.status = TextGenerationStatusKind::Ready;
    }
    result
}

fn supports_isolation(help: &str) -> bool {
    REQUIRED_FLAGS.iter().all(|flag| {
        help.split_whitespace()
            .any(|word| word.trim_end_matches(',') == *flag)
    })
}

fn arguments(selection: &TextGenerationSelection) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "--print",
        "--restricted",
        "--safe-mode",
        "--no-session-persistence",
        "--no-chrome",
        "--tools",
        "",
        "--disallowedTools",
        "*",
        "--strict-mcp-config",
        "--mcp-config",
        r#"{"mcpServers":{}}"#,
        "--output-format",
        "json",
        "--json-schema",
        OUTPUT_SCHEMA,
        "--model",
        selection.model.as_deref().expect("resolved model"),
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    if let Some(effort) = &selection.reasoning_effort {
        args.extend([OsString::from("--effort"), OsString::from(effort)]);
    }
    args
}

fn resolve_selection(
    requested: Option<&TextGenerationSelection>,
    status: &TextGenerationStatus,
) -> Result<TextGenerationSelection, String> {
    let selection = requested
        .filter(|selection| selection.model.is_some())
        .or(status.recommended_selection.as_ref())
        .ok_or("Claude has no compatible model. Check Settings.")?;
    let model = status.models.iter().find(|model| Some(&model.model) == selection.model.as_ref())
        .ok_or("The selected Claude model is unavailable in this CLI version. Choose another model in Settings.")?;
    let valid_effort = match &selection.reasoning_effort {
        Some(effort) => model
            .supported_reasoning_efforts
            .iter()
            .any(|option| &option.reasoning_effort == effort),
        None => model.supported_reasoning_efforts.is_empty(),
    };
    if selection.provider != TextGenerationProvider::Claude || !valid_effort {
        return Err("The selected Claude reasoning level is unsupported. Check Settings.".into());
    }
    Ok(selection.clone())
}

pub(super) fn generate(
    prompt: &str,
    requested: Option<&TextGenerationSelection>,
) -> Result<String, String> {
    let status = status();
    match status.status {
        TextGenerationStatusKind::Ready => {}
        TextGenerationStatusKind::NotInstalled => {
            return Err("Install Claude Code on this machine, then run claude auth login.".into())
        }
        TextGenerationStatusKind::SignedOut => {
            return Err("Run claude auth login on this machine, then try again.".into())
        }
        TextGenerationStatusKind::UpdateRequired => {
            return Err("Update Claude Code to use isolated commit-message generation.".into())
        }
        TextGenerationStatusKind::Unavailable => {
            return Err("Claude Code could not be checked. Check Settings and try again.".into())
        }
    }
    let selection = resolve_selection(requested, &status)?;
    let temporary = tempfile::tempdir().map_err(|error| error.to_string())?;
    let output = command::output_at_with_input_timeout(
        temporary.path(),
        "claude",
        arguments(&selection),
        prompt.as_bytes(),
        GENERATION_TIMEOUT,
    )
    .map_err(|error| format!("Claude: {error}"))?;
    parse_output(&output.stdout, output.status.success())
}

fn parse_output(stdout: &[u8], success: bool) -> Result<String, String> {
    #[derive(Deserialize)]
    struct ResultEnvelope {
        #[serde(rename = "type")]
        kind: String,
        subtype: String,
        is_error: bool,
        structured_output: Option<serde_json::Value>,
    }
    if stdout.len() > 128 * 1024 {
        return Err("Claude returned an oversized result.".into());
    }
    let result: ResultEnvelope = serde_json::from_slice(stdout).map_err(|_| {
        "Claude returned no valid result. Check authentication and model access, then try again."
    })?;
    if !success || result.kind != "result" || result.subtype != "success" || result.is_error {
        // Do not surface raw diagnostics: the CLI may echo repository context or credentials.
        return Err("Claude could not generate a message. Check authentication, model access, and organization policy, then try again.".into());
    }
    result
        .structured_output
        .map(|value| value.to_string())
        .ok_or_else(|| "Claude finished without a structured commit message. Try again.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_isolation_feature_is_required() {
        assert!(supports_isolation(&REQUIRED_FLAGS.join(" ")));
        for omitted in REQUIRED_FLAGS {
            let flags = REQUIRED_FLAGS
                .iter()
                .filter(|flag| flag != &omitted)
                .copied()
                .collect::<Vec<_>>();
            assert!(!supports_isolation(&flags.join(" ")));
        }
    }

    #[test]
    fn invocation_disables_tools_and_customizations_without_bypassing_policy() {
        let selection = TextGenerationSelection {
            provider: TextGenerationProvider::Claude,
            model: Some("test-model".into()),
            reasoning_effort: None,
        };
        let args = arguments(&selection)
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect::<Vec<_>>();
        assert!(args.windows(2).any(|pair| pair == ["--tools", ""]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--disallowedTools", "*"]));
        assert!(args.contains(&"--restricted".into()));
        assert!(args.contains(&"--safe-mode".into()));
        assert!(!args
            .iter()
            .any(|arg| arg.contains("skip-permissions") || arg == "--effort"));
    }

    #[test]
    fn versions_are_compared_numerically_and_unknown_formats_fail_closed() {
        assert_eq!(cli_version("2.1.259 (Claude Code)"), Some([2, 1, 259]));
        assert_eq!(cli_version("unknown"), None);
        assert!(compatible_models([2, 1, 1]).is_empty());
    }

    #[test]
    fn accepts_only_successful_structured_results() {
        assert!(parse_output(br#"{"type":"result","subtype":"success","is_error":false,"structured_output":{"subject":"test","body":""}}"#, true).is_ok());
        assert!(parse_output(br#"{"type":"result","subtype":"error_max_turns","is_error":false,"structured_output":{}}"#, true).is_err());
        assert!(parse_output(
            br#"{"type":"result","subtype":"success","is_error":false}"#,
            true
        )
        .is_err());
        assert!(parse_output(b"not json", false).is_err());
    }
}
