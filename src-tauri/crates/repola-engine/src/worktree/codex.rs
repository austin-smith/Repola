use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use serde::Deserialize;
use serde_json::json;

use super::command::{self, CommandError};
use super::models::{
    ReasoningEffort, TextGenerationModel, TextGenerationStatus, TextGenerationStatusKind,
};
use super::text_generation::{truncate_with_marker, OUTPUT_SCHEMA};
use crate::operation;
use crate::preferences::{TextGenerationProvider, TextGenerationSelection};

const MAX_DIAGNOSTIC_CHARS: usize = 4_000;
const CODEX_TIMEOUT: Duration = Duration::from_secs(3 * 60);
const CODEX_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(20);
// Status probes version, help, authentication, and the model catalog in sequence.
pub(super) const STATUS_BUDGET: Duration = CODEX_DISCOVERY_TIMEOUT.saturating_mul(4);
// Generation checks help and the model catalog before starting the model.
pub(super) const GENERATION_BUDGET: Duration = CODEX_DISCOVERY_TIMEOUT
    .saturating_mul(2)
    .saturating_add(CODEX_TIMEOUT);
const MAX_APP_SERVER_LINE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelListResult {
    data: Vec<AppServerModel>,
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppServerModel {
    model: String,
    display_name: String,
    description: String,
    is_default: bool,
    upgrade: Option<String>,
    default_reasoning_effort: String,
    supported_reasoning_efforts: Vec<AppServerReasoningEffort>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppServerReasoningEffort {
    reasoning_effort: String,
    description: String,
}

pub(super) fn status() -> TextGenerationStatus {
    let Ok(temporary) = tempfile::tempdir() else {
        return provider_status(TextGenerationStatusKind::Unavailable, None, Vec::new());
    };
    let probe = |args: &[&str]| {
        command::output_at_with_input_timeout(
            temporary.path(),
            "codex",
            args,
            b"",
            CODEX_DISCOVERY_TIMEOUT,
        )
    };
    let version_output = match probe(&["--version"]) {
        Ok(output) if output.status.success() => output,
        Err(CommandError::Launch { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            return provider_status(TextGenerationStatusKind::NotInstalled, None, Vec::new());
        }
        Ok(_) | Err(_) => {
            return provider_status(TextGenerationStatusKind::Unavailable, None, Vec::new())
        }
    };
    let version = String::from_utf8_lossy(&version_output.stdout)
        .trim()
        .to_string();
    let version = (!version.is_empty()).then_some(version);

    let help_output = match probe(&["exec", "--help"]) {
        Ok(output) if output.status.success() => output,
        Ok(_) | Err(_) => {
            return provider_status(TextGenerationStatusKind::Unavailable, version, Vec::new())
        }
    };
    if !supports_isolated_generation(&String::from_utf8_lossy(&help_output.stdout)) {
        return provider_status(
            TextGenerationStatusKind::UpdateRequired,
            version,
            Vec::new(),
        );
    }

    match probe(&["login", "status"]) {
        Ok(output) if output.status.success() => match load_codex_models() {
            Ok(models) => provider_status(TextGenerationStatusKind::Ready, version, models),
            Err(detail) => TextGenerationStatus {
                detail: Some(detail),
                ..provider_status(TextGenerationStatusKind::Unavailable, version, Vec::new())
            },
        },
        Ok(output) if authentication_missing(&output.stdout, &output.stderr) => {
            provider_status(TextGenerationStatusKind::SignedOut, version, Vec::new())
        }
        Ok(_) | Err(_) => {
            provider_status(TextGenerationStatusKind::Unavailable, version, Vec::new())
        }
    }
}

fn provider_status(
    status: TextGenerationStatusKind,
    version: Option<String>,
    models: Vec<TextGenerationModel>,
) -> TextGenerationStatus {
    let recommended_selection = recommended_selection(&models);
    TextGenerationStatus {
        status,
        detail: None,
        version,
        models,
        recommended_selection,
    }
}

pub(super) fn generate(
    prompt: &str,
    requested: Option<&TextGenerationSelection>,
) -> Result<String, String> {
    let temporary = tempfile::tempdir()
        .map_err(|error| format!("Could not create generation workspace: {error}"))?;
    let schema_path = temporary.path().join("commit-message-schema.json");
    let output_path = temporary.path().join("commit-message.json");
    fs::write(&schema_path, OUTPUT_SCHEMA).map_err(|error| error.to_string())?;
    verify_codex_cli(temporary.path())?;
    let models = load_codex_models()?;
    let selection = resolve_selection(requested, &models)?;
    let output = command::output_at_with_input_timeout(
        temporary.path(),
        "codex",
        codex_arguments(&schema_path, &output_path, &selection),
        prompt.as_bytes(),
        CODEX_TIMEOUT,
    )
    .map_err(codex_launch_error)?;
    if !output.status.success() {
        return Err(codex_failure(
            &output.stdout,
            &output.stderr,
            output.status.code(),
        ));
    }
    let file = fs::File::open(&output_path)
        .map_err(|error| format!("Codex finished without a readable commit message: {error}"))?;
    let mut raw = String::new();
    file.take(128 * 1024 + 1)
        .read_to_string(&mut raw)
        .map_err(|error| error.to_string())?;
    if raw.len() > 128 * 1024 {
        return Err("Codex returned an oversized commit message.".into());
    }
    Ok(raw)
}

fn codex_arguments(
    schema_path: &std::path::Path,
    output_path: &std::path::Path,
    selection: &TextGenerationSelection,
) -> Vec<OsString> {
    let mut arguments = vec![
        OsString::from("exec"),
        OsString::from("--ephemeral"),
        OsString::from("--skip-git-repo-check"),
        OsString::from("--ignore-user-config"),
        OsString::from("--ignore-rules"),
        OsString::from("--strict-config"),
        OsString::from("--color"),
        OsString::from("never"),
        OsString::from("--model"),
        OsString::from(selection.model.as_deref().expect("resolved model")),
    ];
    arguments.extend(isolation_arguments());
    if let Some(effort) = &selection.reasoning_effort {
        arguments.extend([
            OsString::from("--config"),
            OsString::from(format!("model_reasoning_effort={}", json!(effort))),
        ]);
    }
    arguments.extend([
        OsString::from("--output-schema"),
        schema_path.as_os_str().to_owned(),
        OsString::from("--output-last-message"),
        output_path.as_os_str().to_owned(),
        OsString::from("-"),
    ]);
    arguments
}

/// Deny host-file access at the sandbox boundary as well as removing tools.
/// Strict config parsing makes older CLIs fail closed instead of ignoring policy.
fn isolation_arguments() -> Vec<OsString> {
    [
        "approval_policy=\"never\"",
        "default_permissions=\"repola\"",
        "permissions.repola.filesystem={\":root\"=\"deny\",\":minimal\"=\"read\",\":workspace_roots\"=\"read\",\":tmpdir\"=\"deny\",\":slash_tmp\"=\"deny\"}",
        "permissions.repola.network.enabled=false",
        "features.shell_tool=false",
        "features.view_image=false",
        "features.multi_agent=false",
        "features.apps=false",
        "features.plugins=false",
        "features.hooks=false",
        "features.computer_use=false",
        "features.browser_use=false",
        "features.shell_snapshot=false",
        "skills.include_instructions=false",
        "project_doc_max_bytes=0",
        "web_search=\"disabled\"",
    ]
    .into_iter()
    .flat_map(|value| [OsString::from("--config"), OsString::from(value)])
    .collect()
}

// app-server has no --ignore-user-config flag. Verify its effective catalog
// source before accepting any model; never advertise custom-provider/catalog
// entries to exec, which deliberately ignores user configuration.
fn validate_discovery_config(response: &serde_json::Value) -> Result<(), String> {
    let config = response
        .get("result")
        .and_then(|result| result.get("config"))
        .and_then(serde_json::Value::as_object)
        .ok_or("Codex did not return its effective model configuration.")?;
    let provider = config.get("model_provider");
    if provider.is_some_and(|value| !value.is_null() && value.as_str() != Some("openai"))
        || config
            .get("model_catalog_json")
            .is_some_and(|value| !value.is_null())
    {
        return Err("Repola's isolated Codex integration does not support a custom model_provider or model_catalog_json in Codex configuration.".into());
    }
    Ok(())
}

fn load_codex_models() -> Result<Vec<TextGenerationModel>, String> {
    let result = request_model_catalog()?;
    let models = result
        .data
        .into_iter()
        .map(|model| {
            let recommended_for_commit_messages = is_recommended_commit_model(&model.model);
            TextGenerationModel {
                model: model.model,
                display_name: model.display_name,
                description: model.description,
                is_default: model.is_default,
                recommended_for_commit_messages,
                upgrade: model.upgrade,
                default_reasoning_effort: Some(model.default_reasoning_effort),
                supported_reasoning_efforts: model
                    .supported_reasoning_efforts
                    .into_iter()
                    .map(|effort| ReasoningEffort {
                        reasoning_effort: effort.reasoning_effort,
                        description: effort.description,
                    })
                    .collect(),
            }
        })
        .collect::<Vec<_>>();
    if models.is_empty() {
        return Err("Codex returned an empty model catalog.".into());
    }
    Ok(models)
}

fn is_recommended_commit_model(model: &str) -> bool {
    crate::worktree::model_catalog::manifest()
        .codex
        .recommended
        .iter()
        .any(|id| id == model)
}

fn request_model_catalog() -> Result<ModelListResult, String> {
    let deadline = Instant::now() + CODEX_DISCOVERY_TIMEOUT;
    let temporary = tempfile::tempdir().map_err(|error| error.to_string())?;
    let mut child = command::spawn_piped_at(temporary.path(), "codex", ["app-server", "--stdio"])
        .map_err(codex_launch_error)?;
    let mut stdin = child
        .stdin()
        .take()
        .ok_or_else(|| "Could not open Codex app-server input.".to_string())?;
    let stdout = child
        .stdout()
        .take()
        .ok_or_else(|| "Could not read Codex app-server output.".to_string())?;
    let stderr = child
        .stderr()
        .take()
        .ok_or_else(|| "Could not read Codex app-server diagnostics.".to_string())?;
    let (sender, receiver) = mpsc::sync_channel(32);
    let stdout_reader = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            match reader
                .by_ref()
                .take(MAX_APP_SERVER_LINE_BYTES as u64 + 1)
                .read_line(&mut line)
            {
                Ok(0) => break,
                Ok(_) if line.len() > MAX_APP_SERVER_LINE_BYTES => {
                    let _ = sender.send(Err("Codex returned an oversized response.".to_string()));
                    break;
                }
                Ok(_) => match serde_json::from_str(line.trim()) {
                    Ok(message) => {
                        if sender.send(Ok(message)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Err(format!(
                            "Codex returned an unreadable app-server response: {error}"
                        )));
                        break;
                    }
                },
                Err(error) => {
                    let _ = sender.send(Err(format!("Could not read Codex app-server: {error}")));
                    break;
                }
            }
        }
    });
    let stderr_reader = thread::spawn(move || {
        // Drain diagnostics without retaining repository data or credentials.
        let _ = std::io::copy(&mut { stderr }, &mut std::io::sink());
    });

    let result =
        (|| {
            write_app_server_message(
                &mut stdin,
                &json!({
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "clientInfo": {
                            "name": "repola",
                            "title": "Repola",
                            "version": env!("CARGO_PKG_VERSION")
                        },
                        "capabilities": {}
                    }
                }),
            )?;
            response_with_id(&receiver, 1, deadline)?;
            write_app_server_message(
                &mut stdin,
                &json!({ "method": "initialized", "params": {} }),
            )?;
            write_app_server_message(
                &mut stdin,
                &json!({
                    "id": 2, "method": "config/read", "params": {"includeLayers": false}
                }),
            )?;
            validate_discovery_config(&response_with_id(&receiver, 2, deadline)?)?;
            let mut models = Vec::new();
            let mut cursor: Option<String> = None;
            for page_index in 0_u64..20 {
                let request_id = 3 + page_index;
                write_app_server_message(
                    &mut stdin,
                    &json!({
                        "id": request_id,
                        "method": "model/list",
                        "params": {
                            "cursor": cursor.as_deref(),
                            "limit": 100,
                            "includeHidden": false
                        }
                    }),
                )?;
                let response = response_with_id(&receiver, request_id, deadline)?;
                let mut page: ModelListResult =
                    serde_json::from_value(response.get("result").cloned().ok_or_else(|| {
                        "Codex returned an incomplete model catalog.".to_string()
                    })?)
                    .map_err(|_| "Codex returned an unreadable model catalog.".to_string())?;
                models.append(&mut page.data);
                let Some(next_cursor) = page.next_cursor else {
                    return Ok(ModelListResult {
                        data: models,
                        next_cursor: None,
                    });
                };
                cursor = Some(next_cursor);
            }
            Err("Codex returned too many model-catalog pages.".into())
        })();

    drop(stdin);
    drop(receiver);
    let _ = child.kill();
    let _ = child.wait();
    let _ = stdout_reader.join();
    let _ = stderr_reader.join();
    result
}

