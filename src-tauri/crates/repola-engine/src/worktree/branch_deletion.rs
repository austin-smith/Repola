//! Reviewed deletion of a local branch, its remote branch, or both.
//!
//! Planning is read-only and never fetches. Execution plans again, requires the
//! reviewed fingerprint to be unchanged, asks the remote which branch is its
//! default before deleting anything there, and deletes the local branch first: a
//! refusing `branch -d` then leaves the remote untouched, and the upstream that
//! `-d` checks still exists. The remote branch is deleted last under a push lease
//! pinned to the reviewed remote-tracking value.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::UNIX_EPOCH;

use super::command;
use super::command_display::git_command_line;
use super::discovery::{list_worktrees, repository_context};
use super::models::{
    BranchDeletionConfirmation, BranchDeletionExecutionRequest, BranchDeletionFingerprint,
    BranchDeletionPlan, BranchDeletionRequest, BranchDeletionResult, BranchDeletionStep,
    LocalBranchDeletion, MergeReferenceKind, RemoteBranchDeletion, WorkingCopyRequest,
    WorkingCopySnapshot,
};
use super::working_copy::working_copy_snapshot;

/// Counting stops here so a deletion review stays bounded on very deep histories.
const EXCLUSIVE_COMMIT_LIMIT: u64 = 10_000;

struct RefRecord {
    oid: String,
    upstream: String,
    upstream_remote: String,
    upstream_remote_ref: String,
}

struct RemoteCandidate {
    remote: String,
    remote_ref: String,
    tracking_ref: String,
    oid: String,
}

enum RemoteTarget {
    Available(RemoteCandidate),
    Unavailable(String),
}

/// Read-only repository state shared by every part of one deletion review.
struct Inspection<'a> {
    worktree: &'a Path,
    git_dir: &'a Path,
    refs: &'a BTreeMap<String, RefRecord>,
    occupancy: &'a BTreeMap<String, String>,
    snapshot: &'a WorkingCopySnapshot,
    default_target: Option<&'a str>,
}

/// A plan together with the exact local deletion commands it displays, so
/// execution runs what the final review showed.
struct Review {
    plan: BranchDeletionPlan,
    local_commands: Vec<Vec<String>>,
}

pub fn plan_branch_deletion(request: BranchDeletionRequest) -> Result<BranchDeletionPlan, String> {
    review(request).map(|review| review.plan)
}

fn review(request: BranchDeletionRequest) -> Result<Review, String> {
    validate_branch_ref(&request.branch_ref)?;
    if !request.delete_local && !request.delete_remote {
        return Err("Choose the local branch, the remote branch, or both.".into());
    }
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path.clone(),
        worktree_path: request.worktree_path.clone(),
    })?;
    let worktree = Path::new(&snapshot.worktree_path);
    let repository = repository_context(Path::new(&snapshot.repository_path))?;
    let refs = read_refs(worktree)?;
    let remotes = read_remotes(worktree)?;
    let occupancy: BTreeMap<String, String> = list_worktrees(&repository.path)?
        .into_iter()
        .filter_map(|seed| seed.branch.map(|branch| (branch, seed.path)))
        .collect();

    let inspection = Inspection {
        worktree,
        git_dir: &repository.git_dir,
        refs: &refs,
        occupancy: &occupancy,
        snapshot: &snapshot,
        default_target: repository.default_target.as_deref(),
    };

    let mut blockers = Vec::new();
    let mut warnings = Vec::new();
    let local_name = request.branch_ref.strip_prefix("refs/heads/");
    let record = refs
        .get(&request.branch_ref)
        .ok_or_else(|| match local_name {
            Some(name) => format!("Branch {name} no longer exists."),
            None => format!(
                "Remote-tracking branch {} no longer exists. Fetch, then review again.",
                short_ref(&request.branch_ref)
            ),
        })?;
    let remote_target = match local_name {
        Some(name) => upstream_candidate(name, record, &refs, &remotes),
        None => tracking_candidate(worktree, &request.branch_ref, record, &remotes)?,
    };
    if local_name.is_none() && request.delete_local {
        blockers.push(format!(
            "{} is a remote-tracking branch; there is no local branch to delete.",
            short_ref(&request.branch_ref)
        ));
    }
    let remote_tracking_deleted = match (&remote_target, request.delete_remote) {
        (RemoteTarget::Available(candidate), true) => Some(candidate.tracking_ref.as_str()),
        _ => None,
    };

    let local = match local_name.filter(|_| request.delete_local) {
        Some(name) => Some(local_details(
            &inspection,
            name,
            record,
            remote_tracking_deleted,
        )?),
        None => None,
    };
    let local_deleted = local.as_ref().map(|local| local.name.as_str());

    let (remote, remote_unavailable_reason) = match remote_target {
        RemoteTarget::Available(candidate) => (
            Some(remote_details(&inspection, candidate, local_deleted)?),
            None,
        ),
        RemoteTarget::Unavailable(reason) => (None, Some(reason)),
    };

    if let Some(local) = &local {
        if let Some(path) = &local.occupied_worktree_path {
            blockers.push(format!(
                "{} is checked out in the worktree at {path}. Switch that worktree to another branch before deleting it.",
                local.name
            ));
        }
        if local.is_default_branch {
            warnings.push(format!(
                "{} is the local copy of the repository's default branch.",
                local.name
            ));
        }
        if !local.contained_in_merge_reference {
            warnings.push(format!(
                "{} is not contained in {}, so git branch -d would refuse. Deleting it requires force.",
                local.name, local.merge_reference
            ));
        }
        if local.exclusive_commit_count > 0 {
            warnings.push(format!(
                "{} on {} {} on no other branch, tag, remote-tracking branch, stash, or worktree HEAD. After deletion only the reflog can recover {} until Git prunes {}.",
                commit_count(local.exclusive_commit_count, local.exclusive_commit_count_capped),
                local.name,
                if local.exclusive_commit_count == 1 { "is" } else { "are" },
                if local.exclusive_commit_count == 1 { "it" } else { "them" },
                if local.exclusive_commit_count == 1 { "it" } else { "them" },
            ));
        }
    }
    if request.delete_remote {
        match &remote {
            Some(remote) => {
                if remote.is_remote_default_branch {
                    blockers.push(format!(
                        "{} is the default branch of {}. Repola does not delete a remote's default branch.",
                        remote.display_name, remote.remote
                    ));
                }
                if !remote.tracked_by.is_empty() {
                    warnings.push(format!(
                        "{} {} {}; {} upstream will no longer exist.",
                        list_names(&remote.tracked_by),
                        if remote.tracked_by.len() == 1 {
                            "tracks"
                        } else {
                            "track"
                        },
                        remote.display_name,
                        if remote.tracked_by.len() == 1 {
                            "its"
                        } else {
                            "their"
                        },
                    ));
                }
                if remote.exclusive_commit_count > 0 {
                    warnings.push(format!(
                        "{} on {} {} in no ref that remains in this repository.",
                        commit_count(
                            remote.exclusive_commit_count,
                            remote.exclusive_commit_count_capped
                        ),
                        remote.display_name,
                        if remote.exclusive_commit_count == 1 {
                            "is"
                        } else {
                            "are"
                        },
                    ));
                }
            }
            None => blockers.push(
                remote_unavailable_reason
                    .clone()
                    .unwrap_or_else(|| "The remote branch could not be identified.".into()),
            ),
        }
    }

    let requires_force = local
        .as_ref()
        .is_some_and(|local| !local.contained_in_merge_reference);
    let remote_loses_commits = request.delete_remote
        && remote
            .as_ref()
            .is_some_and(|remote| remote.exclusive_commit_count > 0);
    let confirmation = if requires_force || remote_loses_commits {
        BranchDeletionConfirmation::TypeBranchName
    } else {
        BranchDeletionConfirmation::Confirm
    };

    let local_commands = match &local {
        Some(local) => local_deletion_commands(
            local,
            requires_force,
            has_branch_config(worktree, &local.name)?,
        ),
        None => Vec::new(),
    };
    let mut commands: Vec<String> = local_commands
        .iter()
        .map(|args| git_command_line(worktree, args))
        .collect();
    if let Some(remote) = remote.as_ref().filter(|_| request.delete_remote) {
        commands.push(git_command_line(worktree, remote_deletion_args(remote)));
    }

    let selected_remote = remote.as_ref().filter(|_| request.delete_remote);
    let fingerprint = BranchDeletionFingerprint {
        local_tip: local.as_ref().map(|local| local.tip.clone()),
        merge_reference_oid: local
            .as_ref()
            .and_then(|local| local.merge_reference_oid.clone()),
        requires_force,
        remote: selected_remote.map(|remote| remote.remote.clone()),
        remote_ref: selected_remote.map(|remote| remote.remote_ref.clone()),
        remote_oid: selected_remote.map(|remote| remote.expected_oid.clone()),
        confirmation,
    };

    let plan = BranchDeletionPlan {
        repository_path: snapshot.repository_path.clone(),
        worktree_path: snapshot.worktree_path.clone(),
        branch_name: local_name
            .map(str::to_string)
            .unwrap_or_else(|| short_ref(&request.branch_ref)),
        branch_ref: request.branch_ref,
        delete_local: request.delete_local,
        delete_remote: request.delete_remote,
        local,
        remote,
        remote_unavailable_reason,
        requires_force,
        confirmation,
        commands,
        warnings,
        blockers,
        fingerprint,
    };
    Ok(Review {
        plan,
        local_commands,
    })
}

