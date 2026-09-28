//! Maintained product policy. Codex availability always comes from model/list.
//! Edit model-manifest.json to update defaults and the normal picker. The bundle
//! ships the same policy to desktop and SSH agents; there is no remote config
//! download or account-availability inference for Claude. Validate the manifest
//! and verify provider documentation before publishing a catalog update.
use std::collections::HashSet;
use std::sync::OnceLock;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Manifest {
    version: u16,
    updated_at: String,
    pub codex: CodexPolicy,
    pub claude: ClaudePolicy,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CodexPolicy {
    pub default_model: String,
    pub recommended: Vec<String>,
    pub default_reasoning: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ClaudePolicy {
    pub default_model: String,
    pub models: Vec<ClaudeModel>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ClaudeModel {
    pub id: String,
    pub label: String,
    pub recommended: bool,
    pub minimum_cli_version: [u32; 3],
    pub reasoning: Vec<String>,
    pub default_reasoning: Option<String>,
}

fn parse(raw: &str) -> Result<Manifest, String> {
    let manifest: Manifest = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    if manifest.version != 1 || manifest.updated_at.len() != 10 {
        return Err("Unsupported model manifest version or date.".into());
    }
    let valid_id = |id: &str| {
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-._".contains(&byte))
    };
    let mut ids = HashSet::new();
    for id in &manifest.codex.recommended {
        if !valid_id(id) || !ids.insert(id) {
            return Err("Invalid or duplicate Codex model.".into());
        }
    }
    if !ids.contains(&manifest.codex.default_model) || !valid_id(&manifest.codex.default_reasoning)
    {
        return Err("Invalid Codex default.".into());
    }
    ids.clear();
    for model in &manifest.claude.models {
        if !valid_id(&model.id) || !ids.insert(&model.id) || model.label.trim().is_empty() {
            return Err("Invalid or duplicate Claude model.".into());
        }
        let mut efforts = HashSet::new();
        if model.reasoning.iter().any(|effort| {
            !["low", "medium", "high", "xhigh", "max"].contains(&effort.as_str())
                || !efforts.insert(effort)
        }) || model
            .default_reasoning
            .as_ref()
            .is_some_and(|effort| !efforts.contains(effort))
            || (model.reasoning.is_empty() != model.default_reasoning.is_none())
        {
            return Err("Invalid Claude reasoning options.".into());
        }
    }
    if !ids.contains(&manifest.claude.default_model) {
        return Err("Missing Claude default.".into());
    }
    Ok(manifest)
}

pub(super) fn manifest() -> &'static Manifest {
    static MANIFEST: OnceLock<Manifest> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        parse(include_str!("model-manifest.json")).expect("validated bundled model manifest")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_is_valid() {
        parse(include_str!("model-manifest.json")).unwrap();
    }

    #[test]
    fn rejects_unknown_versions_duplicate_ids_and_unsupported_options() {
        let source: serde_json::Value =
            serde_json::from_str(include_str!("model-manifest.json")).unwrap();
        for path in [
            "/version",
            "/claude/defaultModel",
            "/claude/models/0/defaultReasoning",
        ] {
            let mut value = source.clone();
            *value.pointer_mut(path).unwrap() = serde_json::json!("invalid");
            assert!(parse(&value.to_string()).is_err());
        }
        let mut value = source;
        let duplicate = value["claude"]["models"][0].clone();
        value["claude"]["models"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert!(parse(&value.to_string()).is_err());
    }
}
