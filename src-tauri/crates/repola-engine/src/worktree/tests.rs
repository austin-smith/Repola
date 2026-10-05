use std::path::Path;
use std::process::Command;

use super::actions::{execute_action, prepare_action, review_worktree};
use super::discovery::{parse_worktree_porcelain, remote_provider, scan};
use super::inspection::{parse_status_porcelain, registration_state};
use super::models::{
    ActionExecutionRequest, ActionKind, ActionRequest, RegistrationKind, RemoteProvider,
    ScanRequest, WorktreeSeed,
};

#[test]
fn scan_includes_only_explicitly_registered_repositories() {
    let temp = tempfile::tempdir().expect("temp directory");
    let hidden = temp.path().join("hidden");
    let visible = temp.path().join("visible");
    for repository in [&hidden, &visible] {
        let status = Command::new("git")
            .args(["init", "-b", "main"])
            .arg(repository)
            .status()
            .expect("run git init");
        assert!(status.success());
    }
    let result = scan(ScanRequest {
        repository_paths: vec![visible.to_string_lossy().into_owned()],
    })
    .expect("scan");
    assert_eq!(result.repositories.len(), 1);
    assert_eq!(result.repositories[0].name, "visible");
    assert!(result
        .repositories
        .iter()
        .all(|repository| repository.path != hidden.to_string_lossy()));
}

#[test]
fn scanning_without_registered_repositories_returns_an_empty_inventory() {
    let result = scan(ScanRequest::default()).expect("empty inventory");
    assert!(result.repository_paths.is_empty());
    assert!(result.repositories.is_empty());
    assert!(result.worktrees.is_empty());
}

#[test]
fn missing_registered_repositories_are_reported_without_guessing() {
    let temp = tempfile::tempdir().expect("temp directory");
    let missing = temp.path().join("absent").to_string_lossy().into_owned();
    let result = scan(ScanRequest {
        repository_paths: vec![missing.clone()],
    })
    .expect("missing repositories become warnings");
    assert!(result.repositories.is_empty());
    assert_eq!(result.warnings.len(), 1);
    assert!(result.warnings[0].starts_with(&missing));
}

#[test]
fn a_parent_folder_never_causes_nested_repositories_to_be_discovered() {
    let temp = tempfile::tempdir().expect("temp directory");
    let nested = temp.path().join("nested");
    let status = Command::new("git")
        .args(["init", "-b", "main"])
        .arg(&nested)
        .status()
        .expect("run git init");
    assert!(status.success());

    let result = scan(ScanRequest {
        repository_paths: vec![temp.path().to_string_lossy().into_owned()],
    })
    .expect("invalid explicit path becomes a warning");
    assert!(result.repositories.is_empty());
    assert_eq!(result.warnings.len(), 1);
}

#[test]
fn parses_nul_terminated_worktree_porcelain() {
    let input = b"worktree /repo\0HEAD abc123\0branch refs/heads/main\0\0worktree /tmp/feature\0HEAD def456\0detached\0locked agent session\0\0worktree /tmp/gone\0HEAD 012345\0branch refs/heads/old\0prunable gitdir file points to non-existent location\0\0";
    let records = parse_worktree_porcelain(input);

    assert_eq!(records.len(), 3);
    assert!(records[0].is_primary);
    assert_eq!(records[0].branch.as_deref(), Some("main"));
    assert!(records[1].detached);
    assert_eq!(records[1].locked_reason.as_deref(), Some("agent session"));
    assert_eq!(
        records[2].prunable_reason.as_deref(),
        Some("gitdir file points to non-existent location")
    );
}

#[test]
fn parses_a_bare_main_worktree() {
    let records = parse_worktree_porcelain(
        b"worktree /repo.git\0bare\0\0worktree /tmp/feature\0HEAD def456\0branch refs/heads/feature\0\0",
    );

    assert_eq!(records.len(), 2);
    assert!(records[0].bare);
    assert!(!records[1].bare);
}

#[test]
fn worktree_paths_from_git_use_native_separators() {
    let records = parse_worktree_porcelain(b"worktree C:/code/repo\0HEAD abc123\0\0");
    let expected = if cfg!(windows) {
        r"C:\code\repo"
    } else {
        "C:/code/repo"
    };
    assert_eq!(records[0].path, expected);
    assert_eq!(
        Path::new(&records[0].path),
        Path::new("C:/code/repo"),
        "registration lookups compare paths, not separator styles"
    );
}

#[test]
fn parses_status_without_counting_rename_path_payloads() {
    let input = b" M src/app.rs\0M  Cargo.toml\0?? notes.txt\0UU conflict.rs\0R  new-name.rs\0?? looks-like-status.txt\0";
    let summary = parse_status_porcelain(input);

    assert!(summary.available);
    assert_eq!(summary.total, 5);
    assert_eq!(summary.staged, 3);
    assert_eq!(summary.unstaged, 2);
    assert_eq!(summary.untracked, 1);
    assert_eq!(summary.conflicted, 1);
}