pub fn execute_branch_deletion(
    request: BranchDeletionExecutionRequest,
) -> Result<BranchDeletionResult, String> {
    let Review {
        plan,
        local_commands,
    } = review(request.request)?;
    if let Some(blocker) = plan.blockers.first() {
        return Err(format!("The branch deletion is blocked: {blocker}"));
    }
    if plan.fingerprint != request.expected || plan.requires_force != request.force {
        return Err(
            "The branch changed after this deletion was reviewed. Review the deletion again."
                .into(),
        );
    }
    if plan.confirmation == BranchDeletionConfirmation::TypeBranchName
        && request.typed_confirmation.as_deref() != Some(plan.branch_name.as_str())
    {
        return Err(format!(
            "Type {} exactly to confirm this deletion.",
            plan.branch_name
        ));
    }

    let worktree = Path::new(&plan.worktree_path);
    let selected_remote = plan.remote.as_ref().filter(|_| plan.delete_remote);
    if let Some(remote) = selected_remote {
        confirm_not_remote_default(worktree, remote)?;
    }

    let local = match &plan.local {
        Some(local) => Some(delete_local(worktree, local, &local_commands)?),
        None => None,
    };

    let remote = match selected_remote {
        Some(remote) => {
            let (succeeded, diagnostic) = match command::git_at(
                worktree,
                remote_deletion_args(remote),
            ) {
                Ok(output) => {
                    let succeeded = output.status.success();
                    let mut diagnostic = combined_output(&output.stdout, &output.stderr);
                    if !succeeded
                        && String::from_utf8_lossy(&output.stdout).contains("(stale info)")
                    {
                        diagnostic.push_str(&format!(
                                "\n{} on {} no longer matches the reviewed commit {}, or it no longer exists. Fetch, then review again.",
                                remote.remote_ref, remote.remote, remote.expected_oid
                            ));
                    }
                    (succeeded, diagnostic)
                }
                Err(error) => (false, error.to_string()),
            };
            if !succeeded && local.is_none() {
                return Err(if diagnostic.is_empty() {
                    format!(
                        "Git did not delete {} and returned no diagnostic output.",
                        remote.display_name
                    )
                } else {
                    diagnostic
                });
            }
            Some(BranchDeletionStep {
                target: remote.display_name.clone(),
                deleted_oid: remote.expected_oid.clone(),
                succeeded,
                output: diagnostic,
                warning: None,
                recovery_command: succeeded.then(|| {
                    let refspec = format!("{}:{}", remote.expected_oid, remote.remote_ref);
                    git_command_line(worktree, ["push", "--", remote.remote.as_str(), &refspec])
                }),
            })
        }
        None => None,
    };

    let message = match (&local, &remote) {
        (Some(local), Some(remote)) if remote.succeeded => format!(
            "Deleted local branch {} and remote branch {}.",
            local.target, remote.target
        ),
        (Some(local), Some(remote)) => format!(
            "Deleted local branch {}. Remote branch {} was not deleted.",
            local.target, remote.target
        ),
        (Some(local), None) => format!("Deleted local branch {}.", local.target),
        (None, Some(remote)) => format!("Deleted remote branch {}.", remote.target),
        (None, None) => "No branch was deleted.".into(),
    };
    Ok(BranchDeletionResult {
        message,
        local,
        remote,
        audit_path: None,
        audit_warning: None,
    })
}

