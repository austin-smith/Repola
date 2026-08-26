use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::command;
use super::discovery::{list_worktrees, repository_context};
use super::models::{
    BranchInfo, BranchMutationKind, BranchMutationRequest, BranchMutationResult, BranchRequest,
    WorkingCopyRequest,
};
use super::working_copy::working_copy_snapshot;

pub fn branches(request: BranchRequest) -> Result<Vec<BranchInfo>, String> {
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path.clone(),
        worktree_path: request.worktree_path,
    })?;
    let repository = repository_context(Path::new(&snapshot.repository_path))?;
    let occupied: BTreeMap<String, String> = list_worktrees(&repository.path)?
        .into_iter()
        .filter_map(|worktree| worktree.branch.map(|branch| (branch, worktree.path)))
        .collect();
    let output = command::successful_git_at(
        &repository.path,
        [
            "for-each-ref",
            "--format=%(refname)%1f%(objectname)%1f%(upstream:short)%1f%(upstream:track,nobracket)%1f%(HEAD)",
            "refs/heads",
            "refs/remotes",
        ],
    )
    .map_err(|error| error.to_string())?;
    parse_branches(&output.stdout, &occupied)
}

pub fn mutate_branch(request: BranchMutationRequest) -> Result<BranchMutationResult, String> {
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path.clone(),
        worktree_path: request.worktree_path.clone(),
    })?;
    if snapshot.head != request.expected_head {
        return Err("The working-copy HEAD changed after the branch action was reviewed.".into());
    }
    if snapshot.operation.is_some() {
        return Err(
            "Finish or abort the in-progress Git operation before changing branches.".into(),
        );
    }
    if request.kind != BranchMutationKind::Rename
        && snapshot.changes.iter().any(|change| !change.ignored)
    {
        return Err("Commit or stash local changes before changing branches.".into());
    }
    validate_branch_name(&request.branch)?;
    let worktree = PathBuf::from(&snapshot.worktree_path);
    let output = match request.kind {
        BranchMutationKind::Create => {
            if let Some(start_point) = request.start_point.as_deref() {
                command::git_at(
                    &worktree,
                    ["switch", "--create", request.branch.as_str(), start_point],
                )
            } else {
                command::git_at(&worktree, ["switch", "--create", request.branch.as_str()])
            }
        }
        BranchMutationKind::Checkout => {
            let available = branches(BranchRequest {
                repository_path: request.repository_path.clone(),
                worktree_path: request.worktree_path.clone(),
            })?;
            let selected = available
                .iter()
                .find(|branch| !branch.remote && branch.name == request.branch)
                .or_else(|| {
                    available
                        .iter()
                        .find(|branch| branch.remote && branch.name == request.branch)
                })
                .ok_or_else(|| format!("Branch {} no longer exists.", request.branch))?;
            if let Some(path) = selected
                .occupied_worktree_path
                .as_deref()
                .filter(|path| *path != snapshot.worktree_path)
            {
                return Err(format!(
                    "Branch {} is already checked out at {path}.",
                    request.branch
                ));
            }
            if selected.remote {
                command::git_at(&worktree, ["switch", "--track", selected.name.as_str()])
            } else {
                command::git_at(&worktree, ["switch", selected.name.as_str()])
            }
        }
        BranchMutationKind::Rename => {
            let current = snapshot
                .branch
                .as_deref()
                .ok_or_else(|| "A detached HEAD cannot be renamed.".to_string())?;
            command::git_at(
                &worktree,
                ["branch", "--move", current, request.branch.as_str()],
            )
        }
    }
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: snapshot.repository_path.clone(),
        worktree_path: snapshot.worktree_path.clone(),
    })?;
    let branches = branches(BranchRequest {
        repository_path: snapshot.repository_path.clone(),
        worktree_path: snapshot.worktree_path.clone(),
    })?;
    Ok(BranchMutationResult { branches, snapshot })
}

