//! User preferences the engine acts on: which external editor and terminal to
//! launch, and whether commits are signed by default.

use serde::{Deserialize, Serialize};

pub const APP_PREFERENCES_VERSION: u16 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppPreferences {
    pub version: u16,
    #[serde(alias = "editor")]
    pub editor_id: Option<String>,
    #[serde(alias = "terminal")]
    pub terminal_id: Option<String>,
    pub default_sign_commits: bool,
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            version: APP_PREFERENCES_VERSION,
            editor_id: None,
            terminal_id: None,
            default_sign_commits: false,
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
        APP_PREFERENCES_VERSION => {
            preferences.editor_id = normalize_tool_id(preferences.editor_id);
            preferences.terminal_id = normalize_tool_id(preferences.terminal_id);
            preferences
        }
        _ => AppPreferences::default(),
    }
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
    fn application_preferences_have_an_explicit_forward_safe_migration() {
        let legacy = migrate_app_preferences(AppPreferences {
            version: 0,
            editor_id: Some("cursor".into()),
            terminal_id: Some("warp".into()),
            default_sign_commits: true,
        });
        assert_eq!(legacy.version, 2);
        assert_eq!(legacy.editor_id.as_deref(), Some("cursor"));
        assert_eq!(legacy.terminal_id.as_deref(), Some("warp"));
        assert!(legacy.default_sign_commits);

        let future = migrate_app_preferences(AppPreferences {
            version: 99,
            editor_id: Some("zed".into()),
            terminal_id: Some("iterm2".into()),
            default_sign_commits: true,
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
}