/// Runs the reviewed local deletion. Only the first command deletes the branch;
/// a failure after it is reported with the completed step, never as a failure.
fn delete_local(
    worktree: &Path,
    local: &LocalBranchDeletion,
    commands: &[Vec<String>],
) -> Result<BranchDeletionStep, String> {
    let (deletion, cleanup) = commands
        .split_first()
        .ok_or_else(|| format!("No command was reviewed to delete {}.", local.name))?;
    let output = command::git_at(worktree, deletion).map_err(|error| error.to_string())?;
    let diagnostic = combined_output(&output.stdout, &output.stderr);
    if !output.status.success() {
        return Err(if diagnostic.is_empty() {
            format!(
                "Git did not delete {} and returned no diagnostic output.",
                local.name
            )
        } else {
            diagnostic
        });
    }
    let warnings: Vec<String> = cleanup
        .iter()
        .filter_map(|args| {
            let failure = match command::git_at(worktree, args) {
                Ok(output) if output.status.success() => return None,
                Ok(output) => combined_output(&output.stdout, &output.stderr),
                Err(error) => error.to_string(),
            };
            Some(format!(
                "{} was deleted, but its configuration was not removed ({}). A new branch with this name would inherit it. To remove it: {}",
                local.name,
                failure.trim(),
                git_command_line(worktree, args)
            ))
        })
        .collect();
    Ok(BranchDeletionStep {
        target: local.name.clone(),
        deleted_oid: local.tip.clone(),
        succeeded: true,
        output: diagnostic,
        warning: (!warnings.is_empty()).then(|| warnings.join(" ")),
        recovery_command: Some(git_command_line(
            worktree,
            ["branch", "--", local.name.as_str(), local.tip.as_str()],
        )),
    })
}

/// The exact local deletion commands, both run and displayed. `branch -d` checks
/// containment itself when it runs. `branch -D` would delete whatever the branch
/// points to by then, so a forced deletion uses `update-ref`, which deletes the
/// branch only while it still points to the reviewed tip, and then removes the
/// branch's configuration as `branch -D` does.
fn local_deletion_commands(
    local: &LocalBranchDeletion,
    force: bool,
    has_config: bool,
) -> Vec<Vec<String>> {
    if !force {
        return vec![vec![
            "branch".into(),
            "-d".into(),
            "--".into(),
            local.name.clone(),
        ]];
    }
    let mut commands = vec![vec![
        "update-ref".into(),
        "-d".into(),
        format!("refs/heads/{}", local.name),
        local.tip.clone(),
    ]];
    if has_config {
        commands.push(vec![
            "config".into(),
            "--local".into(),
            "--remove-section".into(),
            format!("branch.{}", local.name),
        ]);
    }
    commands
}

/// Whether the repository configuration has a `branch.<name>` section.
fn has_branch_config(worktree: &Path, name: &str) -> Result<bool, String> {
    let output = command::git_at(
        worktree,
        ["config", "--local", "--list", "--name-only", "-z"],
    )
    .map_err(|error| error.to_string())?;
    let prefix = format!("branch.{name}.");
    Ok(output.stdout.split(|byte| *byte == 0).any(|key| {
        key.strip_prefix(prefix.as_bytes())
            .is_some_and(|variable| !variable.is_empty() && !variable.contains(&b'.'))
    }))
}

/// Asks the remote which branch its HEAD names, because the local
/// `refs/remotes/<remote>/HEAD` can be missing or stale. Refuses when the remote
/// cannot answer, or answers without naming a branch.
///
/// This is a check, not a lock: a push cannot be made conditional on the
/// remote's HEAD, so only the server can make this protection atomic.
fn confirm_not_remote_default(
    worktree: &Path,
    remote: &RemoteBranchDeletion,
) -> Result<(), String> {
    let output = command::git_at(
        worktree,
        [
            "ls-remote",
            "--symref",
            "--",
            remote.remote.as_str(),
            "HEAD",
        ],
    )
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Could not ask {} which branch is its default, so nothing was deleted. {}",
            remote.remote,
            combined_output(&output.stdout, &output.stderr)
        )
        .trim()
        .to_string());
    }
    match parse_remote_head(&String::from_utf8_lossy(&output.stdout)) {
        RemoteHead::Branch(target) if target == remote.remote_ref => Err(format!(
            "{} is the default branch of {}. Repola does not delete a remote's default branch.",
            remote.display_name, remote.remote
        )),
        RemoteHead::Branch(_) | RemoteHead::Absent => Ok(()),
        RemoteHead::Unnamed => Err(format!(
            "{} did not say which branch is its default, so nothing was deleted.",
            remote.remote
        )),
    }
}

#[derive(Debug, PartialEq)]
enum RemoteHead {
    /// HEAD is a symbolic ref to this branch.
    Branch(String),
    /// HEAD exists, but the remote did not report what it points to.
    Unnamed,
    /// The remote has no HEAD, so it has no default branch.
    Absent,
}

fn parse_remote_head(output: &str) -> RemoteHead {
    let mut has_head = false;
    for line in output.lines() {
        let Some((value, name)) = line.split_once('\t') else {
            continue;
        };
        if name != "HEAD" {
            continue;
        }
        if let Some(target) = value.strip_prefix("ref: ") {
            return RemoteHead::Branch(target.to_string());
        }
        has_head = true;
    }
    if has_head {
        RemoteHead::Unnamed
    } else {
        RemoteHead::Absent
    }
}

/// The exact arguments both run and displayed for the remote deletion. The lease
/// makes Git refuse unless the remote still has the reviewed commit.
fn remote_deletion_args(remote: &RemoteBranchDeletion) -> [String; 6] {
    [
        "push".into(),
        "--porcelain".into(),
        format!(
            "--force-with-lease={}:{}",
            remote.remote_ref, remote.expected_oid
        ),
        "--".into(),
        remote.remote.clone(),
        format!(":{}", remote.remote_ref),
    ]
}

fn validate_branch_ref(value: &str) -> Result<(), String> {
    let invalid = || Err("The branch to delete is not a valid branch reference.".to_string());
    if value.len() > 1024 || value.chars().any(char::is_control) {
        return invalid();
    }
    let Some(name) = value
        .strip_prefix("refs/heads/")
        .or_else(|| value.strip_prefix("refs/remotes/"))
    else {
        return invalid();
    };
    if name.is_empty() || (value.starts_with("refs/remotes/") && name.ends_with("/HEAD")) {
        return invalid();
    }
    let output =
        command::output("git", ["check-ref-format", value]).map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        invalid()
    }
}

fn read_refs(worktree: &Path) -> Result<BTreeMap<String, RefRecord>, String> {
    let output = command::successful_git_at(
        worktree,
        [
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(upstream)%00%(upstream:remotename)%00%(upstream:remoteref)",
            "refs/heads",
            "refs/remotes",
        ],
    )
    .map_err(|error| error.to_string())?;
    parse_refs(&output.stdout)
}