pub(super) fn validate_branch_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 1024 || name.contains('\0') {
        return Err("The branch name is invalid.".into());
    }
    let output = command::output("git", ["check-ref-format", "--branch", name])
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn parse_branches(
    bytes: &[u8],
    occupied: &BTreeMap<String, String>,
) -> Result<Vec<BranchInfo>, String> {
    let mut branches = Vec::new();
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let fields: Vec<&[u8]> = line.split(|byte| *byte == 0x1f).collect();
        if fields.len() != 5 {
            return Err("Git returned malformed branch metadata.".into());
        }
        let full_name = String::from_utf8_lossy(fields[0]).into_owned();
        if full_name.ends_with("/HEAD") {
            continue;
        }
        let remote = full_name.starts_with("refs/remotes/");
        let name = full_name
            .strip_prefix(if remote {
                "refs/remotes/"
            } else {
                "refs/heads/"
            })
            .unwrap_or(&full_name)
            .to_string();
        let (ahead, behind) = parse_tracking(&String::from_utf8_lossy(fields[3]));
        branches.push(BranchInfo {
            occupied_worktree_path: (!remote).then(|| occupied.get(&name).cloned()).flatten(),
            name,
            full_name,
            head: String::from_utf8_lossy(fields[1]).into_owned(),
            remote,
            current: fields[4] == b"*",
            upstream: (!fields[2].is_empty())
                .then(|| String::from_utf8_lossy(fields[2]).into_owned()),
            ahead,
            behind,
        });
    }
    branches.sort_by(|left, right| {
        left.remote
            .cmp(&right.remote)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(branches)
}

fn parse_tracking(value: &str) -> (u64, u64) {
    let mut ahead = 0;
    let mut behind = 0;
    for part in value.split(',').map(str::trim) {
        if let Some(value) = part.strip_prefix("ahead ") {
            ahead = value.parse().unwrap_or(0);
        } else if let Some(value) = part.strip_prefix("behind ") {
            behind = value.parse().unwrap_or(0);
        }
    }
    (ahead, behind)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn initialize_repository() -> tempfile::TempDir {
        let repository = tempfile::tempdir().expect("repository");
        command::successful_git_at(repository.path(), ["init"]).expect("init");
        command::successful_git_at(repository.path(), ["config", "user.name", "Branch Test"])
            .expect("name");
        command::successful_git_at(
            repository.path(),
            ["config", "user.email", "branch@example.invalid"],
        )
        .expect("email");
        std::fs::write(repository.path().join("file.txt"), "initial").expect("file");
        command::successful_git_at(repository.path(), ["add", "--", "file.txt"]).expect("stage");
        command::successful_git_at(repository.path(), ["commit", "-m", "initial"]).expect("commit");
        repository
    }

    #[test]
    fn parses_tracking_and_worktree_occupancy() {
        let input = b"refs/heads/feature\x1fabc\x1forigin/feature\x1fahead 2, behind 3\x1f*\nrefs/remotes/origin/feature\x1fabc\x1f\x1f\x1f \nrefs/remotes/origin/HEAD\x1fabc\x1f\x1f\x1f \n";
        let occupied = BTreeMap::from([("feature".into(), "/work/feature".into())]);
        let branches = parse_branches(input, &occupied).expect("branches");
        assert_eq!(branches.len(), 2);
        assert_eq!((branches[0].ahead, branches[0].behind), (2, 3));
        assert_eq!(
            branches[0].occupied_worktree_path.as_deref(),
            Some("/work/feature")
        );
        assert!(branches[1].remote);
    }

    #[test]
    fn creates_renames_and_protects_branches_occupied_by_another_worktree() {
        let repository = initialize_repository();
        let path = repository.path().to_string_lossy().into_owned();
        let initial = working_copy_snapshot(WorkingCopyRequest {
            repository_path: path.clone(),
            worktree_path: path.clone(),
        })
        .expect("snapshot");
        let created = mutate_branch(BranchMutationRequest {
            repository_path: path.clone(),
            worktree_path: path.clone(),
            kind: BranchMutationKind::Create,
            branch: "feature/one".into(),
            start_point: None,
            expected_head: initial.head,
        })
        .expect("create branch");
        assert_eq!(created.snapshot.branch.as_deref(), Some("feature/one"));

        let renamed = mutate_branch(BranchMutationRequest {
            repository_path: path.clone(),
            worktree_path: path.clone(),
            kind: BranchMutationKind::Rename,
            branch: "feature/renamed".into(),
            start_point: None,
            expected_head: created.snapshot.head,
        })
        .expect("rename branch");
        assert_eq!(renamed.snapshot.branch.as_deref(), Some("feature/renamed"));

        let main = renamed
            .branches
            .iter()
            .find(|branch| !branch.remote && !branch.current)
            .expect("original branch")
            .name
            .clone();
        let second = tempfile::tempdir().expect("worktree parent");
        let second_path = second.path().join("occupied");
        command::successful_git_at(
            repository.path(),
            ["worktree", "add", &second_path.to_string_lossy(), &main],
        )
        .expect("add worktree");
        let error = mutate_branch(BranchMutationRequest {
            repository_path: path.clone(),
            worktree_path: path,
            kind: BranchMutationKind::Checkout,
            branch: main,
            start_point: None,
            expected_head: renamed.snapshot.head,
        })
        .expect_err("occupied branch must be protected");
        assert!(error.contains("already checked out"));
    }
}