fn write_app_server_message(
    stdin: &mut impl Write,
    message: &serde_json::Value,
) -> Result<(), String> {
    writeln!(stdin, "{message}")
        .and_then(|_| stdin.flush())
        .map_err(|error| format!("Could not write to Codex app-server: {error}"))
}

fn response_with_id(
    receiver: &mpsc::Receiver<Result<serde_json::Value, String>>,
    expected_id: u64,
    deadline: Instant,
) -> Result<serde_json::Value, String> {
    loop {
        if operation::current_operation().is_cancelled() {
            return Err("Codex model discovery was cancelled.".into());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("Codex model discovery timed out.".into());
        }
        match receiver.recv_timeout(remaining.min(Duration::from_millis(50))) {
            Ok(Ok(message))
                if message.get("id").and_then(serde_json::Value::as_u64) == Some(expected_id) =>
            {
                if let Some(error) = message.get("error") {
                    return Err(format!(
                        "Codex model discovery failed (code {}).",
                        error
                            .get("code")
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or_default()
                    ));
                }
                return Ok(message);
            }
            Ok(Ok(_)) => {}
            Ok(Err(error)) => return Err(error),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("Codex app-server stopped before returning its model catalog.".into())
            }
        }
    }
}

fn recommended_selection(models: &[TextGenerationModel]) -> Option<TextGenerationSelection> {
    let model = models
        .iter()
        .find(|model| {
            model.model
                == crate::worktree::model_catalog::manifest()
                    .codex
                    .default_model
        })
        .or_else(|| {
            crate::worktree::model_catalog::manifest()
                .codex
                .recommended
                .iter()
                .find_map(|id| models.iter().find(|model| &model.model == id))
        })?;
    let reasoning_effort = model
        .supported_reasoning_efforts
        .iter()
        .find(|effort| {
            effort.reasoning_effort
                == crate::worktree::model_catalog::manifest()
                    .codex
                    .default_reasoning
        })
        .map(|effort| effort.reasoning_effort.clone())
        .or_else(|| {
            model
                .supported_reasoning_efforts
                .iter()
                .find(|effort| {
                    Some(&effort.reasoning_effort) == model.default_reasoning_effort.as_ref()
                })
                .map(|effort| effort.reasoning_effort.clone())
        })
        .or_else(|| {
            model
                .supported_reasoning_efforts
                .first()
                .map(|effort| effort.reasoning_effort.clone())
        });
    Some(TextGenerationSelection {
        provider: TextGenerationProvider::Codex,
        model: Some(model.model.clone()),
        reasoning_effort,
    })
}

