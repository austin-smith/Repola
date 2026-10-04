//! User preferences the engine acts on: which external editor and terminal to
//! launch, whether commits are signed by default, and how diffs are shown.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

pub const APP_PREFERENCES_VERSION: u16 = 5;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TextGenerationProvider {
    #[default]
    Codex,
    Claude,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextGenerationSelection {
    #[serde(default)]
    pub provider: TextGenerationProvider,
    /// None selects this provider's maintained default; explicit IDs never fall back.
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

/// The active provider and each provider's remembered model settings are separate.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "StoredGenerationPreferences")]
pub struct TextGenerationPreferences {
    pub provider: TextGenerationProvider,
    pub selections: BTreeMap<TextGenerationProvider, ModelSelection>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelection {
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredGenerationPreferences {
    Current {
        provider: TextGenerationProvider,
        selections: BTreeMap<TextGenerationProvider, ModelSelection>,
    },
    Legacy(TextGenerationSelection),
}

impl From<StoredGenerationPreferences> for TextGenerationPreferences {
    fn from(stored: StoredGenerationPreferences) -> Self {
        match stored {
            StoredGenerationPreferences::Current {
                provider,
                selections,
            } => Self {
                provider,
                selections,
            },
            StoredGenerationPreferences::Legacy(selection) => selection.into(),
        }
    }
}

impl From<TextGenerationSelection> for TextGenerationPreferences {
    fn from(selection: TextGenerationSelection) -> Self {
        Self {
            provider: selection.provider,
            selections: BTreeMap::from([(
                selection.provider,
                ModelSelection {
                    model: selection.model,
                    reasoning_effort: selection.reasoning_effort,
                },
            )]),
        }
    }
}

impl TextGenerationPreferences {
    pub fn selection(&self) -> TextGenerationSelection {
        let selected = self
            .selections
            .get(&self.provider)
            .cloned()
            .unwrap_or_default();
        TextGenerationSelection {
            provider: self.provider,
            model: selected.model,
            reasoning_effort: selected.reasoning_effort,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.selections.iter().all(|(provider, selected)| {
            valid_text_generation_selection(&TextGenerationSelection {
                provider: *provider,
                model: selected.model.clone(),
                reasoning_effort: selected.reasoning_effort.clone(),
            })
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppPreferences {
    pub version: u16,
    #[serde(alias = "editor")]
    pub editor_id: Option<String>,
    #[serde(alias = "terminal")]
    pub terminal_id: Option<String>,
    pub default_sign_commits: bool,
    #[serde(alias = "codexSelections")]
    pub text_generation_selections: BTreeMap<String, TextGenerationPreferences>,
    #[serde(deserialize_with = "lenient_diff_preferences")]
    pub diff: DiffPreferences,
}

/// How diffs are presented. Changes and History remember whitespace
/// visibility separately because hiding it disables line selection only
/// where lines can be selected.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DiffPreferences {
    pub hide_whitespace_in_changes: bool,
    pub hide_whitespace_in_history: bool,
}

/// Diff presentation is cosmetic, so a value this build cannot read (written
/// by a newer build, or damaged) resets only the diff preferences instead of
/// failing the whole preference record.
fn lenient_diff_preferences<'de, D>(deserializer: D) -> Result<DiffPreferences, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            version: APP_PREFERENCES_VERSION,
            editor_id: None,
            terminal_id: None,
            default_sign_commits: false,
            text_generation_selections: BTreeMap::new(),
            diff: DiffPreferences::default(),
        }
    }
}

/// Bring stored preferences forward to the current version. Unknown future
/// versions fall back to defaults rather than guessing at their meaning.
pub fn migrate_app_preferences(mut preferences: AppPreferences) -> AppPreferences {
    match preferences.version {
        0 | 1 => {
            preferences.version = APP_PREFERENCES_VERSION;
            preferences.editor_id = match preferences.editor_id.as_deref() {
                Some("visualStudioCode") => Some("vscode".into()),
                Some("cursor") => Some("cursor".into()),
                Some("zed") => Some("zed".into()),
                _ => None,
            };
            preferences.terminal_id = match preferences.terminal_id.as_deref() {
                Some("systemDefault") => Some(platform_default_terminal_id().into()),
                Some("iTerm2") => Some("iterm2".into()),
                Some("warp") => Some("warp".into()),
                _ => None,
            };
            preferences
        }
        2 | 3 | 4 | APP_PREFERENCES_VERSION => {
            preferences.version = APP_PREFERENCES_VERSION;
            preferences.editor_id = normalize_tool_id(preferences.editor_id);
            preferences.terminal_id = normalize_tool_id(preferences.terminal_id);
            preferences
                .text_generation_selections
                .retain(|machine_id, selection| {
                    valid_machine_id(machine_id) && selection.is_valid()
                });
            preferences
        }
        _ => AppPreferences::default(),
    }
}

pub fn valid_text_generation_selection(selection: &TextGenerationSelection) -> bool {
    selection
        .model
        .as_ref()
        .is_none_or(|model| valid_preference_value(model, 128))
        && (selection.model.is_some() || selection.reasoning_effort.is_none())
        && selection
            .reasoning_effort
            .as_ref()
            .is_none_or(|effort| valid_preference_value(effort, 32))
}

fn valid_machine_id(value: &str) -> bool {
    valid_preference_value(value, 128)
}

fn valid_preference_value(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value
            .chars()
            .all(|character| !character.is_control() && !character.is_whitespace())
}

#[cfg(target_os = "macos")]
pub fn platform_default_terminal_id() -> &'static str {
    "terminal"
}

#[cfg(target_os = "windows")]
pub fn platform_default_terminal_id() -> &'static str {
    "windows-terminal"
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn platform_default_terminal_id() -> &'static str {
    "default-terminal"
}

/// Tool IDs come from the backend registry and are lowercase kebab-case; anything
/// else is treated as "no preference".
pub fn normalize_tool_id(value: Option<String>) -> Option<String> {
    value.map(|value| value.trim().to_string()).filter(|value| {
        !value.is_empty()
            && value.len() <= 80
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_four_claude_choice_and_both_provider_choices_survive_migration() {
        let legacy: AppPreferences = serde_json::from_value(serde_json::json!({
            "version": 4, "textGenerationSelections": {
                "local": {"provider":"claude", "model":"saved-claude", "reasoningEffort":null}
            }
        }))
        .unwrap();
        let mut migrated = migrate_app_preferences(legacy);
        let preferences = migrated
            .text_generation_selections
            .get_mut("local")
            .unwrap();
        assert_eq!(
            preferences.selection().model.as_deref(),
            Some("saved-claude")
        );
        preferences.selections.insert(
            TextGenerationProvider::Codex,
            ModelSelection {
                model: Some("saved-codex".into()),
                reasoning_effort: Some("high".into()),
            },
        );
        preferences.provider = TextGenerationProvider::Codex;
        let round_trip: AppPreferences =
            serde_json::from_value(serde_json::to_value(&migrated).unwrap()).unwrap();
        assert_eq!(migrate_app_preferences(round_trip), migrated);
        assert_eq!(
            migrated.text_generation_selections["local"]
                .selections
                .len(),
            2
        );
    }

    #[test]
    fn version_three_codex_preferences_migrate_without_changing_the_model() {
        let legacy: AppPreferences = serde_json::from_value(serde_json::json!({
            "version": 3, "codexSelections": {
                "local": {"model": "saved-model", "reasoningEffort": "high"}
            }
        }))
        .unwrap();
        let migrated = migrate_app_preferences(legacy);
        let selection = migrated.text_generation_selections["local"].selection();
        assert_eq!(migrated.version, APP_PREFERENCES_VERSION);
        assert_eq!(selection.provider, TextGenerationProvider::Codex);
        assert_eq!(selection.model.as_deref(), Some("saved-model"));
        assert_eq!(selection.reasoning_effort.as_deref(), Some("high"));
        let serialized = serde_json::to_value(migrated).unwrap();
        assert!(serialized.get("codexSelections").is_none());
        assert!(serialized.get("textGenerationSelections").is_some());
    }

    #[test]
    fn provider_defaults_and_explicit_models_round_trip() {
        for provider in [
            TextGenerationProvider::Codex,
            TextGenerationProvider::Claude,
        ] {
            let selection = TextGenerationSelection {
                provider,
                model: None,
                reasoning_effort: None,
            };
            assert!(valid_text_generation_selection(&selection));
            let raw = serde_json::to_string(&selection).unwrap();
            assert_eq!(
                serde_json::from_str::<TextGenerationSelection>(&raw).unwrap(),
                selection
            );
        }
    }

    #[test]
    fn application_preferences_have_an_explicit_forward_safe_migration() {
        let legacy = migrate_app_preferences(AppPreferences {
            version: 0,
            editor_id: Some("cursor".into()),
            terminal_id: Some("warp".into()),
            default_sign_commits: true,
            text_generation_selections: BTreeMap::new(),
            diff: DiffPreferences::default(),
        });
        assert_eq!(legacy.version, APP_PREFERENCES_VERSION);
        assert_eq!(legacy.editor_id.as_deref(), Some("cursor"));
        assert_eq!(legacy.terminal_id.as_deref(), Some("warp"));
        assert!(legacy.default_sign_commits);

        let future = migrate_app_preferences(AppPreferences {
            version: 99,
            editor_id: Some("zed".into()),
            terminal_id: Some("iterm2".into()),
            default_sign_commits: true,
            text_generation_selections: BTreeMap::new(),
            diff: DiffPreferences::default(),
        });
        assert_eq!(future, AppPreferences::default());
    }

    #[test]
    fn legacy_backend_names_migrate_to_stable_tool_ids() {
        let legacy: AppPreferences = serde_json::from_value(serde_json::json!({
            "version": 1,
            "editor": "visualStudioCode",
            "terminal": "systemDefault",
            "defaultSignCommits": false
        }))
        .expect("legacy preferences");
        let migrated = migrate_app_preferences(legacy);
        assert_eq!(migrated.editor_id.as_deref(), Some("vscode"));
        assert_eq!(
            migrated.terminal_id.as_deref(),
            Some(platform_default_terminal_id())
        );
    }

    #[test]
    fn invalid_text_generation_selections_are_removed_during_migration() {
        let mut selections = BTreeMap::new();
        selections.insert(
            "local".into(),
            TextGenerationSelection {
                provider: TextGenerationProvider::Codex,
                model: Some("gpt-5.6-luna".into()),
                reasoning_effort: Some("low".into()),
            }
            .into(),
        );
        selections.insert(
            "bad machine".into(),
            TextGenerationSelection {
                provider: TextGenerationProvider::Codex,
                model: Some("gpt-5.6-luna".into()),
                reasoning_effort: Some("low".into()),
            }
            .into(),
        );
        let migrated = migrate_app_preferences(AppPreferences {
            version: 3,
            text_generation_selections: selections,
            ..AppPreferences::default()
        });
        assert_eq!(migrated.text_generation_selections.len(), 1);
        assert!(migrated.text_generation_selections.contains_key("local"));
    }

    #[test]
    fn diff_preferences_load_in_both_directions_without_resetting_other_choices() {
        let current = serde_json::json!({
            "version": APP_PREFERENCES_VERSION,
            "editorId": "zed",
            "defaultSignCommits": true,
        });

        // Written before diff preferences existed.
        let older: AppPreferences =
            serde_json::from_value(current.clone()).expect("preferences without diff");
        let older = migrate_app_preferences(older);
        assert_eq!(older.diff, DiffPreferences::default());
        assert_eq!(older.editor_id.as_deref(), Some("zed"));
        assert!(older.default_sign_commits);

        // Written by a newer build that added keys this build does not know.
        let mut newer = current.clone();
        newer["futurePreference"] = serde_json::json!({ "enabled": true });
        newer["diff"] = serde_json::json!({
            "hideWhitespaceInChanges": true,
            "futureDiffChoice": "wrapped",
        });
        let newer = migrate_app_preferences(
            serde_json::from_value(newer).expect("preferences with future keys"),
        );
        assert!(newer.diff.hide_whitespace_in_changes);
        assert!(!newer.diff.hide_whitespace_in_history);
        assert_eq!(newer.editor_id.as_deref(), Some("zed"));
        assert!(newer.default_sign_commits);

        // A diff value this build cannot read resets only the diff choices.
        let mut unreadable = current;
        unreadable["diff"] = serde_json::json!({ "hideWhitespaceInHistory": "sometimes" });
        let unreadable = migrate_app_preferences(
            serde_json::from_value(unreadable).expect("preferences with unreadable diff"),
        );
        assert_eq!(unreadable.diff, DiffPreferences::default());
        assert_eq!(unreadable.editor_id.as_deref(), Some("zed"));
        assert!(unreadable.default_sign_commits);

        let round_trip: AppPreferences = serde_json::from_value(
            serde_json::to_value(AppPreferences {
                diff: DiffPreferences {
                    hide_whitespace_in_changes: false,
                    hide_whitespace_in_history: true,
                },
                ..AppPreferences::default()
            })
            .expect("serialize preferences"),
        )
        .expect("deserialize preferences");
        assert!(round_trip.diff.hide_whitespace_in_history);
        assert!(!round_trip.diff.hide_whitespace_in_changes);
    }
}