fn parse_refs(bytes: &[u8]) -> Result<BTreeMap<String, RefRecord>, String> {
    let mut refs = BTreeMap::new();
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let fields: Vec<String> = line
            .split(|byte| *byte == 0)
            .map(|field| String::from_utf8_lossy(field).into_owned())
            .collect();
        let [name, oid, upstream, upstream_remote, upstream_remote_ref] =
            <[String; 5]>::try_from(fields)
                .map_err(|_| "Git returned malformed branch metadata.".to_string())?;
        refs.insert(
            name,
            RefRecord {
                oid,
                upstream,
                upstream_remote,
                upstream_remote_ref,
            },
        );
    }
    Ok(refs)
}

fn read_remotes(worktree: &Path) -> Result<Vec<String>, String> {
    let output =
        command::successful_git_at(worktree, ["remote"]).map_err(|error| error.to_string())?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|remote| !remote.is_empty())
        .map(str::to_string)
        .collect())
}

/// The remote branch a local branch's configured upstream names.
fn upstream_candidate(
    name: &str,
    record: &RefRecord,
    refs: &BTreeMap<String, RefRecord>,
    remotes: &[String],
) -> RemoteTarget {
    if record.upstream.is_empty() {
        return RemoteTarget::Unavailable(format!("{name} has no upstream branch."));
    }
    let display = short_ref(&record.upstream);
    if !remotes.contains(&record.upstream_remote)
        || !record.upstream_remote_ref.starts_with("refs/heads/")
    {
        return RemoteTarget::Unavailable(format!(
            "{name} tracks {display}, which is not a branch on a configured remote."
        ));
    }
    match refs.get(&record.upstream) {
        Some(tracking) => RemoteTarget::Available(RemoteCandidate {
            remote: record.upstream_remote.clone(),
            remote_ref: record.upstream_remote_ref.clone(),
            tracking_ref: record.upstream.clone(),
            oid: tracking.oid.clone(),
        }),
        None => RemoteTarget::Unavailable(format!(
            "{display} has not been fetched, so its remote branch cannot be reviewed. Fetch, then review again."
        )),
    }
}

/// The remote branch a remote-tracking ref mirrors. Only the default fetch refspec
/// is mapped back to a remote ref; anything else is reported instead of guessed.
fn tracking_candidate(
    worktree: &Path,
    tracking_ref: &str,
    record: &RefRecord,
    remotes: &[String],
) -> Result<RemoteTarget, String> {
    let display = short_ref(tracking_ref);
    let matches: Vec<(&String, &str)> = remotes
        .iter()
        .filter_map(|remote| {
            tracking_ref
                .strip_prefix(&format!("refs/remotes/{remote}/"))
                .map(|branch| (remote, branch))
        })
        .collect();
    let (remote, branch) = match matches.as_slice() {
        [single] => *single,
        [] => {
            return Ok(RemoteTarget::Unavailable(format!(
                "{display} does not belong to a configured remote."
            )))
        }
        _ => {
            return Ok(RemoteTarget::Unavailable(format!(
            "{display} matches more than one configured remote, so its remote branch is ambiguous."
        )))
        }
    };
    let key = format!("remote.{remote}.fetch");
    let output = command::git_at(worktree, ["config", "--get-all", key.as_str()])
        .map_err(|error| error.to_string())?;
    let default_refspec = format!("refs/heads/*:refs/remotes/{remote}/*");
    let refspecs: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    if refspecs.is_empty()
        || refspecs
            .iter()
            .any(|refspec| refspec.trim_start_matches('+') != default_refspec)
    {
        return Ok(RemoteTarget::Unavailable(format!(
            "{remote} uses a custom fetch refspec, so the remote branch behind {display} cannot be identified safely."
        )));
    }
    Ok(RemoteTarget::Available(RemoteCandidate {
        remote: remote.clone(),
        remote_ref: format!("refs/heads/{branch}"),
        tracking_ref: tracking_ref.to_string(),
        oid: record.oid.clone(),
    }))
}

fn local_details(
    inspection: &Inspection,
    name: &str,
    record: &RefRecord,
    remote_tracking_deleted: Option<&str>,
) -> Result<LocalBranchDeletion, String> {
    let Inspection {
        worktree,
        refs,
        snapshot,
        ..
    } = *inspection;
    // `git branch -d` checks the upstream when it resolves, otherwise HEAD of the
    // worktree that runs it, which is the selected worktree.
    let (merge_reference_kind, merge_reference, merge_reference_oid) = match refs
        .get(&record.upstream)
        .filter(|_| !record.upstream.is_empty())
    {
        Some(upstream) => (
            MergeReferenceKind::Upstream,
            short_ref(&record.upstream),
            Some(upstream.oid.clone()),
        ),
        None => (
            MergeReferenceKind::Head,
            snapshot
                .branch
                .as_ref()
                .map(|branch| format!("HEAD ({branch})"))
                .unwrap_or_else(|| "HEAD (detached)".into()),
            snapshot.head.clone(),
        ),
    };
    let contained_in_merge_reference = match &merge_reference_oid {
        Some(reference) => is_ancestor(worktree, &record.oid, reference)?,
        None => false,
    };
    let default_target = inspection
        .default_target
        .and_then(|target| refs.get(target).map(|record| (target, record)));
    let contained_in_default_target = match default_target {
        Some((_, target)) => Some(is_ancestor(worktree, &record.oid, &target.oid)?),
        None => None,
    };
    let local_ref = format!("refs/heads/{name}");
    let mut excluded = vec![local_ref.as_str()];
    excluded.extend(remote_tracking_deleted);
    let (exclusive_commit_count, exclusive_commit_count_capped) =
        exclusive_commits(worktree, &record.oid, &excluded)?;
    Ok(LocalBranchDeletion {
        name: name.to_string(),
        tip: record.oid.clone(),
        merge_reference,
        merge_reference_kind,
        merge_reference_oid,
        contained_in_merge_reference,
        default_target: default_target.map(|(target, _)| short_ref(target)),
        contained_in_default_target,
        occupied_worktree_path: inspection.occupancy.get(name).cloned(),
        is_default_branch: default_target
            .and_then(|(target, _)| target.strip_prefix("refs/remotes/origin/"))
            == Some(name),
        upstream: (!record.upstream.is_empty()).then(|| short_ref(&record.upstream)),
        exclusive_commit_count,
        exclusive_commit_count_capped,
    })
}