#[test]
fn locked_registration_takes_precedence_over_prunable_metadata() {
    let seed = WorktreeSeed {
        path: "/tmp/missing".to_string(),
        bare: false,
        head: None,
        branch: Some("old-work".to_string()),
        detached: false,
        is_primary: false,
        locked_reason: Some("agent session".to_string()),
        prunable_reason: Some("missing gitdir".to_string()),
    };

    let registration = registration_state(&seed, false, false);

    assert_eq!(registration.kind, RegistrationKind::Locked);
}

#[test]
fn classifies_remote_providers() {
    assert_eq!(
        remote_provider(Some("git@github.com:austin/example.git")),
        RemoteProvider::GitHub
    );
    assert_eq!(
        remote_provider(Some("ssh://git@github.corp.example/platform/app.git")),
        RemoteProvider::GitHub
    );
    assert_eq!(
        remote_provider(Some("https://dev.azure.com/org/project/_git/repo")),
        RemoteProvider::AzureDevOps
    );
    assert_eq!(remote_provider(None), RemoteProvider::None);
}

#[test]
fn removal_revalidates_cleanliness_and_preserves_the_branch() {
    let temp = tempfile::tempdir().expect("temp directory");
    let root = dunce::canonicalize(temp.path()).expect("canonical temp path");
    let repository = root.join("repository");
    let worktree = root.join("linked");
    std::fs::create_dir(&repository).expect("repository directory");
    git(&repository, &["init", "--initial-branch", "main"]);
    // Fixtures assert on exact bytes; keep the host's line-ending conversion out of them.
    git(&repository, &["config", "core.autocrlf", "false"]);
    git(&repository, &["config", "user.name", "Repola Tests"]);
    git(
        &repository,
        &["config", "user.email", "tests@example.invalid"],
    );
    std::fs::write(repository.join("README.md"), "fixture\n").expect("fixture file");
    git(&repository, &["add", "README.md"]);
    git(&repository, &["commit", "-m", "initial commit"]);
    git(
        &repository,
        &[
            "worktree",
            "add",
            "-b",
            "old-work",
            worktree.to_str().expect("UTF-8 worktree path"),
        ],
    );

    let request = ActionRequest {
        kind: ActionKind::Remove,
        repository_path: repository.to_string_lossy().into_owned(),
        worktree_path: worktree.to_string_lossy().into_owned(),
    };
    let reviewed = prepare_action(request.clone()).expect("clean worktree is removable");

    git(&worktree, &["branch", "-m", "renamed-work"]);
    let stale_branch = execute_action(ActionExecutionRequest {
        kind: reviewed.kind,
        repository_path: reviewed.repository_path.clone(),
        worktree_path: reviewed.worktree_path.clone(),
        expected_head: reviewed.expected_head.clone(),
        expected_branch: reviewed.branch.clone(),
        expected_affected_paths: reviewed.affected_paths.clone(),
    });
    assert!(
        stale_branch.is_err(),
        "a branch rename after review must require a new preflight"
    );
    assert!(
        worktree.is_dir(),
        "stale review must preserve the worktree directory"
    );
    git(&worktree, &["branch", "-m", "old-work"]);

    std::fs::write(worktree.join("untracked.txt"), "do not delete\n").expect("untracked file");
    let blocked = execute_action(ActionExecutionRequest {
        kind: reviewed.kind,
        repository_path: reviewed.repository_path.clone(),
        worktree_path: reviewed.worktree_path.clone(),
        expected_head: reviewed.expected_head.clone(),
        expected_branch: reviewed.branch.clone(),
        expected_affected_paths: reviewed.affected_paths.clone(),
    });
    assert!(blocked.is_err(), "a changed worktree must be blocked");
    assert!(
        worktree.is_dir(),
        "blocked removal must preserve the directory"
    );

    std::fs::remove_file(worktree.join("untracked.txt")).expect("remove test fixture");
    let reviewed = prepare_action(request).expect("clean worktree is removable after re-review");
    let removed = execute_action(ActionExecutionRequest {
        kind: reviewed.kind,
        repository_path: reviewed.repository_path,
        worktree_path: reviewed.worktree_path,
        expected_head: reviewed.expected_head,
        expected_branch: reviewed.branch,
        expected_affected_paths: reviewed.affected_paths,
    })
    .expect("normal Git removal succeeds");

    assert!(!worktree.exists(), "Git removes the linked directory");
    let branch = Command::new("git")
        .current_dir(&repository)
        .args(["show-ref", "--verify", "refs/heads/old-work"])
        .output()
        .expect("query branch");
    assert!(branch.status.success(), "removal must preserve the branch");
    let follow_up = removed
        .follow_up
        .expect("the retained branch is offered for review");
    assert_eq!(follow_up.branch, "old-work");
    assert_eq!(
        Path::new(&follow_up.worktree_path),
        repository,
        "the retained branch is reviewed from the primary checkout"
    );
}

