use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::operation;
use rayon::prelude::*;
use thiserror::Error;

use super::command;
use super::identity;
use super::inspection::{apply_measurement, inspect_worktree_shallow, measure_worktree};
use super::models::{
    RegistrationKind, RemoteProvider, RepositoryContext, RepositorySummary, ScanEvent, ScanRequest,
    ScanResult, ScanTotals, WorktreeRecord, WorktreeSeed,
};

#[derive(Debug, Error)]
pub enum ScanError {
    #[error("The worktree scan was cancelled.")]
    Cancelled,
}

pub fn scan(request: ScanRequest) -> Result<ScanResult, ScanError> {
    scan_streaming(request, |_event| {})
}

pub fn scan_streaming<F>(request: ScanRequest, emit: F) -> Result<ScanResult, ScanError>
where
    F: Fn(ScanEvent) + Sync,
{
    if operation::is_cancelled() {
        return Err(ScanError::Cancelled);
    }
    let repository_paths = request.repository_paths.clone();
    let mut warnings = Vec::new();
    let mut repositories = Vec::new();
    let mut work_items = Vec::new();
    let mut seen_git_dirs = BTreeSet::new();

    for path in &repository_paths {
        if operation::is_cancelled() {
            return Err(ScanError::Cancelled);
        }
        let inspected = inspect_registered_repository(path);
        match inspected {
            Ok((context, seeds)) => {
                let git_dir = dunce::canonicalize(&context.git_dir)
                    .unwrap_or_else(|_| context.git_dir.clone());
                let repository_id = identity::repository_id(&git_dir);
                if !seen_git_dirs.insert(git_dir) {
                    continue;
                }
                let summary = RepositorySummary {
                    id: repository_id,
                    name: context.name.clone(),
                    path: context.path.to_string_lossy().into_owned(),
                    remote_url: context.remote_url.clone(),
                    provider: context.provider,
                    worktree_count: seeds.len(),
                    attention_count: seeds
                        .iter()
                        .filter(|seed| {
                            seed.locked_reason.is_some()
                                || seed.prunable_reason.is_some()
                                || !Path::new(&seed.path).exists()
                        })
                        .count(),
                    conflicted_count: 0,
                    allocated_bytes: 0,
                    allocation_incomplete: false,
                };
                repositories.push(summary);
                work_items.extend(seeds.into_iter().map(|seed| (context.clone(), seed)));
            }
            Err(error) => warnings.push(format!("{path}: {error}")),
        }
    }
    repositories.sort_by_key(|repository| repository.name.to_lowercase());
    emit(ScanEvent::Repositories {
        repository_paths: repository_paths.clone(),
        repositories: repositories.clone(),
        warnings: warnings.clone(),
    });

    let operation_token = operation::current_operation();
    let mut worktrees: Vec<WorktreeRecord> = work_items
        .par_iter()
        .filter_map(|(repository, seed)| {
            if operation_token.is_cancelled() {
                return None;
            }
            let record = operation::with_operation(operation_token.clone(), || {
                inspect_worktree_shallow(repository, seed)
            });
            emit(ScanEvent::Worktree {
                record: Box::new(record.clone()),
            });
            Some(record)
        })
        .collect();
    if operation_token.is_cancelled() {
        return Err(ScanError::Cancelled);
    }
    worktrees.par_iter_mut().for_each(|record| {
        if operation_token.is_cancelled() {
            return;
        }
        if let Some(measurement) =
            operation::with_operation(operation_token.clone(), || measure_worktree(record))
        {
            apply_measurement(record, &measurement);
            emit(ScanEvent::Size {
                id: record.id.clone(),
                size_bytes: record.size_bytes,
                size_incomplete: record.size_incomplete,
                last_activity_at_ms: record.last_activity_at_ms,
            });
        }
    });
    if operation_token.is_cancelled() {
        return Err(ScanError::Cancelled);
    }
    worktrees.sort_by(|left, right| {
        left.repository_name
            .to_lowercase()
            .cmp(&right.repository_name.to_lowercase())
            .then_with(|| right.is_primary.cmp(&left.is_primary))
            .then_with(|| right.created_at_ms.cmp(&left.created_at_ms))
            .then_with(|| left.path.cmp(&right.path))
    });

    for repository in &mut repositories {
        let records: Vec<&WorktreeRecord> = worktrees
            .iter()
            .filter(|worktree| worktree.repository_path == repository.path)
            .collect();
        repository.worktree_count = records.len();
        repository.attention_count = records
            .iter()
            .filter(|worktree| {
                !worktree.exists
                    || matches!(
                        worktree.registration.kind,
                        RegistrationKind::BrokenLink
                            | RegistrationKind::Locked
                            | RegistrationKind::Missing
                            | RegistrationKind::Prunable
                    )
            })
            .count();
        repository.conflicted_count = records
            .iter()
            .filter(|worktree| worktree.status.conflicted > 0)
            .count();
        repository.allocated_bytes = records
            .iter()
            .filter_map(|worktree| worktree.size_bytes)
            .fold(0_u64, u64::saturating_add);
        repository.allocation_incomplete = records
            .iter()
            .any(|worktree| worktree.size_incomplete || worktree.size_bytes.is_none());
    }

    let totals = calculate_totals(repositories.len(), &worktrees);
    let result = ScanResult {
        scanned_at_ms: now_ms(),
        repository_paths,
        repositories,
        worktrees,
        totals,
        warnings,
    };
    emit(ScanEvent::Done {
        result: Box::new(result.clone()),
    });
    Ok(result)
}