fn remote_details(
    inspection: &Inspection,
    candidate: RemoteCandidate,
    local_deleted: Option<&str>,
) -> Result<RemoteBranchDeletion, String> {
    let worktree = inspection.worktree;
    let local_ref = local_deleted.map(|name| format!("refs/heads/{name}"));
    let mut excluded = vec![candidate.tracking_ref.as_str()];
    excluded.extend(local_ref.as_deref());
    let (exclusive_commit_count, exclusive_commit_count_capped) =
        exclusive_commits(worktree, &candidate.oid, &excluded)?;
    let tracked_by = inspection
        .refs
        .iter()
        .filter(|(_, record)| record.upstream == candidate.tracking_ref)
        .filter_map(|(name, _)| name.strip_prefix("refs/heads/"))
        .filter(|name| Some(*name) != local_deleted)
        .map(|name| match inspection.occupancy.get(name) {
            Some(path) => format!("{name} (checked out at {path})"),
            None => name.to_string(),
        })
        .collect();
    let symbolic_head = format!("refs/remotes/{}/HEAD", candidate.remote);
    let remote_head = command::git_at(worktree, ["symbolic-ref", "-q", symbolic_head.as_str()])
        .map_err(|error| error.to_string())?;
    let is_remote_default_branch = remote_head.status.success()
        && String::from_utf8_lossy(&remote_head.stdout).trim() == candidate.tracking_ref;
    Ok(RemoteBranchDeletion {
        display_name: short_ref(&candidate.tracking_ref),
        tracking_ref_updated_at: tracking_ref_updated_at(worktree, &candidate.tracking_ref)?,
        last_fetched_at: last_fetched_at(inspection.git_dir),
        is_remote_default_branch,
        tracked_by,
        exclusive_commit_count,
        exclusive_commit_count_capped,
        expected_oid: candidate.oid,
        remote: candidate.remote,
        remote_ref: candidate.remote_ref,
        tracking_ref: candidate.tracking_ref,
    })
}

fn is_ancestor(worktree: &Path, ancestor: &str, descendant: &str) -> Result<bool, String> {
    let output = command::git_at(
        worktree,
        ["merge-base", "--is-ancestor", ancestor, descendant],
    )
    .map_err(|error| error.to_string())?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
    }
}

/// Commits reachable from `tip` but from no ref (including stashes and every
/// worktree's HEAD) other than the refs that the deletion removes.
fn exclusive_commits(worktree: &Path, tip: &str, removed: &[&str]) -> Result<(u64, bool), String> {
    let limit = format!("--max-count={}", EXCLUSIVE_COMMIT_LIMIT + 1);
    let mut args = vec![
        "rev-list".to_string(),
        "--count".to_string(),
        limit,
        tip.to_string(),
        "--not".to_string(),
    ];
    args.extend(
        removed
            .iter()
            .map(|reference| format!("--exclude={reference}")),
    );
    args.push("--all".to_string());
    let output = command::successful_git_at(worktree, &args).map_err(|error| error.to_string())?;
    let count: u64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .map_err(|_| "Git returned a malformed commit count.".to_string())?;
    Ok((
        count.min(EXCLUSIVE_COMMIT_LIMIT),
        count > EXCLUSIVE_COMMIT_LIMIT,
    ))
}

