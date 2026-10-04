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
    if request.kind == ActionKind::DeleteBranch {
        let repository = load_repository(&request.repository_path)?;
        let branch = request
            .branch
            .as_deref()
            .ok_or_else(|| ActionError::Blocked("no branch was named for deletion".to_string()))?;
        return delete_branch_plan(&repository, branch);
    }
    let (repository, seed, record) = load_record(&request)?;
    plan_for_record(request.kind, &repository, &seed, &record)
}

pub fn execute_action(request: ActionExecutionRequest) -> Result<ActionResult, ActionError> {
    let current_plan = prepare_action(ActionRequest {
        kind: request.kind,
        repository_path: request.repository_path.clone(),
        worktree_path: request.worktree_path.clone(),
        branch: request.expected_branch.clone(),
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
        ActionKind::DeleteBranch => {
            let branch = current_plan.branch.as_deref().ok_or_else(|| {
                ActionError::Blocked("no branch was named for deletion".to_string())
            })?;
            command::git_at(repository, ["branch", "-d", branch])
        }
    }
    .map_err(|error| ActionError::Git(error.to_string()))?;

    if !output.status.success() {
        return Err(ActionError::Git(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }

    let follow_up = match (request.kind, &current_plan.branch) {
        (ActionKind::Remove, Some(branch)) => Some(FollowUpAction {
            kind: ActionKind::DeleteBranch,
            repository_path: request.repository_path.clone(),
            branch: branch.clone(),
            description: format!(
                "Branch {branch} was retained and can now be reviewed for deletion."
            ),
        }),
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
            ActionKind::DeleteBranch => format!(
                "Branch {} deleted with git branch -d.",
                current_plan.branch.as_deref().unwrap_or("(unknown)")
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
        ActionKind::DeleteBranch => Err(ActionError::Blocked(
            "branch deletion is planned per branch, not per worktree".to_string(),
        )),
    }
}

fn delete_branch_plan(
    repository: &RepositoryContext,
    branch: &str,
) -> Result<ActionPlan, ActionError> {
    let branch_ref = format!("refs/heads/{branch}");
    let tip = command::successful_git_at(&repository.path, ["rev-parse", "--verify", &branch_ref])
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .map_err(|_| {
            ActionError::Blocked(format!(
                "branch {branch} no longer exists in this repository"
            ))
        })?;

    let seeds = list_worktrees(&repository.path).map_err(ActionError::Repository)?;
    if let Some(seed) = seeds
        .iter()
        .find(|seed| seed.branch.as_deref() == Some(branch))
    {
        return Err(ActionError::Blocked(format!(
            "branch {branch} is still checked out at {}",
            seed.path
        )));
    }

    let Some(target) = repository.default_target.as_deref() else {
        return Err(ActionError::Blocked(
            "the repository has no default remote target to verify the branch against".to_string(),
        ));
    };
    let contained = command::git_at(
        &repository.path,
        ["merge-base", "--is-ancestor", tip.as_str(), target],
    )
    .map_err(|error| ActionError::Git(error.to_string()))?;
    let display_target = target.strip_prefix("refs/remotes/").unwrap_or(target);
    if !contained.status.success() {
        return Err(ActionError::Blocked(format!(
            "the tip of {branch} is not contained in {display_target}. \
             A squash-merged branch can be integrated without containment; \
             verify its pull request before deleting the branch manually."
        )));
    }

    Ok(ActionPlan {
        kind: ActionKind::DeleteBranch,
        title: format!("Delete integrated branch {branch}?"),
        summary: format!(
            "Every commit on {branch} is contained in {display_target}. Git will delete only the local branch ref with branch -d, which refuses unmerged work."
        ),
        repository_path: repository.path.to_string_lossy().into_owned(),
        worktree_path: String::new(),
        branch: Some(branch.to_string()),
        expected_head: Some(tip),
        command_display: git_command_line(&repository.path, ["branch", "-d", branch]),
        affected_paths: vec![branch_ref],
        warnings: vec![
            "No worktree directories or remote branches are touched. This app never passes -D."
                .to_string(),
        ],
        confirmation_text: "DELETE".to_string(),
        destructive: true,
    })
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