#[test]
fn removal_in_a_bare_repository_reviews_the_branch_from_a_remaining_checkout() {
    let temp = tempfile::tempdir().expect("temp directory");
    let root = dunce::canonicalize(temp.path()).expect("canonical temp path");
    let source = fixture_repository(&root);
    let bare = root.join("bare.git");
    let kept = root.join("kept");
    let linked = root.join("linked");
    git(
        &root,
        &[
            "clone",
            "--bare",
            source.to_str().expect("UTF-8 source path"),
            bare.to_str().expect("UTF-8 bare path"),
        ],
    );
    git(
        &bare,
        &["worktree", "add", kept.to_str().expect("UTF-8"), "main"],
    );
    git(
        &bare,
        &[
            "worktree",
            "add",
            "-b",
            "old-work",
            linked.to_str().expect("UTF-8"),
        ],
    );

    let reviewed = prepare_action(ActionRequest {
        kind: ActionKind::Remove,
        repository_path: bare.to_string_lossy().into_owned(),
        worktree_path: linked.to_string_lossy().into_owned(),
    })
    .expect("clean worktree is removable");
    let removed = execute_action(ActionExecutionRequest {
        kind: reviewed.kind,
        repository_path: reviewed.repository_path,
        worktree_path: reviewed.worktree_path,
        expected_head: reviewed.expected_head,
        expected_branch: reviewed.branch,
        expected_affected_paths: reviewed.affected_paths,
    })
    .expect("normal Git removal succeeds");

    let follow_up = removed
        .follow_up
        .expect("the retained branch is offered for review");
    assert_eq!(
        Path::new(&follow_up.worktree_path),
        kept,
        "a bare repository has no checkout of its own to review from"
    );
}

#[test]
fn removal_reviews_the_branch_from_a_checkout_that_still_exists() {
    let temp = tempfile::tempdir().expect("temp directory");
    let root = dunce::canonicalize(temp.path()).expect("canonical temp path");
    let source = fixture_repository(&root);
    let bare = root.join("bare.git");
    let gone = root.join("gone");
    let kept = root.join("kept");
    let linked = root.join("linked");
    git(
        &root,
        &[
            "clone",
            "--bare",
            source.to_str().expect("UTF-8 source path"),
            bare.to_str().expect("UTF-8 bare path"),
        ],
    );
    // Locked, so Git keeps its registration after its directory disappears.
    git(
        &bare,
        &[
            "worktree",
            "add",
            "--detach",
            gone.to_str().expect("UTF-8"),
            "main",
        ],
    );
    git(&bare, &["worktree", "lock", gone.to_str().expect("UTF-8")]);
    std::fs::remove_dir_all(&gone).expect("remove the locked worktree's directory");
    git(
        &bare,
        &["worktree", "add", kept.to_str().expect("UTF-8"), "main"],
    );
    git(
        &bare,
        &[
            "worktree",
            "add",
            "-b",
            "old-work",
            linked.to_str().expect("UTF-8"),
        ],
    );

    let reviewed = prepare_action(ActionRequest {
        kind: ActionKind::Remove,
        repository_path: bare.to_string_lossy().into_owned(),
        worktree_path: linked.to_string_lossy().into_owned(),
    })
    .expect("clean worktree is removable");
    let removed = execute_action(ActionExecutionRequest {
        kind: reviewed.kind,
        repository_path: reviewed.repository_path,
        worktree_path: reviewed.worktree_path,
        expected_head: reviewed.expected_head,
        expected_branch: reviewed.branch,
        expected_affected_paths: reviewed.affected_paths,
    })
    .expect("normal Git removal succeeds");

    let follow_up = removed
        .follow_up
        .expect("the retained branch is offered for review");
    assert_eq!(Path::new(&follow_up.worktree_path), kept);
}

#[test]
fn a_retained_branch_is_reviewed_only_from_a_checkout_git_can_work_in() {
    let temp = tempfile::tempdir().expect("temp directory");
    let root = dunce::canonicalize(temp.path()).expect("canonical temp path");
    let source = fixture_repository(&root);
    let bare = root.join("bare.git");
    git(
        &root,
        &[
            "clone",
            "--bare",
            source.to_str().expect("UTF-8 source path"),
            bare.to_str().expect("UTF-8 bare path"),
        ],
    );
    // Inside another repository, which Git finds once the `.git` file is gone.
    let hollow = source.join("hollow");
    let hollow_path = hollow.to_str().expect("UTF-8 hollow path");
    git(&bare, &["worktree", "add", "--detach", hollow_path, "main"]);
    git(&bare, &["worktree", "lock", hollow_path]);
    std::fs::remove_file(hollow.join(".git")).expect("remove the .git file");
    assert_eq!(review_worktree(&bare), None);

    let kept = root.join("kept");
    git(
        &bare,
        &[
            "worktree",
            "add",
            "--detach",
            kept.to_str().expect("UTF-8 kept path"),
            "main",
        ],
    );
    assert_eq!(
        review_worktree(&bare).map(std::path::PathBuf::from),
        Some(kept)
    );
}