fn resolve_selection(
    requested: Option<&TextGenerationSelection>,
    models: &[TextGenerationModel],
) -> Result<TextGenerationSelection, String> {
    let selection = requested
        .filter(|selection| selection.model.is_some())
        .cloned()
        .or_else(|| recommended_selection(models))
        .ok_or_else(|| "Codex has no model available for commit-message generation.".to_string())?;
    let model = models
        .iter()
        .find(|model| Some(&model.model) == selection.model.as_ref())
        .ok_or_else(|| {
            format!(
                "The selected Codex model {} is no longer available. Choose another model in Settings.",
                selection.model.as_deref().unwrap_or("default")
            )
        })?;
    let valid_effort = match &selection.reasoning_effort {
        Some(effort) => model
            .supported_reasoning_efforts
            .iter()
            .any(|option| &option.reasoning_effort == effort),
        None => model.supported_reasoning_efforts.is_empty(),
    };
    if selection.provider != TextGenerationProvider::Codex || !valid_effort {
        return Err(format!(
            "{} does not support {} reasoning. Choose another reasoning level in Settings.",
            model.display_name,
            selection.reasoning_effort.as_deref().unwrap_or("default")
        ));
    }
    Ok(selection)
}

fn verify_codex_cli(directory: &std::path::Path) -> Result<(), String> {
    let output = command::output_at_with_input_timeout(
        directory,
        "codex",
        ["exec", "--help"],
        b"",
        CODEX_DISCOVERY_TIMEOUT,
    )
    .map_err(codex_launch_error)?;
    if !output.status.success() {
        return Err(codex_failure(
            &output.stdout,
            &output.stderr,
            output.status.code(),
        ));
    }
    let help = String::from_utf8_lossy(&output.stdout);
    if !supports_isolated_generation(&help) {
        return Err(
            "This Codex CLI is too old for Repola's isolated commit-message generation. Update Codex, then try again."
                .into(),
        );
    }
    Ok(())
}

