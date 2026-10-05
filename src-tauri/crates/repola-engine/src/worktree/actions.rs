use std::path::{Path, PathBuf};

use thiserror::Error;

use super::command;
use super::command_display::git_command_line;
use super::discovery::{list_worktrees, repository_context};
use super::inspection::inspect_worktree;
use super::models::{
    ActionExecutionRequest, ActionKind, ActionPlan, ActionRequest, ActionResult, FollowUpAction,
    RegistrationKind, RepositoryContext, WorktreeRecord, WorktreeSeed,
};

#[derive(Debug, Error)]
pub enum ActionError {
    #[error("The repository could not be inspected: {0}")]
    Repository(String),
    #[error("The selected worktree is no longer registered in this repository.")]
    NotRegistered,
    #[error("The action is blocked: {0}")]
    Blocked(String),
    #[error("The worktree changed after review. Refresh and review it again.")]
    StaleReview,
    #[error("Git rejected the action: {0}")]
    Git(String),
}

pub fn prepare_action(request: ActionRequest) -> Result<ActionPlan, ActionError> {
    let (repository, seed, record) = load_record(&request)?;
    plan_for_record(request.kind, &repository, &seed, &record)
}

pub fn execute_action(request: ActionExecutionRequest) -> Result<ActionResult, ActionError> {
    let current_plan = prepare_action(ActionRequest {
        kind: request.kind,
        repository_path: request.repository_path.clone(),
        worktree_path: request.worktree_path.clone(),
    })?;
    if current_plan.expected_head != request.expected_head
        || current_plan.branch != request.expected_branch
        || current_plan.affected_paths != request.expected_affected_paths
    {
        return Err(ActionError::StaleReview);
    }

    let repository = Path::new(&request.repository_path);
    let output = match request.kind {
        ActionKind::Remove => command::git_at(
            repository,
            ["worktree", "remove", request.worktree_path.as_str()],
        ),
        ActionKind::Repair => command::git_at(
            repository,
            ["worktree", "repair", request.worktree_path.as_str()],
        ),
        ActionKind::Unlock => command::git_at(
            repository,
            ["worktree", "unlock", request.worktree_path.as_str()],
        ),
        ActionKind::PruneRepository => command::git_at(
            repository,
            ["worktree", "prune", "--expire=now", "--verbose"],
        ),
    }
    .map_err(|error| ActionError::Git(error.to_string()))?;

    if !output.status.success() {
        return Err(ActionError::Git(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }

    let follow_up = match (request.kind, &current_plan.branch) {
        (ActionKind::Remove, Some(branch)) => {
            review_worktree(repository).map(|worktree_path| FollowUpAction {
                repository_path: request.repository_path.clone(),
                worktree_path,
                branch: branch.clone(),
                description: format!(
                    "Branch {branch} was retained and can now be reviewed for deletion."
                ),
            })
        }
        _ => None,
    };

    Ok(ActionResult {
        message: match request.kind {
            ActionKind::Remove => "Worktree removed. Its Git branch was left intact.".to_string(),
            ActionKind::Repair => "Worktree registration repaired.".to_string(),
            ActionKind::Unlock => "Worktree unlocked.".to_string(),
            ActionKind::PruneRepository => format!(
                "Pruned {} stale registration{}.",
                current_plan.affected_paths.len(),
                if current_plan.affected_paths.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ),
        },
        audit_path: None,
        audit_warning: None,
        follow_up,
    })
}

fn load_repository(requested_path: &str) -> Result<RepositoryContext, ActionError> {
    let requested_repository = PathBuf::from(requested_path);
    let repository_path = dunce::canonicalize(&requested_repository)
        .map_err(|error| ActionError::Repository(error.to_string()))?;
    if repository_path != requested_repository {
        return Err(ActionError::Repository(
            "Use the repository's canonical path before managing worktrees.".to_string(),
        ));
    }
    repository_context(&repository_path).map_err(ActionError::Repository)
}

fn load_record(
    request: &ActionRequest,
) -> Result<(RepositoryContext, WorktreeSeed, WorktreeRecord), ActionError> {
    let repository = load_repository(&request.repository_path)?;
    let seeds = list_worktrees(&repository.path).map_err(ActionError::Repository)?;
    let seed = seeds
        .into_iter()
        .find(|seed| Path::new(&seed.path) == Path::new(&request.worktree_path))
        .ok_or(ActionError::NotRegistered)?;
    let record = inspect_worktree(&repository, &seed);
    Ok((repository, seed, record))
}

fn plan_for_record(
    kind: ActionKind,
    repository: &RepositoryContext,
    seed: &WorktreeSeed,
    record: &WorktreeRecord,
) -> Result<ActionPlan, ActionError> {
    match kind {
        ActionKind::Remove => remove_plan(repository, record),
        ActionKind::Repair => repair_plan(repository, record),
        ActionKind::Unlock => unlock_plan(repository, record),
        ActionKind::PruneRepository => prune_plan(repository, seed, record),
    }
}

/// The worktree a retained branch is reviewed from once its own worktree is
/// gone: the first remaining checkout Git can work in, which is the primary one
/// when it has one.
pub(super) fn review_worktree(repository: &Path) -> Option<String> {
    let common_dir = canonical_git_path(repository, "--git-common-dir")?;
    list_worktrees(repository)
        .ok()?
        .into_iter()
        .filter(|seed| seed.head.is_some() && seed.prunable_reason.is_none())
        // A locked worktree is never marked prunable, even once its directory
        // or its `.git` file is gone. Git run in what is left finds an
        // enclosing repository, or none.
        .find(|seed| {
            let Ok(path) = dunce::canonicalize(&seed.path) else {
                return false;
            };
            canonical_git_path(&path, "--show-toplevel").as_ref() == Some(&path)
                && canonical_git_path(&path, "--git-common-dir").as_ref() == Some(&common_dir)
        })
        .map(|seed| seed.path)
}

/// The path `git rev-parse --path-format=absolute <option>` prints in
/// `directory`, canonicalized.
fn canonical_git_path(directory: &Path, option: &str) -> Option<PathBuf> {
    let output =
        command::git_at(directory, ["rev-parse", "--path-format=absolute", option]).ok()?;
    if !output.status.success() {
        return None;
    }
    let printed = String::from_utf8(output.stdout).ok()?;
    dunce::canonicalize(printed.strip_suffix('\n')?).ok()
}

fn remove_plan(
    repository: &RepositoryContext,
    record: &WorktreeRecord,
) -> Result<ActionPlan, ActionError> {
    if record.is_primary {
        return Err(ActionError::Blocked(
            "primary worktrees cannot be removed here".to_string(),
        ));
    }
    if record.detached || record.branch.is_none() {
        return Err(ActionError::Blocked(
            "detached worktrees may contain commits without a durable branch".to_string(),
        ));
    }
    if record.registration.kind != RegistrationKind::Healthy {
        return Err(ActionError::Blocked(format!(
            "registration state is {:?}",
            record.registration.kind
        )));
    }
    if !record.status.available {
        return Err(ActionError::Blocked(
            "Git status could not be verified".to_string(),
        ));
    }
    if record.status.total > 0 {
        return Err(ActionError::Blocked(format!(
            "{} changed paths are present",
            record.status.total
        )));
    }

    let mut warnings =
        vec!["The directory will be deleted by Git. This app never passes --force.".to_string()];
    if let Some(count) = record.unpushed_commit_count.filter(|count| *count > 0) {
        warnings.push(format!(
            "{count} commit{} on this branch {} not on any remote. The retained branch preserves {}.",
            if count == 1 { "" } else { "s" },
            if count == 1 { "is" } else { "are" },
            if count == 1 { "it" } else { "them" },
        ));
    }

    Ok(ActionPlan {
        kind: ActionKind::Remove,
        title: "Remove clean worktree?".to_string(),
        summary: "Git will remove this worktree directory and registration. The branch will remain in the repository.".to_string(),
        repository_path: repository.path.to_string_lossy().into_owned(),
        worktree_path: record.path.clone(),
        branch: record.branch.clone(),
        expected_head: record.head.clone(),
        command_display: git_command_line(&repository.path, ["worktree", "remove", record.path.as_str()]),
        affected_paths: vec![record.path.clone()],
        warnings,
        confirmation_text: "REMOVE".to_string(),
        destructive: true,
    })
}

fn repair_plan(
    repository: &RepositoryContext,
    record: &WorktreeRecord,
) -> Result<ActionPlan, ActionError> {
    if record.registration.kind != RegistrationKind::BrokenLink || !record.exists {
        return Err(ActionError::Blocked(
            "only an existing worktree with a broken Git link can be repaired".to_string(),
        ));
    }
    Ok(ActionPlan {
        kind: ActionKind::Repair,
        title: "Repair worktree link?".to_string(),
        summary: "Git will update administrative pointers after the primary repository was moved."
            .to_string(),
        repository_path: repository.path.to_string_lossy().into_owned(),
        worktree_path: record.path.clone(),
        branch: record.branch.clone(),
        expected_head: record.head.clone(),
        command_display: git_command_line(
            &repository.path,
            ["worktree", "repair", record.path.as_str()],
        ),
        affected_paths: vec![record.path.clone()],
        warnings: vec!["No working-tree files will be deleted.".to_string()],
        confirmation_text: "REPAIR".to_string(),
        destructive: false,
    })
}

fn unlock_plan(
    repository: &RepositoryContext,
    record: &WorktreeRecord,
) -> Result<ActionPlan, ActionError> {
    if record.registration.kind != RegistrationKind::Locked {
        return Err(ActionError::Blocked(
            "only a locked worktree can be unlocked".to_string(),
        ));
    }
    Ok(ActionPlan {
        kind: ActionKind::Unlock,
        title: "Unlock worktree?".to_string(),
        summary: "Git will allow this registration to be moved, removed, or pruned again."
            .to_string(),
        repository_path: repository.path.to_string_lossy().into_owned(),
        worktree_path: record.path.clone(),
        branch: record.branch.clone(),
        expected_head: record.head.clone(),
        command_display: git_command_line(
            &repository.path,
            ["worktree", "unlock", record.path.as_str()],
        ),
        affected_paths: vec![record.path.clone()],
        warnings: vec!["Unlocking does not remove files or metadata.".to_string()],
        confirmation_text: "UNLOCK".to_string(),
        destructive: false,
    })
}

fn prune_plan(
    repository: &RepositoryContext,
    _seed: &WorktreeSeed,
    record: &WorktreeRecord,
) -> Result<ActionPlan, ActionError> {
    if record.registration.kind != RegistrationKind::Prunable {
        return Err(ActionError::Blocked(
            "Git has not marked this registration as prunable".to_string(),
        ));
    }
    let affected_paths = list_worktrees(&repository.path)
        .map_err(ActionError::Repository)?
        .into_iter()
        .filter(|candidate| candidate.prunable_reason.is_some())
        .map(|candidate| candidate.path)
        .collect::<Vec<_>>();
    if affected_paths.is_empty() {
        return Err(ActionError::Blocked(
            "no prunable registrations remain".to_string(),
        ));
    }
    Ok(ActionPlan {
        kind: ActionKind::PruneRepository,
        title: format!("Prune {} stale registration{}?", affected_paths.len(), if affected_paths.len() == 1 { "" } else { "s" }),
        summary: format!("Git will remove every expired worktree registration it currently considers prunable in {}. Existing directories are not deleted by worktree prune.", repository.name),
        repository_path: repository.path.to_string_lossy().into_owned(),
        worktree_path: record.path.clone(),
        branch: record.branch.clone(),
        expected_head: record.head.clone(),
        command_display: git_command_line(&repository.path, ["worktree", "prune", "--expire=now", "--verbose"]),
        affected_paths,
        warnings: vec!["This is repository-wide. Review every affected path below.".to_string()],
        confirmation_text: "PRUNE".to_string(),
        destructive: true,
    })
}