fn tracking_ref_updated_at(worktree: &Path, tracking_ref: &str) -> Result<Option<u64>, String> {
    let output = command::git_at(
        worktree,
        [
            "log",
            "--walk-reflogs",
            "--max-count=1",
            "--date=unix",
            "--format=%gd",
            tracking_ref,
            "--",
        ],
    )
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(parse_reflog_time(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_reflog_time(selector: &str) -> Option<u64> {
    let selector = selector.trim();
    let start = selector.rfind("@{")? + 2;
    selector[start..].strip_suffix('}')?.parse().ok()
}

/// `FETCH_HEAD` is per worktree: the primary's sits in the common directory and each
/// linked worktree's in `worktrees/<id>/`. The newest one is the last fetch anywhere.
fn last_fetched_at(git_dir: &Path) -> Option<u64> {
    let mut candidates = vec![git_dir.join("FETCH_HEAD")];
    if let Ok(entries) = std::fs::read_dir(git_dir.join("worktrees")) {
        candidates.extend(
            entries
                .flatten()
                .map(|entry| entry.path().join("FETCH_HEAD")),
        );
    }
    candidates
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok()?.modified().ok())
        .filter_map(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_secs())
        .max()
}

fn short_ref(reference: &str) -> String {
    reference
        .strip_prefix("refs/heads/")
        .or_else(|| reference.strip_prefix("refs/remotes/"))
        .unwrap_or(reference)
        .to_string()
}

fn commit_count(count: u64, capped: bool) -> String {
    match (count, capped) {
        (_, true) => format!("More than {count} commits"),
        (1, false) => "1 commit".into(),
        (count, false) => format!("{count} commits"),
    }
}

fn list_names(names: &[String]) -> String {
    match names {
        [single] => format!("Local branch {single}"),
        _ => format!("Local branches {}", names.join(", ")),
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
    use std::path::PathBuf;

    use super::*;

    struct Fixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        remote: PathBuf,
        repository: PathBuf,
    }

    impl Fixture {
        /// A repository with a bare origin, `main` pushed with upstream, and origin/HEAD set.
        fn new() -> Self {
            let temp = tempfile::tempdir().expect("temp directory");
            let root = dunce::canonicalize(temp.path()).expect("canonical temp path");
            let remote = root.join("origin.git");
            let repository = root.join("repository");
            std::fs::create_dir(&repository).expect("repository directory");
            git(
                &root,
                &["init", "--bare", "--initial-branch=main", path(&remote)],
            );
            git(&repository, &["init", "--initial-branch=main"]);
            identity(&repository);
            commit(&repository, "initial");
            git(&repository, &["remote", "add", "origin", path(&remote)]);
            git(&repository, &["push", "--set-upstream", "origin", "main"]);
            git(
                &repository,
                &[
                    "symbolic-ref",
                    "refs/remotes/origin/HEAD",
                    "refs/remotes/origin/main",
                ],
            );
            Self {
                _temp: temp,
                root,
                remote,
                repository,
            }
        }

        fn request(
            &self,
            branch_ref: &str,
            delete_local: bool,
            delete_remote: bool,
        ) -> BranchDeletionRequest {
            BranchDeletionRequest {
                repository_path: path(&self.repository).to_string(),
                worktree_path: path(&self.repository).to_string(),
                branch_ref: branch_ref.to_string(),
                delete_local,
                delete_remote,
            }
        }

        fn plan(
            &self,
            branch_ref: &str,
            delete_local: bool,
            delete_remote: bool,
        ) -> BranchDeletionPlan {
            plan_branch_deletion(self.request(branch_ref, delete_local, delete_remote))
                .expect("deletion plan")
        }

        /// A second clone that can move the remote behind this repository's back.
        fn peer(&self) -> PathBuf {
            let peer = self.root.join("peer");
            git(&self.root, &["clone", path(&self.remote), path(&peer)]);
            identity(&peer);
            peer
        }

        fn has_ref(&self, reference: &str) -> bool {
            command::git_at(
                &self.repository,
                ["show-ref", "--verify", "--quiet", reference],
            )
            .expect("show-ref")
            .status
            .success()
        }

        fn remote_has_branch(&self, branch: &str) -> bool {
            let reference = format!("refs/heads/{branch}");
            command::git_at(
                &self.remote,
                ["show-ref", "--verify", "--quiet", reference.as_str()],
            )
            .expect("show-ref on remote")
            .status
            .success()
        }
    }

    fn execute(
        plan: &BranchDeletionPlan,
        typed: Option<&str>,
    ) -> Result<BranchDeletionResult, String> {
        execute_branch_deletion(BranchDeletionExecutionRequest {
            request: BranchDeletionRequest {
                repository_path: plan.repository_path.clone(),
                worktree_path: plan.worktree_path.clone(),
                branch_ref: plan.branch_ref.clone(),
                delete_local: plan.delete_local,
                delete_remote: plan.delete_remote,
            },
            force: plan.requires_force,
            expected: plan.fingerprint.clone(),
            typed_confirmation: typed.map(str::to_string),
        })
    }

    fn path(value: &Path) -> &str {
        value.to_str().expect("UTF-8 fixture path")
    }

    fn identity(repository: &Path) {
        git(repository, &["config", "user.name", "Branch Deletion Test"]);
        git(
            repository,
            &["config", "user.email", "branches@example.invalid"],
        );
        git(repository, &["config", "commit.gpgsign", "false"]);
    }

    fn commit(repository: &Path, message: &str) {
        git(
            repository,
            &["commit", "--allow-empty", "--message", message],
        );
    }

    fn git(directory: &Path, args: &[&str]) {
        command::successful_git_at(directory, args).expect("fixture Git command");
    }

    #[test]
    fn deletes_a_contained_local_branch_with_d() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "contained"]);
        let plan = fixture.plan("refs/heads/contained", true, false);
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
        assert!(!plan.requires_force);
        assert_eq!(plan.confirmation, BranchDeletionConfirmation::Confirm);
        let local = plan.local.as_ref().expect("local details");
        assert_eq!(local.merge_reference_kind, MergeReferenceKind::Head);
        assert!(local.contained_in_merge_reference);
        assert_eq!(local.contained_in_default_target, Some(true));
        assert_eq!(local.exclusive_commit_count, 0);
        assert_eq!(
            plan.commands,
            vec![git_command_line(
                Path::new(&plan.worktree_path),
                ["branch", "-d", "--", "contained"]
            )]
        );

        let result = execute(&plan, None).expect("delete contained branch");
        let step = result.local.expect("local step");
        assert!(step.succeeded);
        assert_eq!(step.deleted_oid, local.tip);
        let recovery = ["branch", "--", "contained", local.tip.as_str()];
        assert_eq!(
            step.recovery_command,
            Some(git_command_line(Path::new(&plan.worktree_path), recovery))
        );
        assert!(result.remote.is_none());
        assert!(!fixture.has_ref("refs/heads/contained"));
        git(&fixture.repository, &recovery);
        assert!(
            fixture.has_ref("refs/heads/contained"),
            "the recovery command restores the branch"
        );
    }

    #[test]
    fn an_unmerged_branch_requires_force_and_the_typed_branch_name() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["switch", "--create", "unmerged"]);
        commit(&fixture.repository, "only on unmerged");
        git(&fixture.repository, &["switch", "main"]);

        let plan = fixture.plan("refs/heads/unmerged", true, false);
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
        assert!(plan.requires_force);
        assert_eq!(
            plan.confirmation,
            BranchDeletionConfirmation::TypeBranchName
        );
        let local = plan.local.as_ref().expect("local details");
        assert!(!local.contained_in_merge_reference);
        assert_eq!(local.exclusive_commit_count, 1);
        assert_eq!(
            plan.commands,
            vec![git_command_line(
                Path::new(&plan.worktree_path),
                [
                    "update-ref",
                    "-d",
                    "refs/heads/unmerged",
                    local.tip.as_str()
                ]
            )]
        );

        let mut unforced = plan.clone();
        unforced.requires_force = false;
        execute(&unforced, Some("unmerged"))
            .expect_err("an unforced review must not force the deletion");
        execute(&plan, None).expect_err("a forced deletion requires the typed branch name");
        execute(&plan, Some("UNMERGED")).expect_err("the typed name must match exactly");
        assert!(fixture.has_ref("refs/heads/unmerged"));

        execute(&plan, Some("unmerged")).expect("forced deletion after typed confirmation");
        assert!(!fixture.has_ref("refs/heads/unmerged"));
    }

    #[test]
    fn a_forced_deletion_removes_only_the_reviewed_tip_and_its_configuration() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["switch", "--create", "forced"]);
        git(
            &fixture.repository,
            &["push", "--set-upstream", "origin", "forced"],
        );
        commit(&fixture.repository, "not on the upstream");
        git(&fixture.repository, &["switch", "main"]);
        let plan = fixture.plan("refs/heads/forced", true, false);
        assert!(plan.requires_force);
        let local = plan.local.as_ref().expect("local details");
        let worktree = Path::new(&plan.worktree_path);
        assert_eq!(
            plan.commands,
            vec![
                git_command_line(
                    worktree,
                    ["update-ref", "-d", "refs/heads/forced", local.tip.as_str()]
                ),
                git_command_line(
                    worktree,
                    ["config", "--local", "--remove-section", "branch.forced"]
                ),
            ]
        );

        // A branch that moves between the final review and the deletion keeps
        // its unreviewed commits: the reviewed command no longer matches it.
        let deletion = &local_deletion_commands(local, true, true)[0];
        git(
            &fixture.repository,
            &["update-ref", "refs/heads/forced", "main"],
        );
        let refused = command::git_at(worktree, deletion).expect("run update-ref");
        assert!(!refused.status.success());
        assert!(fixture.has_ref("refs/heads/forced"));

        git(
            &fixture.repository,
            &["update-ref", "refs/heads/forced", local.tip.as_str()],
        );
        execute(&plan, Some("forced")).expect("forced deletion");
        assert!(!fixture.has_ref("refs/heads/forced"));
        assert!(
            !has_branch_config(worktree, "forced").expect("read configuration"),
            "the branch's upstream configuration goes with it, as with branch -D"
        );
    }

    #[test]
    fn a_forced_deletion_reports_configuration_it_could_not_remove() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["switch", "--create", "stuck"]);
        git(
            &fixture.repository,
            &["push", "--set-upstream", "origin", "stuck"],
        );
        commit(&fixture.repository, "not on the upstream");
        git(&fixture.repository, &["switch", "main"]);
        let plan = fixture.plan("refs/heads/stuck", true, false);
        assert!(plan.requires_force);

        // Git refuses to write its configuration while another writer holds the lock.
        let lock = fixture.repository.join(".git").join("config.lock");
        std::fs::write(&lock, "").expect("hold the configuration lock");
        let result = execute(&plan, Some("stuck")).expect("the branch itself is deleted");
        std::fs::remove_file(&lock).expect("release the configuration lock");

        let step = result.local.expect("local step");
        assert!(step.succeeded);
        assert!(!fixture.has_ref("refs/heads/stuck"));
        let warning = step
            .warning
            .expect("the leftover configuration is reported");
        assert!(
            warning.contains(&git_command_line(
                Path::new(&plan.worktree_path),
                ["config", "--local", "--remove-section", "branch.stuck"]
            )),
            "{warning}"
        );
        assert!(
            has_branch_config(Path::new(&plan.worktree_path), "stuck").expect("read configuration")
        );
    }

    #[test]
    fn exclusive_commits_count_every_worktree_head() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["switch", "--create", "feature"]);
        commit(&fixture.repository, "first");
        commit(&fixture.repository, "second");
        git(&fixture.repository, &["switch", "main"]);
        let detached = fixture.root.join("detached");
        git(
            &fixture.repository,
            &["worktree", "add", "--detach", path(&detached), "feature~1"],
        );
        let plan = fixture.plan("refs/heads/feature", true, false);
        let local = plan.local.expect("local details");
        assert_eq!(
            local.exclusive_commit_count, 1,
            "the detached worktree HEAD retains the first commit"
        );
    }

    #[test]
    fn blocks_a_branch_checked_out_in_any_worktree() {
        let fixture = Fixture::new();
        let current = fixture.plan("refs/heads/main", true, false);
        assert!(current
            .blockers
            .iter()
            .any(|blocker| blocker.contains("checked out")));
        execute(&current, None).expect_err("the current branch must not be deleted");

        let linked = fixture.root.join("linked");
        git(
            &fixture.repository,
            &["worktree", "add", "-b", "occupied", path(&linked)],
        );
        let plan = fixture.plan("refs/heads/occupied", true, false);
        let local = plan.local.as_ref().expect("local details");
        assert_eq!(local.occupied_worktree_path.as_deref(), Some(path(&linked)));
        assert!(plan
            .blockers
            .iter()
            .any(|blocker| blocker.contains(path(&linked))));
        execute(&plan, None).expect_err("an occupied branch must not be deleted");
        assert!(fixture.has_ref("refs/heads/occupied"));
    }

    #[test]
    fn a_branch_that_moves_after_review_is_not_deleted() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "moving"]);
        let plan = fixture.plan("refs/heads/moving", true, false);
        commit(&fixture.repository, "advance main");
        git(
            &fixture.repository,
            &["branch", "--force", "moving", "HEAD"],
        );
        let error = execute(&plan, None).expect_err("a moved branch is stale");
        assert!(error.contains("changed after this deletion was reviewed"));
        assert!(fixture.has_ref("refs/heads/moving"));
    }

    #[test]
    fn a_head_change_that_alters_the_merge_reference_is_stale() {
        let fixture = Fixture::new();
        commit(&fixture.repository, "second on main");
        git(&fixture.repository, &["branch", "contained-in-head"]);
        let plan = fixture.plan("refs/heads/contained-in-head", true, false);
        assert!(!plan.requires_force);
        git(&fixture.repository, &["reset", "--hard", "HEAD~1"]);
        let error = execute(&plan, None).expect_err("the merge reference moved");
        assert!(error.contains("changed after this deletion was reviewed"));
        assert!(fixture.has_ref("refs/heads/contained-in-head"));
    }

    #[test]
    fn deletes_the_local_and_remote_branch_under_a_lease() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["switch", "--create", "published"]);
        commit(&fixture.repository, "published work");
        git(
            &fixture.repository,
            &["push", "--set-upstream", "origin", "published"],
        );
        git(&fixture.repository, &["switch", "main"]);

        let local_only = fixture.plan("refs/heads/published", true, false);
        let remote = local_only
            .remote
            .as_ref()
            .expect("remote candidate is reported");
        assert_eq!(remote.remote, "origin");
        assert_eq!(remote.remote_ref, "refs/heads/published");
        assert_eq!(remote.display_name, "origin/published");
        assert!(remote.tracking_ref_updated_at.is_some());
        assert!(local_only
            .commands
            .iter()
            .all(|command| !command.contains("push")));
        let local = local_only.local.as_ref().expect("local details");
        assert_eq!(local.merge_reference_kind, MergeReferenceKind::Upstream);
        assert!(local.contained_in_merge_reference);

        let plan = fixture.plan("refs/heads/published", true, true);
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
        assert!(!plan.requires_force);
        assert_eq!(
            plan.confirmation,
            BranchDeletionConfirmation::TypeBranchName,
            "deleting both copies leaves the published commit in no remaining ref"
        );
        let remote = plan.remote.as_ref().expect("remote details");
        let push = remote_deletion_args(remote);
        assert_eq!(
            push[2],
            format!(
                "--force-with-lease=refs/heads/published:{}",
                remote.expected_oid
            )
        );
        assert_eq!(push[5], ":refs/heads/published");
        assert_eq!(
            plan.commands[1],
            git_command_line(Path::new(&plan.worktree_path), push)
        );

        let result = execute(&plan, Some("published")).expect("delete both copies");
        assert!(result.local.expect("local step").succeeded);
        let remote_step = result.remote.expect("remote step");
        assert!(remote_step.succeeded);
        assert!(!fixture.has_ref("refs/heads/published"));
        assert!(!fixture.has_ref("refs/remotes/origin/published"));
        assert!(!fixture.remote_has_branch("published"));

        let refspec = format!("{}:refs/heads/published", remote.expected_oid);
        let recovery = ["push", "--", "origin", refspec.as_str()];
        assert_eq!(
            remote_step.recovery_command,
            Some(git_command_line(Path::new(&plan.worktree_path), recovery))
        );
        git(&fixture.repository, &recovery);
        assert!(
            fixture.remote_has_branch("published"),
            "the recovery command republishes the branch"
        );
    }

    #[test]
    fn a_stale_lease_protects_a_remote_branch_that_moved() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "shared"]);
        git(&fixture.repository, &["push", "origin", "shared"]);
        let plan = fixture.plan("refs/remotes/origin/shared", false, true);
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
        assert!(plan.local.is_none());

        let peer = fixture.peer();
        git(&peer, &["switch", "shared"]);
        commit(&peer, "pushed after review");
        git(&peer, &["push", "origin", "shared"]);

        let error = execute(&plan, None).expect_err("the lease must reject a moved branch");
        assert!(error.contains("stale info"), "{error}");
        assert!(fixture.remote_has_branch("shared"));
    }

    #[test]
    fn a_remote_failure_after_the_local_deletion_is_reported_as_partial() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["switch", "--create", "partial"]);
        git(
            &fixture.repository,
            &["push", "--set-upstream", "origin", "partial"],
        );
        git(&fixture.repository, &["switch", "main"]);
        let plan = fixture.plan("refs/heads/partial", true, true);
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);

        let peer = fixture.peer();
        git(&peer, &["switch", "partial"]);
        commit(&peer, "peer work");
        git(&peer, &["push", "origin", "partial"]);

        let result = execute(&plan, Some("partial")).expect("partial result");
        assert!(result.local.as_ref().expect("local step").succeeded);
        let remote = result.remote.as_ref().expect("remote step");
        assert!(!remote.succeeded);
        assert!(remote.recovery_command.is_none());
        assert!(result.message.contains("was not deleted"));
        assert!(!fixture.has_ref("refs/heads/partial"));
        assert!(fixture.remote_has_branch("partial"));
    }

    #[test]
    fn the_remote_default_branch_is_never_deleted() {
        let fixture = Fixture::new();
        let plan = fixture.plan("refs/remotes/origin/main", false, true);
        let remote = plan.remote.as_ref().expect("remote details");
        assert!(remote.is_remote_default_branch);
        assert!(plan
            .blockers
            .iter()
            .any(|blocker| blocker.contains("default branch")));
        execute(&plan, None).expect_err("the remote default branch is protected");
        assert!(fixture.remote_has_branch("main"));
    }

    #[test]
    fn the_remote_is_asked_for_its_default_branch_before_deleting() {
        let fixture = Fixture::new();
        git(
            &fixture.repository,
            &["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"],
        );
        let plan = fixture.plan("refs/remotes/origin/main", false, true);
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
        assert!(
            !plan
                .remote
                .as_ref()
                .expect("remote details")
                .is_remote_default_branch
        );

        let error = execute(&plan, None).expect_err("the remote reports main as its default");
        assert!(error.contains("default branch"), "{error}");
        assert!(fixture.remote_has_branch("main"));
    }

    #[test]
    fn parses_the_remote_head() {
        assert_eq!(
            parse_remote_head("ref: refs/heads/main\tHEAD\n0123abcd\tHEAD\n"),
            RemoteHead::Branch("refs/heads/main".into())
        );
        assert_eq!(parse_remote_head("0123abcd\tHEAD\n"), RemoteHead::Unnamed);
        assert_eq!(parse_remote_head(""), RemoteHead::Absent);
        assert_eq!(
            parse_remote_head("0123abcd\trefs/heads/HEAD\n"),
            RemoteHead::Absent
        );
    }

    #[test]
    fn remote_deletion_lists_local_branches_that_track_it() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "topic"]);
        git(&fixture.repository, &["push", "origin", "topic"]);
        git(
            &fixture.repository,
            &["branch", "--track", "follower", "origin/topic"],
        );
        let plan = fixture.plan("refs/remotes/origin/topic", false, true);
        let remote = plan.remote.as_ref().expect("remote details");
        assert_eq!(remote.tracked_by, vec!["follower".to_string()]);
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.contains("follower")));
        assert_eq!(plan.confirmation, BranchDeletionConfirmation::Confirm);
    }

    #[test]
    fn deleting_a_remote_only_commit_requires_the_typed_branch_name() {
        let fixture = Fixture::new();
        let peer = fixture.peer();
        git(&peer, &["switch", "--create", "peer-only"]);
        commit(&peer, "exists only on the remote");
        git(&peer, &["push", "origin", "peer-only"]);
        git(&fixture.repository, &["fetch", "origin"]);

        let plan = fixture.plan("refs/remotes/origin/peer-only", false, true);
        let remote = plan.remote.as_ref().expect("remote details");
        assert_eq!(remote.exclusive_commit_count, 1);
        assert!(remote.last_fetched_at.is_some());
        assert_eq!(
            plan.confirmation,
            BranchDeletionConfirmation::TypeBranchName
        );
        execute(&plan, None).expect_err("the typed name is required");
        execute(&plan, Some("origin/peer-only")).expect("remote-only deletion");
        assert!(!fixture.remote_has_branch("peer-only"));
        assert!(!fixture.has_ref("refs/remotes/origin/peer-only"));
    }

    #[test]
    fn remote_deletion_without_an_upstream_is_blocked() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "unpublished"]);
        let plan = fixture.plan("refs/heads/unpublished", true, true);
        assert!(plan.remote.is_none());
        assert!(plan
            .remote_unavailable_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("no upstream")));
        assert!(!plan.blockers.is_empty());
        execute(&plan, None).expect_err("a blocked plan never executes");
        assert!(fixture.has_ref("refs/heads/unpublished"));
    }

    #[test]
    fn rejects_malformed_and_option_like_references() {
        let fixture = Fixture::new();
        for reference in [
            "",
            "-D",
            "--all",
            "main",
            "refs/heads/",
            "refs/tags/v1",
            "refs/remotes/origin/HEAD",
            "refs/heads/a..b",
            "refs/heads/bad\nname",
        ] {
            plan_branch_deletion(fixture.request(reference, true, false)).expect_err(reference);
        }
        plan_branch_deletion(fixture.request("refs/heads/main", false, false))
            .expect_err("a deletion must choose at least one branch");
    }

    #[test]
    fn a_branch_named_like_an_option_is_passed_as_a_name() {
        let fixture = Fixture::new();
        git(
            &fixture.repository,
            &["update-ref", "refs/heads/-D", "HEAD"],
        );
        let plan = fixture.plan("refs/heads/-D", true, false);
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
        execute(&plan, None).expect("delete the oddly named branch");
        assert!(!fixture.has_ref("refs/heads/-D"));
        assert!(fixture.has_ref("refs/heads/main"));
    }

    #[test]
    fn parses_reflog_selectors() {
        assert_eq!(
            parse_reflog_time("origin/main@{1791088354}\n"),
            Some(1791088354)
        );
        assert_eq!(parse_reflog_time("weird@{name}@{12}"), Some(12));
        assert_eq!(parse_reflog_time("origin/main"), None);
        assert_eq!(parse_reflog_time(""), None);
    }
}