fn fixture_repository(root: &Path) -> std::path::PathBuf {
    let repository = root.join("repository");
    std::fs::create_dir(&repository).expect("repository directory");
    git(&repository, &["init", "--initial-branch", "main"]);
    // Fixtures assert on exact bytes; keep the host's line-ending conversion out of them.
    git(&repository, &["config", "core.autocrlf", "false"]);
    git(&repository, &["config", "user.name", "Repola Tests"]);
    git(
        &repository,
        &["config", "user.email", "tests@example.invalid"],
    );
    std::fs::write(repository.join("README.md"), "fixture\n").expect("fixture file");
    git(&repository, &["add", "README.md"]);
    git(&repository, &["commit", "-m", "initial commit"]);
    repository
}

/// Adds a bare "origin" remote, pushes main, and records origin/HEAD so the
/// repository gains a default integration target like a cloned repository.
fn fixture_remote(root: &Path, repository: &Path) {
    let remote = root.join("origin.git");
    git(root, &["init", "--bare", remote.to_str().expect("UTF-8")]);
    git(
        repository,
        &["remote", "add", "origin", remote.to_str().expect("UTF-8")],
    );
    git(repository, &["push", "-u", "origin", "main"]);
    git(
        repository,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
}

#[test]
fn counts_commits_missing_from_every_remote() {
    let temp = tempfile::tempdir().expect("temp directory");
    let root = dunce::canonicalize(temp.path()).expect("canonical temp path");
    let repository = fixture_repository(&root);
    fixture_remote(&root, &repository);
    let worktree = root.join("linked");
    git(
        &repository,
        &[
            "worktree",
            "add",
            "-b",
            "feature",
            worktree.to_str().expect("UTF-8 worktree path"),
        ],
    );

    let context = super::discovery::repository_context(&repository).expect("repository context");
    let seed = |path: &Path| {
        super::discovery::list_worktrees(&repository)
            .expect("worktree list")
            .into_iter()
            .find(|seed| seed.path == path.to_string_lossy())
            .expect("registered worktree")
    };

    let record = super::inspection::inspect_worktree(&context, &seed(&worktree));
    assert_eq!(
        record.unpushed_commit_count,
        Some(0),
        "a branch at the pushed tip has no unpushed commits"
    );

    std::fs::write(worktree.join("work.txt"), "local\n").expect("fixture file");
    git(&worktree, &["add", "work.txt"]);
    git(&worktree, &["commit", "-m", "local-only work"]);
    let record = super::inspection::inspect_worktree(&context, &seed(&worktree));
    assert_eq!(
        record.unpushed_commit_count,
        Some(1),
        "a commit on no remote must be counted"
    );
    assert!(
        record
            .safety
            .reasons
            .iter()
            .any(|reason| reason.contains("not on any remote")),
        "unpushed commits must surface in the review signal: {:?}",
        record.safety.reasons
    );
}

#[test]
fn worktree_changes_returns_patch_and_untracked_files() {
    let temp = tempfile::tempdir().expect("temp directory");
    let root = dunce::canonicalize(temp.path()).expect("canonical temp path");
    let repository = fixture_repository(&root);
    let worktree = root.join("linked");
    git(
        &repository,
        &[
            "worktree",
            "add",
            "-b",
            "feature",
            worktree.to_str().expect("UTF-8 worktree path"),
        ],
    );
    std::fs::write(worktree.join("README.md"), "edited fixture\n").expect("tracked change");
    std::fs::write(worktree.join("notes.txt"), "untracked\n").expect("untracked file");

    let changes = super::inspection::worktree_changes(
        &repository.to_string_lossy(),
        &worktree.to_string_lossy(),
    )
    .expect("changes are inspectable");

    assert!(changes.available);
    assert!(!changes.truncated);
    assert!(
        changes.patch.contains("diff --git a/README.md b/README.md"),
        "tracked modifications must appear in the patch"
    );
    assert!(
        changes.patch.contains("+edited fixture"),
        "patch must include the new content"
    );
    assert_eq!(changes.untracked, vec!["notes.txt".to_string()]);
}

fn git(directory: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(directory)
        .args(args)
        .output()
        .expect("run Git fixture command");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}