fn inspect_registered_repository(
    path: &str,
) -> Result<(RepositoryContext, Vec<WorktreeSeed>), String> {
    validate_repository_path(path)?;
    let requested = dunce::canonicalize(path)
        .map_err(|error| format!("The registered repository is unavailable: {error}"))?;
    if !requested.is_dir() {
        return Err("The registered repository is not a folder.".into());
    }
    let candidate = repository_context(&requested)?;
    let seeds = list_worktrees(&candidate.path)?;
    let primary = seeds
        .first()
        .ok_or_else(|| "Git reported no working copy for this repository.".to_string())?;
    let context = if Path::new(&primary.path) == candidate.path {
        candidate
    } else {
        repository_context(Path::new(&primary.path))?
    };
    Ok((context, seeds))
}

pub fn resolve_repository(path: &str) -> Result<String, String> {
    let (context, _) = inspect_registered_repository(path)?;
    dunce::canonicalize(&context.path)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|error| format!("Repola could not resolve the repository working copy: {error}"))
}

fn validate_repository_path(path: &str) -> Result<(), String> {
    if path.trim() != path
        || path.is_empty()
        || path.len() > 32 * 1024
        || path.chars().any(char::is_control)
    {
        return Err("The repository path is empty, malformed, or too long.".into());
    }
    Ok(())
}

pub(crate) fn repository_context(path: &Path) -> Result<RepositoryContext, String> {
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let git_dir_output = command::successful_git_at(
        path,
        ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .map_err(|error| error.to_string())?;
    let git_dir = PathBuf::from(String::from_utf8_lossy(&git_dir_output.stdout).trim());
    let remote_url = command::git_at(path, ["config", "--get", "remote.origin.url"])
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty());
    let provider = remote_provider(remote_url.as_deref());
    let default_target = command::git_at(path, ["symbolic-ref", "-q", "refs/remotes/origin/HEAD"])
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty());

    Ok(RepositoryContext {
        name,
        path: path.to_path_buf(),
        git_dir,
        remote_url,
        provider,
        default_target,
    })
}

pub(crate) fn list_worktrees(repository: &Path) -> Result<Vec<WorktreeSeed>, String> {
    let output = command::successful_git_at(repository, ["worktree", "list", "--porcelain", "-z"])
        .map_err(|error| error.to_string())?;
    Ok(parse_worktree_porcelain(&output.stdout))
}

pub(crate) fn parse_worktree_porcelain(bytes: &[u8]) -> Vec<WorktreeSeed> {
    let mut records = Vec::new();
    let mut current: Option<WorktreeSeed> = None;
    let mut first = true;

    for field in bytes.split(|byte| *byte == 0) {
        if field.is_empty() {
            if let Some(record) = current.take() {
                records.push(record);
            }
            continue;
        }
        let text = String::from_utf8_lossy(field);
        if let Some(path) = text.strip_prefix("worktree ") {
            if let Some(record) = current.take() {
                records.push(record);
            }
            current = Some(WorktreeSeed {
                path: path.to_string(),
                head: None,
                branch: None,
                detached: false,
                is_primary: first,
                locked_reason: None,
                prunable_reason: None,
            });
            first = false;
        } else if let Some(record) = current.as_mut() {
            if let Some(head) = text.strip_prefix("HEAD ") {
                record.head = Some(head.to_string());
            } else if let Some(branch) = text.strip_prefix("branch ") {
                record.branch = Some(
                    branch
                        .strip_prefix("refs/heads/")
                        .unwrap_or(branch)
                        .to_string(),
                );
            } else if text == "detached" {
                record.detached = true;
            } else if text == "locked" {
                record.locked_reason = Some("Locked without a recorded reason.".to_string());
            } else if let Some(reason) = text.strip_prefix("locked ") {
                record.locked_reason = Some(reason.to_string());
            } else if text == "prunable" {
                record.prunable_reason =
                    Some("Git marked this registration as prunable.".to_string());
            } else if let Some(reason) = text.strip_prefix("prunable ") {
                record.prunable_reason = Some(reason.to_string());
            }
        }
    }
    if let Some(record) = current {
        records.push(record);
    }
    records
}

pub(crate) fn remote_provider(remote_url: Option<&str>) -> RemoteProvider {
    let Some(remote_url) = remote_url else {
        return RemoteProvider::None;
    };
    let normalized = remote_url.to_ascii_lowercase();
    if normalized.contains("github") {
        RemoteProvider::GitHub
    } else if normalized.contains("dev.azure.com") || normalized.contains("visualstudio.com") {
        RemoteProvider::AzureDevOps
    } else {
        RemoteProvider::Other
    }
}

fn calculate_totals(repository_count: usize, worktrees: &[WorktreeRecord]) -> ScanTotals {
    let mut totals = ScanTotals {
        repository_count,
        ..ScanTotals::default()
    };
    for worktree in worktrees {
        if worktree.is_primary {
            totals.primary_count += 1;
            continue;
        }
        totals.linked_count += 1;
        if worktree.exists {
            totals.existing_linked_count += 1;
        } else {
            totals.missing_count += 1;
        }
        if worktree.status.available {
            if worktree.status.total == 0 {
                totals.clean_count += 1;
            } else {
                totals.dirty_count += 1;
            }
        }
        if worktree.registration.kind == RegistrationKind::Prunable {
            totals.prunable_count += 1;
        }
        if worktree.registration.kind == RegistrationKind::BrokenLink {
            totals.broken_link_count += 1;
        }
        totals.linked_size_bytes = totals
            .linked_size_bytes
            .saturating_add(worktree.size_bytes.unwrap_or(0));
    }
    totals
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}
