use std::path::Path;

use super::models::{WorktreeOrigin, WorktreeOriginKind};

struct AgentClassifier {
    id: &'static str,
    label: &'static str,
    path_markers: &'static [&'static str],
}

// Agent-specific knowledge lives here. The API and frontend consume the generic
// descriptor returned by `detect_worktree_origin` and do not know these IDs.
const AGENT_CLASSIFIERS: &[AgentClassifier] = &[
    AgentClassifier {
        id: "codex",
        label: "Codex",
        path_markers: &["/.codex/worktrees/"],
    },
    AgentClassifier {
        id: "claude",
        label: "Claude",
        path_markers: &["/.claude/worktrees/"],
    },
    AgentClassifier {
        id: "t3",
        label: "T3",
        path_markers: &["/.t3/worktrees/"],
    },
    AgentClassifier {
        id: "codex-monitor",
        label: "Codex Monitor",
        path_markers: &["/codexmonitor/", "/codex-monitor/"],
    },
];

pub(crate) fn detect_worktree_origin(path: &Path, is_primary: bool) -> WorktreeOrigin {
    if is_primary {
        return WorktreeOrigin {
            kind: WorktreeOriginKind::Primary,
            id: "primary".to_string(),
            label: "Primary".to_string(),
        };
    }

    let normalized = path.to_string_lossy().replace('\\', "/").to_lowercase();
    let classifier = AGENT_CLASSIFIERS.iter().find(|candidate| {
        candidate
            .path_markers
            .iter()
            .any(|marker| normalized.contains(marker))
    });

    match classifier {
        Some(agent) => WorktreeOrigin {
            kind: WorktreeOriginKind::Agent,
            id: agent.id.to_string(),
            label: agent.label.to_string(),
        },
        None => WorktreeOrigin {
            kind: WorktreeOriginKind::Unattributed,
            id: "linked".to_string(),
            label: "Linked".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn every_registered_agent_marker_is_detected() {
        for classifier in AGENT_CLASSIFIERS {
            for marker in classifier.path_markers {
                let path = Path::new("/tmp")
                    .join(marker.trim_matches('/'))
                    .join("repository");
                let origin = detect_worktree_origin(&path, false);
                assert_eq!(origin.kind, WorktreeOriginKind::Agent);
                assert_eq!(origin.id, classifier.id);
                assert_eq!(origin.label, classifier.label);
            }
        }
    }

    #[test]
    fn registered_agent_ids_and_markers_are_unique() {
        let mut ids = HashSet::new();
        let mut markers = HashSet::new();
        for classifier in AGENT_CLASSIFIERS {
            assert_ne!(classifier.id, "primary", "agent id is reserved");
            assert_ne!(classifier.id, "linked", "agent id is reserved");
            assert!(
                ids.insert(classifier.id),
                "duplicate agent id: {}",
                classifier.id
            );
            assert!(
                classifier
                    .id
                    .chars()
                    .all(|character| character.is_ascii_lowercase()
                        || character.is_ascii_digit()
                        || character == '-'),
                "agent id must be lowercase kebab-case: {}",
                classifier.id
            );
            assert!(
                !classifier.label.trim().is_empty(),
                "agent label cannot be empty"
            );
            assert!(
                !classifier.path_markers.is_empty(),
                "agent must have a path marker"
            );
            for marker in classifier.path_markers {
                assert!(
                    marker.starts_with('/') && marker.ends_with('/'),
                    "agent marker must match complete path segments: {marker}"
                );
                assert_eq!(
                    *marker,
                    marker.to_lowercase(),
                    "agent markers must be lowercase"
                );
                assert!(markers.insert(*marker), "duplicate agent marker: {marker}");
            }
        }
    }

    #[test]
    fn serialized_origin_contract_is_agent_agnostic() {
        let classifier = &AGENT_CLASSIFIERS[0];
        let path = Path::new("/tmp")
            .join(classifier.path_markers[0].trim_matches('/'))
            .join("repository");
        let value = serde_json::to_value(detect_worktree_origin(&path, false))
            .expect("serialize origin descriptor");

        assert_eq!(
            value,
            serde_json::json!({
                "kind": "agent",
                "id": classifier.id,
                "label": classifier.label,
            })
        );
    }

    #[test]
    fn primary_and_unattributed_origins_remain_generic() {
        let primary = detect_worktree_origin(Path::new("/source/repository"), true);
        assert_eq!(primary.kind, WorktreeOriginKind::Primary);
        assert_eq!(primary.id, "primary");

        let linked = detect_worktree_origin(Path::new("/source/worktrees/task"), false);
        assert_eq!(linked.kind, WorktreeOriginKind::Unattributed);
        assert_eq!(linked.id, "linked");
        assert_eq!(linked.label, "Linked");
    }
}
