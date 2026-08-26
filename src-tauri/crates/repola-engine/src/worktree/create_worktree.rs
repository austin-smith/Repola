use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::branches::validate_branch_name;
use super::command;
use super::discovery::{list_worktrees, repository_context};
use super::models::{CreateWorktreeRequest, CreateWorktreeResult, WorkingCopyRequest};
use super::working_copy::working_copy_snapshot;

pub fn create_worktree(request: CreateWorktreeRequest) -> Result<CreateWorktreeResult, String> {
    let source = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path.clone(),
        worktree_path: request.source_worktree_path,
    })?;
    if source.head != request.expected_head {
        return Err("The source working-copy HEAD changed after this worktree was reviewed. Refresh and try again.".into());
    }
    validate_branch_name(&request.branch)?;
    let repository = repository_context(Path::new(&source.repository_path))?;
    let destination = validate_destination(&request.destination_path, &repository.path)?;
    let worktrees = list_worktrees(&repository.path)?;
    if worktrees.iter().any(|worktree| {
        worktree.branch.as_deref() == Some(request.branch.as_str())
            && Path::new(&worktree.path) != destination
    }) {
        return Err(format!(
            "Branch {} is already checked out in another worktree.",
            request.branch
        ));
    }

    let mut args = vec![OsString::from("worktree"), OsString::from("add")];
    if request.create_branch {
        ensure_branch_missing(&repository.path, &request.branch)?;
        args.push(OsString::from("-b"));
        args.push(OsString::from(&request.branch));
    } else {
        ensure_local_branch_exists(&repository.path, &request.branch)?;
    }
    args.push(OsString::from("--"));
    args.push(destination.as_os_str().to_owned());
    if request.create_branch {
        if let Some(start_point) = request.start_point.as_deref() {
            validate_start_point(&repository.path, start_point)?;
            args.push(OsString::from(start_point));
        }
    } else {
        args.push(OsString::from(&request.branch));
    }

    let output = command::git_at(&repository.path, args).map_err(|error| error.to_string())?;
    let diagnostic = combined_output(&output.stdout, &output.stderr);
    if !output.status.success() {
        return Err(if diagnostic.is_empty() {
            "Git could not create the linked worktree and returned no diagnostic output.".into()
        } else {
            diagnostic
        });
    }
    let canonical = dunce::canonicalize(&destination).map_err(|error| {
        format!(
            "Git created the worktree, but Repola could not resolve {}: {error}",
            destination.display()
        )
    })?;
    let created = list_worktrees(&repository.path)?
        .into_iter()
        .find(|worktree| Path::new(&worktree.path) == canonical)
        .ok_or_else(|| "Git completed without registering the linked worktree.".to_string())?;
    if created.branch.as_deref() != Some(request.branch.as_str()) {
        return Err("Git registered the worktree on an unexpected branch.".into());
    }
    Ok(CreateWorktreeResult {
        repository_path: repository.path.to_string_lossy().into_owned(),
        worktree_path: canonical.to_string_lossy().into_owned(),
        branch: request.branch,
        output: diagnostic,
    })
}

fn validate_destination(value: &str, repository: &Path) -> Result<PathBuf, String> {
    if value.is_empty() || value.len() > 32 * 1024 || value.contains('\0') {
        return Err("The worktree destination is empty, malformed, or too long.".into());
    }
    let destination = PathBuf::from(value);
    if !destination.is_absolute() {
        return Err("The worktree destination must be a full path on the selected machine.".into());
    }
    if destination.exists() {
        return Err("The worktree destination already exists. Choose a new folder.".into());
    }
    let parent = destination.parent().ok_or_else(|| {
        "The worktree destination must have an existing parent folder.".to_string()
    })?;
    let canonical_parent = dunce::canonicalize(parent)
        .map_err(|error| format!("Could not resolve the worktree destination parent: {error}"))?;
    let name = destination
        .file_name()
        .ok_or_else(|| "The worktree destination must name a folder.".to_string())?;
    let normalized = canonical_parent.join(name);
    for existing in list_worktrees(repository)? {
        let existing = Path::new(&existing.path);
        if normalized.starts_with(existing) {
            return Err(format!(
                "The new worktree cannot be nested inside the existing worktree at {}.",
                existing.display()
            ));
        }
    }
    Ok(normalized)
}