fn supports_isolated_generation(help: &str) -> bool {
    [
        "--ephemeral",
        "--ignore-user-config",
        "--ignore-rules",
        "--output-schema",
        "--output-last-message",
        "--model",
        "--config",
        "--strict-config",
    ]
    .iter()
    .all(|flag| help.contains(flag))
}

fn authentication_missing(stdout: &[u8], stderr: &[u8]) -> bool {
    let diagnostic = format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    )
    .to_lowercase();
    diagnostic.contains("not logged in")
        || diagnostic.contains("not authenticated")
        || diagnostic.contains("authentication required")
}

fn codex_launch_error(error: CommandError) -> String {
    match error {
        CommandError::Launch { source, .. }
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            "Codex is not installed or is not available on PATH on this machine. Install the Codex CLI, then run `codex login`.".into()
        }
        other => format!("Could not start Codex: {other}"),
    }
}

fn codex_failure(stdout: &[u8], stderr: &[u8], status: Option<i32>) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let stdout = String::from_utf8_lossy(stdout);
    let diagnostic = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    let lower = diagnostic.to_lowercase();
    if lower.contains("not logged in")
        || lower.contains("not authenticated")
        || lower.contains("authentication required")
        || lower.contains("401 unauthorized")
    {
        return "Codex is not signed in on this machine. Run `codex login`, then try again.".into();
    }
    let detail = truncate_with_marker(diagnostic, MAX_DIAGNOSTIC_CHARS, "diagnostic");
    if detail.is_empty() {
        format!(
            "Codex could not generate a commit message{}.",
            status
                .map(|code| format!(" (exit code {code})"))
                .unwrap_or_default()
        )
    } else {
        format!("Codex could not generate a commit message: {detail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_is_ephemeral_isolated_read_only_and_schema_constrained() {
        let arguments = codex_arguments(
            std::path::Path::new("schema.json"),
            std::path::Path::new("output.json"),
            &TextGenerationSelection {
                provider: TextGenerationProvider::Codex,
                model: Some("gpt-5.6-luna".into()),
                reasoning_effort: Some("low".into()),
            },
        );
        let arguments = arguments
            .iter()
            .map(|argument| argument.to_string_lossy())
            .collect::<Vec<_>>();
        for flag in [
            "--ephemeral",
            "--ignore-user-config",
            "--ignore-rules",
            "--strict-config",
        ] {
            assert!(arguments.iter().any(|arg| arg == flag));
        }
        for pair in [
            ["--config", "features.shell_tool=false"],
            ["--config", "features.view_image=false"],
            ["--config", "features.apps=false"],
            ["--config", "features.plugins=false"],
            ["--config", "default_permissions=\"repola\""],
            ["--config", "permissions.repola.filesystem={\":root\"=\"deny\",\":minimal\"=\"read\",\":workspace_roots\"=\"read\",\":tmpdir\"=\"deny\",\":slash_tmp\"=\"deny\"}"],
            ["--config", "permissions.repola.network.enabled=false"],
            ["--config", "approval_policy=\"never\""],
            ["--model", "gpt-5.6-luna"],
            ["--config", "model_reasoning_effort=\"low\""],
            ["--output-schema", "schema.json"],
            ["--output-last-message", "output.json"],
        ] {
            assert!(arguments.windows(2).any(|args| args == pair), "missing {pair:?}");
        }
        assert!(!arguments.iter().any(|arg| arg == "--sandbox"));
    }

    #[test]
    fn status_requires_every_isolation_feature_and_recognizes_signed_out_cli() {
        let help =
            "--ephemeral --ignore-user-config --ignore-rules --output-schema --output-last-message --model --config --strict-config";
        assert!(supports_isolated_generation(help));
        assert!(!supports_isolated_generation("--ephemeral --output-schema"));
        assert!(authentication_missing(b"Not logged in", b""));
    }

    #[test]
    fn commit_generation_prefers_the_fast_model_and_low_reasoning() {
        let models = vec![
            model("gpt-5.6-sol", true, "medium", &["low", "medium"]),
            model("gpt-5.6-luna", false, "medium", &["low", "medium"]),
        ];
        assert_eq!(
            recommended_selection(&models),
            Some(TextGenerationSelection {
                provider: TextGenerationProvider::Codex,
                model: Some("gpt-5.6-luna".into()),
                reasoning_effort: Some("low".into()),
            })
        );
    }

    #[test]
    fn fallback_is_maintained_and_never_a_legacy_cli_default() {
        let legacy = model("legacy", true, "low", &["low"]);
        let approved = model("gpt-5.6-sol", false, "low", &["low"]);
        assert_eq!(
            recommended_selection(&[legacy.clone(), approved])
                .unwrap()
                .model
                .as_deref(),
            Some("gpt-5.6-sol")
        );
        assert!(recommended_selection(std::slice::from_ref(&legacy)).is_none());
        assert!(resolve_selection(None, &[legacy]).is_err());
    }

    #[test]
    fn discovery_rejects_catalog_sources_ignored_by_generation() {
        assert!(validate_discovery_config(
            &json!({"result":{"config":{"model_provider":"openai","model_catalog_json":null}}})
        )
        .is_ok());
        for config in [
            json!({"model_provider":"custom"}),
            json!({"model_catalog_json":"custom.json"}),
        ] {
            assert!(validate_discovery_config(&json!({"result":{"config":config}})).is_err());
        }
        assert!(validate_discovery_config(&json!({"result":{}})).is_err());
    }

    #[test]
    fn commit_model_choices_exclude_old_and_specialized_catalog_entries() {
        for model in &crate::worktree::model_catalog::manifest().codex.recommended {
            assert!(is_recommended_commit_model(model));
        }
        assert!(!is_recommended_commit_model("gpt-5.5"));
        assert!(!is_recommended_commit_model("gpt-5.4"));
        assert!(!is_recommended_commit_model("gpt-daybreak-blue-latest"));
    }

    #[test]
    fn selected_model_and_reasoning_must_still_exist_in_live_catalog() {
        let models = vec![model("gpt-5.6-luna", false, "medium", &["low", "medium"])];
        assert!(resolve_selection(
            Some(&TextGenerationSelection {
                provider: TextGenerationProvider::Codex,
                model: Some("retired-model".into()),
                reasoning_effort: Some("low".into()),
            }),
            &models
        )
        .is_err());
        assert!(resolve_selection(
            Some(&TextGenerationSelection {
                provider: TextGenerationProvider::Codex,
                model: Some("gpt-5.6-luna".into()),
                reasoning_effort: Some("ultra".into()),
            }),
            &models
        )
        .is_err());
    }

    #[test]
    #[ignore = "requires an installed, signed-in Codex CLI"]
    fn live_codex_catalog_can_be_discovered_over_app_server() {
        let status = status();
        assert_eq!(status.status, TextGenerationStatusKind::Ready);
        assert!(!status.models.is_empty());
        assert!(status.recommended_selection.is_some());
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires the installed Codex sandbox and permission to invoke Seatbelt"]
    fn live_codex_sandbox_denies_reads_outside_generation_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let allowed = workspace.path().join("allowed.txt");
        let denied = outside.path().join("private.txt");
        fs::write(&allowed, "allowed fixture").unwrap();
        fs::write(&denied, "private fixture").unwrap();
        let read = |path: &std::path::Path| {
            let mut args = vec![
                OsString::from("sandbox"),
                OsString::from("-P"),
                OsString::from("repola"),
            ];
            args.extend(isolation_arguments());
            args.extend([
                OsString::from("--"),
                OsString::from("/bin/cat"),
                path.as_os_str().to_owned(),
            ]);
            command::output_at_with_input_timeout(
                workspace.path(),
                "codex",
                args,
                b"",
                CODEX_DISCOVERY_TIMEOUT,
            )
            .unwrap()
        };
        let allowed_output = read(&allowed);
        assert!(
            allowed_output.status.success(),
            "{}",
            String::from_utf8_lossy(&allowed_output.stderr)
        );
        assert_eq!(allowed_output.stdout, b"allowed fixture");
        let denied_output = read(&denied);
        assert!(!denied_output.status.success());
        assert!(denied_output.stdout.is_empty());
    }

    #[test]
    #[ignore = "sends a synthetic prompt using the installed, signed-in Codex CLI"]
    fn live_codex_generation_returns_structured_output() {
        let raw = generate(
            "Return a JSON commit message for this synthetic change: correct a spelling error in README. Use subject and body string fields. Do not use tools.",
            None,
        ).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(value["subject"]
            .as_str()
            .is_some_and(|subject| !subject.is_empty()));
        assert!(value["body"].is_string());
    }

    fn model(
        name: &str,
        is_default: bool,
        default_reasoning_effort: &str,
        supported_reasoning_efforts: &[&str],
    ) -> TextGenerationModel {
        TextGenerationModel {
            model: name.into(),
            display_name: name.into(),
            description: String::new(),
            is_default,
            recommended_for_commit_messages: is_recommended_commit_model(name),
            upgrade: None,
            default_reasoning_effort: Some(default_reasoning_effort.into()),
            supported_reasoning_efforts: supported_reasoning_efforts
                .iter()
                .map(|effort| ReasoningEffort {
                    reasoning_effort: (*effort).into(),
                    description: String::new(),
                })
                .collect(),
        }
    }
}
