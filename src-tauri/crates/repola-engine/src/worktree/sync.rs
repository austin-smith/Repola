use std::ffi::OsString;
use std::path::Path;

use super::command;
use super::models::{SyncKind, SyncRequest, SyncResult, WorkingCopyRequest};
use super::working_copy::working_copy_snapshot;

pub fn synchronize(request: SyncRequest) -> Result<SyncResult, String> {
    let before = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path.clone(),
        worktree_path: request.worktree_path.clone(),
    })?;
    if request.kind != SyncKind::Fetch && before.head != request.expected_head {
        return Err("The working-copy HEAD changed after this operation was reviewed. Refresh and try again.".into());
    }
    if request.kind == SyncKind::ForcePush && before.upstream_head != request.expected_upstream_head
    {
        return Err("The reviewed remote branch changed. Fetch and review its new state before force-pushing.".into());
    }
    if request.kind == SyncKind::Pull {
        if before.operation.is_some() {
            return Err("Finish or abort the in-progress Git operation before pulling.".into());
        }
        if before.changes.iter().any(|change| !change.ignored) {
            return Err("Commit or stash local changes before pulling.".into());
        }
        if before.upstream.is_none() {
            return Err("Publish this branch or configure an upstream before pulling.".into());
        }
    }
    let remote = before.remote.as_deref().ok_or_else(|| {
        "This repository has no configured upstream remote or origin.".to_string()
    })?;
    let args: Vec<OsString> = match request.kind {
        SyncKind::Fetch => vec!["fetch".into(), "--progress".into(), remote.into()],
        SyncKind::Pull => vec!["pull".into(), "--progress".into(), "--no-edit".into()],
        SyncKind::Push => {
            if before.upstream.is_none() {
                return Err("Publish this branch before pushing.".into());
            }
            vec!["push".into(), "--progress".into()]
        }
        SyncKind::Publish => {
            let branch = before
                .branch
                .as_deref()
                .ok_or_else(|| "Create or check out a branch before publishing.".to_string())?;
            if before.upstream.is_some() {
                return Err("This branch is already published; use Push instead.".into());
            }
            vec![
                "push".into(),
                "--progress".into(),
                "--set-upstream".into(),
                remote.into(),
                branch.into(),
            ]
        }
        SyncKind::ForcePush => {
            let branch = before
                .branch
                .as_deref()
                .ok_or_else(|| "Check out a branch before force-pushing.".to_string())?;
            let expected = before.upstream_head.as_deref().ok_or_else(|| {
                "Fetch the published branch before force-pushing so Repola can establish an exact lease.".to_string()
            })?;
            if before.upstream.is_none() {
                return Err("Publish this branch before force-pushing.".into());
            }
            let lease = format!("--force-with-lease=refs/heads/{branch}:{expected}");
            vec![
                "push".into(),
                "--progress".into(),
                lease.into(),
                remote.into(),
                branch.into(),
            ]
        }
    };
    let output = command::git_at(Path::new(&before.worktree_path), args)
        .map_err(|error| error.to_string())?;
    let diagnostic = combined_output(&output.stdout, &output.stderr);
    if !output.status.success() {
        return Err(if diagnostic.is_empty() {
            "Git synchronization failed without diagnostic output.".into()
        } else {
            diagnostic
        });
    }
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: before.repository_path,
        worktree_path: before.worktree_path,
    })?;
    Ok(SyncResult {
        snapshot,
        output: diagnostic,
    })
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

    #[test]
    fn publishes_to_a_real_bare_remote_and_sets_upstream() {
        let repository = tempfile::tempdir().expect("repository");
        let remote = tempfile::tempdir().expect("remote");
        command::successful_git_at(repository.path(), ["init"]).expect("init repository");
        command::successful_git_at(remote.path(), ["init", "--bare"]).expect("init remote");
        command::successful_git_at(repository.path(), ["config", "user.name", "Sync Test"])
            .expect("name");
        command::successful_git_at(
            repository.path(),
            ["config", "user.email", "sync@example.invalid"],
        )
        .expect("email");
        std::fs::write(repository.path().join("file.txt"), "content").expect("file");
        command::successful_git_at(repository.path(), ["add", "--", "file.txt"]).expect("stage");
        command::successful_git_at(repository.path(), ["commit", "-m", "initial"]).expect("commit");
        command::successful_git_at(
            repository.path(),
            ["remote", "add", "origin", &remote.path().to_string_lossy()],
        )
        .expect("remote");
        let path = repository.path().to_string_lossy().into_owned();
        let before = working_copy_snapshot(WorkingCopyRequest {
            repository_path: path.clone(),
            worktree_path: path.clone(),
        })
        .expect("snapshot");
        let result = synchronize(SyncRequest {
            repository_path: path.clone(),
            worktree_path: path.clone(),
            kind: SyncKind::Publish,
            expected_head: before.head,
            expected_upstream_head: before.upstream_head,
        })
        .expect("publish");
        assert!(result.snapshot.upstream.is_some());
        assert_eq!(result.snapshot.remote.as_deref(), Some("origin"));

        let branch = result.snapshot.branch.clone().expect("branch");
        let peer = tempfile::tempdir().expect("peer parent");
        let peer_path = peer.path().join("clone");
        command::successful_git_at(
            peer.path(),
            [
                "clone",
                &remote.path().to_string_lossy(),
                &peer_path.to_string_lossy(),
            ],
        )
        .expect("clone peer");
        command::successful_git_at(&peer_path, ["config", "user.name", "Peer"]).expect("peer name");
        command::successful_git_at(&peer_path, ["config", "user.email", "peer@example.invalid"])
            .expect("peer email");

        std::fs::write(repository.path().join("local.txt"), "local").expect("local file");
        command::successful_git_at(repository.path(), ["add", "local.txt"]).expect("local add");
        command::successful_git_at(repository.path(), ["commit", "-m", "local rewrite"])
            .expect("local commit");
        let local_head = String::from_utf8_lossy(
            &command::successful_git_at(repository.path(), ["rev-parse", "HEAD"])
                .expect("local head")
                .stdout,
        )
        .trim()
        .to_string();

        std::fs::write(peer_path.join("peer.txt"), "first").expect("peer file");
        command::successful_git_at(&peer_path, ["add", "peer.txt"]).expect("peer add");
        command::successful_git_at(&peer_path, ["commit", "-m", "peer first"])
            .expect("peer commit");
        command::successful_git_at(&peer_path, ["push"]).expect("peer push");
        command::successful_git_at(repository.path(), ["fetch", "origin"]).expect("fetch peer");
        let reviewed = working_copy_snapshot(WorkingCopyRequest {
            repository_path: path.clone(),
            worktree_path: path.clone(),
        })
        .expect("reviewed divergence");

        std::fs::write(peer_path.join("peer.txt"), "second").expect("peer update");
        command::successful_git_at(&peer_path, ["commit", "-am", "peer second"])
            .expect("peer second commit");
        command::successful_git_at(&peer_path, ["push"]).expect("peer second push");
        let stale_lease = synchronize(SyncRequest {
            repository_path: path.clone(),
            worktree_path: path.clone(),
            kind: SyncKind::ForcePush,
            expected_head: reviewed.head.clone(),
            expected_upstream_head: reviewed.upstream_head.clone(),
        });
        assert!(
            stale_lease.is_err(),
            "the exact lease must reject a raced remote update"
        );

        command::successful_git_at(repository.path(), ["fetch", "origin"]).expect("refetch peer");
        let refreshed = working_copy_snapshot(WorkingCopyRequest {
            repository_path: path.clone(),
            worktree_path: path.clone(),
        })
        .expect("refreshed divergence");
        let forced = synchronize(SyncRequest {
            repository_path: path.clone(),
            worktree_path: path,
            kind: SyncKind::ForcePush,
            expected_head: refreshed.head,
            expected_upstream_head: refreshed.upstream_head,
        })
        .expect("force with current lease");
        assert_eq!(forced.snapshot.behind, 0);
        let remote_ref = format!("refs/heads/{branch}");
        let remote_head = command::successful_git_at(remote.path(), ["rev-parse", &remote_ref])
            .expect("remote head");
        assert_eq!(
            String::from_utf8_lossy(&remote_head.stdout).trim(),
            local_head
        );
    }
}