fn ensure_branch_missing(repository: &Path, branch: &str) -> Result<(), String> {
    let reference = format!("refs/heads/{branch}");
    let output = command::git_at(repository, ["show-ref", "--verify", "--quiet", &reference])
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Err(format!(
            "Branch {branch} already exists. Choose Existing branch instead."
        ))
    } else {
        Ok(())
    }
}

fn ensure_local_branch_exists(repository: &Path, branch: &str) -> Result<(), String> {
    let reference = format!("refs/heads/{branch}");
    let output = command::git_at(repository, ["show-ref", "--verify", "--quiet", &reference])
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("Local branch {branch} does not exist."))
    }
}

fn validate_start_point(repository: &Path, start_point: &str) -> Result<(), String> {
    if start_point.is_empty()
        || start_point.len() > 4096
        || start_point.contains(['\0', '\n', '\r'])
    {
        return Err("The branch start point is invalid.".into());
    }
    let revision = format!("{start_point}^{{commit}}");
    let output = command::git_at(
        repository,
        [
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &revision,
        ],
    )
    .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Start point {start_point} does not resolve to a commit."
        ))
    }
}

fn combined_output(stdout: &[u8], stderr: &[u8]) -> String {
    [stdout, stderr]
        .into_iter()
        .map(|bytes| String::from_utf8_lossy(bytes).trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn creates_new_and_existing_branch_worktrees_and_rejects_occupancy() {
        let parent = tempfile::tempdir().expect("parent");
        let repository = parent.path().join("repository");
        command::successful_git_at(parent.path(), ["init", repository.to_str().unwrap()])
            .expect("initialize");
        command::successful_git_at(&repository, ["config", "user.name", "Repola Test"])
            .expect("name");
        command::successful_git_at(
            &repository,
            ["config", "user.email", "repola@example.invalid"],
        )
        .expect("email");
        fs::write(repository.join("README.md"), "base\n").expect("readme");
        command::successful_git_at(&repository, ["add", "README.md"]).expect("add");
        command::successful_git_at(&repository, ["commit", "-m", "base"]).expect("commit");
        command::successful_git_at(&repository, ["branch", "existing"]).expect("branch");
        let head = String::from_utf8_lossy(
            &command::successful_git_at(&repository, ["rev-parse", "HEAD"])
                .expect("head")
                .stdout,
        )
        .trim()
        .to_string();

        let new_path = parent.path().join("new-worktree");
        let created = create_worktree(CreateWorktreeRequest {
            repository_path: repository.to_string_lossy().into_owned(),
            source_worktree_path: repository.to_string_lossy().into_owned(),
            destination_path: new_path.to_string_lossy().into_owned(),
            branch: "new-branch".into(),
            create_branch: true,
            start_point: Some("HEAD".into()),
            expected_head: Some(head.clone()),
        })
        .expect("new branch worktree");
        assert_eq!(created.branch, "new-branch");

        let existing_path = parent.path().join("existing-worktree");
        create_worktree(CreateWorktreeRequest {
            repository_path: repository.to_string_lossy().into_owned(),
            source_worktree_path: repository.to_string_lossy().into_owned(),
            destination_path: existing_path.to_string_lossy().into_owned(),
            branch: "existing".into(),
            create_branch: false,
            start_point: None,
            expected_head: Some(head.clone()),
        })
        .expect("existing branch worktree");

        let occupied = create_worktree(CreateWorktreeRequest {
            repository_path: repository.to_string_lossy().into_owned(),
            source_worktree_path: repository.to_string_lossy().into_owned(),
            destination_path: parent
                .path()
                .join("duplicate")
                .to_string_lossy()
                .into_owned(),
            branch: "existing".into(),
            create_branch: false,
            start_point: None,
            expected_head: Some(head),
        });
        assert!(occupied.is_err());
    }
}
