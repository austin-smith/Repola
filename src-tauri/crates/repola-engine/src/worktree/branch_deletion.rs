//! Reviewed deletion of a local branch, its remote branch, or both.
//!
//! Planning is read-only and never fetches. Execution reviews the branch again
//! and refuses unless the review is unchanged: the same tips, the same
//! reference the branch was found contained in, the same commits left
//! unreachable, the same push URL, warnings, and commands. It then deletes
//! the local branch before the remote one, so a refused local deletion leaves
//! the remote untouched and the upstream it was checked against still exists.
//!
//! What Git enforces atomically:
//! - The local branch is deleted only at its reviewed tip, with
//!   `update-ref --no-deref`, which never reaches another branch through a
//!   symbolic ref. `git branch -d` and `-D` would delete whatever the branch
//!   points to when they run.
//! - The remote branch is deleted only while the remote still has the reviewed
//!   commit, under a push lease.
//!
//! What is checked immediately before acting, because Git offers no way to
//! make the deletion conditional on it:
//! - No worktree uses the branch: none has it checked out, and no rebase or
//!   bisect in progress will return to it or move it. These are the checks
//!   `git branch -d` and `-D` make, read the way Git reads them, except the
//!   one case described at [`state_branch`].
//! - The branch is still a regular ref, not a symbolic one.
//! - Each deletion still leaves unreachable exactly the commits the review
//!   found.
//! - Unless the deletion is forced, the reference the branch was found
//!   contained in has not moved, as `git branch -d` requires.
//! - The remote still pushes to the reviewed URL, and that URL does not name
//!   this branch as its default.
//!
//! Another process can still change these between the check and the deletion;
//! `git branch -d` has the same window between its checks and its deletion.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::UNIX_EPOCH;

use sha2::{Digest, Sha256};

use crate::diagnostics;

use super::branch_pull_requests::branch_pull_requests;
use super::command;
use super::command_display::git_command_line;
use super::discovery::{list_worktrees, native_path, repository_context};
use super::models::{
    BranchDeletionConfirmation, BranchDeletionExecutionRequest, BranchDeletionFingerprint,
    BranchDeletionPlan, BranchDeletionRequest, BranchDeletionResult, BranchDeletionStep,
    BranchPullRequests, LocalBranchDeletion, MergeReferenceKind, OpenPullRequest,
    RemoteBranchDeletion, RepositoryContext, WorkingCopyRequest, WorkingCopySnapshot,
};
use super::working_copy::working_copy_snapshot;

/// Counting stops here so a deletion review stays bounded on very deep histories.
const EXCLUSIVE_COMMIT_LIMIT: u64 = 10_000;

/// How a worktree uses a branch that Git therefore refuses to delete.
#[derive(Clone, Copy)]
enum Activity {
    CheckedOut,
    Rebasing,
    Bisecting,
}

#[derive(Clone)]
struct BranchUse {
    worktree: String,
    activity: Activity,
}

impl BranchUse {
    fn activity(&self) -> &'static str {
        match self.activity {
            Activity::CheckedOut => "checked out",
            Activity::Rebasing => "being rebased",
            Activity::Bisecting => "being bisected",
        }
    }

    fn blocker(&self, name: &str) -> String {
        let remedy = match self.activity {
            Activity::CheckedOut => "Switch that worktree to another branch before deleting it.",
            Activity::Rebasing => "Finish or abort the rebase before deleting it.",
            Activity::Bisecting => "End the bisect before deleting it.",
        };
        format!(
            "{name} is {} in the worktree at {}. {remedy}",
            self.activity(),
            self.worktree
        )
    }
}

struct RefRecord {
    oid: String,
    /// The ref this one points to when it is symbolic; empty otherwise.
    symref: String,
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
    reachability: &'a Reachability<'a>,
    refs: &'a BTreeMap<String, RefRecord>,
    occupancy: &'a BTreeMap<String, BranchUse>,
    snapshot: &'a WorkingCopySnapshot,
    default_target: Option<&'a str>,
}

/// A plan together with the exact local deletion commands it displays, so
/// execution runs what the final review showed, the reference whose containment
/// of the branch the review checked, and the one URL a remote deletion pushes to.
struct Review {
    plan: BranchDeletionPlan,
    local_commands: Vec<Vec<String>>,
    merge_reference: Option<String>,
    repository: RepositoryContext,
    push_url: Option<String>,
}

pub fn plan_branch_deletion(request: BranchDeletionRequest) -> Result<BranchDeletionPlan, String> {
    review(request, None).map(|review| review.plan)
}

/// Reviews a deletion. At execution, `reviewed_pull_requests` are the open
/// pull requests the user was shown, which keep requiring the typed branch name
/// even when the provider cannot be asked again.
fn review(
    request: BranchDeletionRequest,
    reviewed_pull_requests: Option<&[String]>,
) -> Result<Review, String> {
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
    let mut refs = read_refs(worktree)?;
    // `git branch -d` checks an upstream wherever its fetch refspec stores it.
    if let Some(upstream) = refs
        .get(&request.branch_ref)
        .map(|record| record.upstream.clone())
        .filter(|upstream| !upstream.is_empty() && !refs.contains_key(upstream))
    {
        if let Some(record) = read_ref(worktree, &upstream)? {
            refs.insert(upstream, record);
        }
    }
    let remotes = read_remotes(worktree)?;
    let occupancy = branches_in_use(&repository)?;
    let reachability = Reachability::read(worktree, &repository.path)?;

    let inspection = Inspection {
        worktree,
        git_dir: &repository.git_dir,
        reachability: &reachability,
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
    let remote_target = match remote_target {
        RemoteTarget::Available(candidate) => {
            single_push_url(worktree, &candidate.remote).map(|url| (candidate, url))
        }
        RemoteTarget::Unavailable(reason) => Err(reason),
    };
    if !record.symref.is_empty() {
        blockers.push(format!(
            "{} is a symbolic ref to {}. Repola deletes only branches that point directly to a commit.",
            short_ref(&request.branch_ref),
            short_ref(&record.symref)
        ));
    }
    if local_name.is_none() && request.delete_local {
        blockers.push(format!(
            "{} is a remote-tracking branch; there is no local branch to delete.",
            short_ref(&request.branch_ref)
        ));
    }
    let remote_tracking_deleted = match (&remote_target, request.delete_remote) {
        (Ok((candidate, _)), true) => Some(candidate.tracking_ref.as_str()),
        _ => None,
    };

    let (local, merge_reference, local_reachability) =
        match local_name.filter(|_| request.delete_local) {
            Some(name) => {
                let (local, merge_reference, reachability) =
                    local_details(&inspection, name, record, remote_tracking_deleted)?;
                (Some(local), Some(merge_reference), Some(reachability))
            }
            None => (None, None, None),
        };
    let local_deleted = local.as_ref().map(|local| local.name.as_str());

    let push_url = remote_target.as_ref().ok().map(|(_, url)| url.clone());
    let (mut remote, remote_reachability, remote_unavailable_reason) = match remote_target {
        Ok((candidate, url)) => {
            let (remote, reachability) =
                remote_details(&inspection, candidate, &url, local_deleted)?;
            (Some(remote), Some(reachability), None)
        }
        Err(reason) => (None, None, Some(reason)),
    };
    // Only a deletion that pushes asks the provider, which goes over the network.
    if let (true, Some(remote), Some(url)) = (request.delete_remote, remote.as_mut(), &push_url) {
        remote.pull_requests = Some(branch_pull_requests(worktree, url, &remote.remote_ref));
    }
    let open_pull_requests: Option<Vec<String>> = match remote
        .as_ref()
        .filter(|_| request.delete_remote)
        .and_then(|remote| remote.pull_requests.as_ref())
    {
        Some(BranchPullRequests::Checked { pulls, .. }) => {
            Some(pulls.iter().map(OpenPullRequest::key).collect())
        }
        _ => None,
    };
    let more_pull_requests = matches!(
        remote
            .as_ref()
            .filter(|_| request.delete_remote)
            .and_then(|remote| remote.pull_requests.as_ref()),
        Some(BranchPullRequests::Checked {
            more_than_listed: true,
            ..
        })
    );

    if let Some(local) = &local {
        if let Some(usage) = occupancy.get(&format!("refs/heads/{}", local.name)) {
            blockers.push(usage.blocker(&local.name));
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
                "{} on {} {} on no other ref, stash entry, or worktree HEAD. After deletion, {} can be recovered only by commit ID, which the restore command records, until Git prunes {}.",
                commit_count(local.exclusive_commit_count, local.exclusive_commit_count_capped),
                local.name,
                if local.exclusive_commit_count == 1 { "is" } else { "are" },
                if local.exclusive_commit_count == 1 { "it" } else { "they" },
                if local.exclusive_commit_count == 1 { "it" } else { "them" },
            ));
        }
    }
    if request.delete_remote {
        match &remote {
            Some(remote) => {
                // The local record can be stale either way; the remote itself
                // is asked right before deleting, and that answer decides.
                if remote.is_remote_default_branch {
                    warnings.push(format!(
                        "{} was the default branch of {} when it was last fetched. Repola asks {} before deleting and does not delete its default branch.",
                        remote.display_name, remote.remote, remote.remote
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
                        "{} on {} {} on no ref, stash entry, or worktree HEAD that remains in this repository.",
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
    let closes_pull_requests = affects_pull_requests(
        remote
            .as_ref()
            .filter(|_| request.delete_remote)
            .and_then(|remote| remote.pull_requests.as_ref()),
        reviewed_pull_requests,
    );
    let confirmation = if requires_force || remote_loses_commits || closes_pull_requests {
        BranchDeletionConfirmation::TypeBranchName
    } else {
        BranchDeletionConfirmation::Confirm
    };

    let local_commands = match &local {
        Some(local) => local_deletion_commands(local, has_branch_config(worktree, &local.name)?),
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
        pull_requests: open_pull_requests,
        more_pull_requests,
        push_destination: push_url
            .as_deref()
            .filter(|_| request.delete_remote)
            .map(digest),
        local_reachability,
        remote_reachability: remote_reachability.filter(|_| request.delete_remote),
        commands: commands.clone(),
        warnings: warnings.clone(),
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
        merge_reference,
        repository,
        push_url,
    })
}

pub fn execute_branch_deletion(
    request: BranchDeletionExecutionRequest,
) -> Result<BranchDeletionResult, String> {
    let reviewed = review(request.request, request.expected.pull_requests.as_deref())?;
    let plan = &reviewed.plan;
    if let Some(blocker) = plan.blockers.first() {
        return Err(format!("The branch deletion is blocked: {blocker}"));
    }
    // Pull requests come from the provider, which can fail to answer; only one
    // the review did not show refuses the deletion. It is checked first: a new
    // one also changes the confirmation the fingerprint records.
    if let Some(change) =
        pull_request_change(&request.expected, &plan.fingerprint, &plan.branch_name)
    {
        return Err(change);
    }
    let without_pull_requests =
        |fingerprint: &BranchDeletionFingerprint| BranchDeletionFingerprint {
            pull_requests: None,
            more_pull_requests: false,
            ..fingerprint.clone()
        };
    if without_pull_requests(&plan.fingerprint) != without_pull_requests(&request.expected)
        || plan.requires_force != request.force
    {
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
        let url = reviewed
            .push_url
            .as_deref()
            .ok_or("The URL the remote deletion pushes to could not be identified.")?;
        confirm_not_remote_default(worktree, remote, url)?;
    }

    let local = match &plan.local {
        Some(local) => Some(delete_local(&reviewed, local)?),
        None => None,
    };

    // A local deletion that may not have happened stops before the remote.
    let local_unconfirmed = local.as_ref().is_some_and(|local| local.unconfirmed);
    let mut already_gone = false;
    let remote = match selected_remote.filter(|_| !local_unconfirmed) {
        Some(remote) => {
            let deletion = delete_remote(&reviewed, remote);
            // Recreates the branch only while the remote has none, so it can
            // never move a branch someone made since.
            let lease = format!("--force-with-lease={}:", remote.remote_ref);
            let refspec = format!("{}:{}", remote.expected_oid, remote.remote_ref);
            let push_back = || {
                vec![git_command_line(
                    worktree,
                    [
                        "push",
                        lease.as_str(),
                        "--",
                        remote.remote.as_str(),
                        &refspec,
                    ],
                )]
            };
            let unconfirmed = matches!(deletion.outcome, RemoteOutcome::Unconfirmed);
            let (succeeded, warning, recovery_commands) = match &deletion.outcome {
                RemoteOutcome::Deleted => (true, None, push_back()),
                // Restoring a branch that still exists is refused, so it is safe to offer.
                RemoteOutcome::Unconfirmed => (false, None, push_back()),
                RemoteOutcome::Gone {
                    already: false,
                    tracking_kept,
                } => (
                    true,
                    tracking_kept.as_ref().map(|reason| {
                        format!(
                            "{} kept its remote-tracking ref, which no longer matches the review ({reason}). Fetch to update it.",
                            remote.display_name
                        )
                    }),
                    push_back(),
                ),
                RemoteOutcome::Gone {
                    already: true,
                    tracking_kept,
                } => {
                    already_gone = true;
                    match tracking_kept {
                        None => (
                            true,
                            None,
                            vec![git_command_line(
                                worktree,
                                [
                                    "update-ref",
                                    remote.tracking_ref.as_str(),
                                    remote.expected_oid.as_str(),
                                ],
                            )],
                        ),
                        Some(reason) => (
                            true,
                            Some(format!(
                                "{} kept its remote-tracking ref, which no longer matches the review ({reason}). Fetch to update it.",
                                remote.display_name
                            )),
                            Vec::new(),
                        ),
                    }
                }
                RemoteOutcome::Failed => (false, None, Vec::new()),
            };
            if !succeeded && !unconfirmed && local.is_none() {
                return Err(if deletion.output.is_empty() {
                    format!(
                        "Git did not delete {} and returned no diagnostic output.",
                        remote.display_name
                    )
                } else {
                    deletion.output
                });
            }
            Some(BranchDeletionStep {
                target: remote.display_name.clone(),
                deleted_oid: remote.expected_oid.clone(),
                succeeded,
                unconfirmed,
                output: deletion.output,
                warning,
                finish_commands: Vec::new(),
                recovery_commands,
            })
        }
        None => None,
    };

    let local_outcome = |local: &BranchDeletionStep| {
        if local.unconfirmed {
            format!(
                "Repola could not confirm whether local branch {} was deleted.",
                local.target
            )
        } else {
            format!("Deleted local branch {}.", local.target)
        }
    };
    let remote_outcome = |remote: &BranchDeletionStep| {
        if remote.unconfirmed {
            format!(
                "Repola could not confirm whether remote branch {} was deleted.",
                remote.target
            )
        } else if already_gone {
            format!("Remote branch {} was already deleted.", remote.target)
        } else if remote.succeeded {
            format!("Deleted remote branch {}.", remote.target)
        } else {
            format!("Remote branch {} was not deleted.", remote.target)
        }
    };
    let skipped = selected_remote
        .filter(|_| local_unconfirmed)
        .map(|remote| format!("Remote branch {} was not deleted.", remote.display_name));
    let message = match (&local, &remote) {
        (Some(local), Some(remote)) if local.succeeded && remote.succeeded && !already_gone => {
            format!(
                "Deleted local branch {} and remote branch {}.",
                local.target, remote.target
            )
        }
        _ => local
            .as_ref()
            .map(local_outcome)
            .into_iter()
            .chain(remote.as_ref().map(remote_outcome))
            .chain(skipped)
            .reduce(|message, next| format!("{message} {next}"))
            .unwrap_or_else(|| "No branch was deleted.".into()),
    };
    Ok(BranchDeletionResult {
        message,
        local,
        remote,
        audit_path: None,
        audit_warning: None,
    })
}

/// Whether deleting the remote branch may close open pull requests: some are
/// listed now or were in the review, or the provider has more than it listed.
fn affects_pull_requests(
    current: Option<&BranchPullRequests>,
    reviewed: Option<&[String]>,
) -> bool {
    let listed_now = match current {
        Some(BranchPullRequests::Checked {
            pulls,
            more_than_listed,
            ..
        }) => *more_than_listed || !pulls.is_empty(),
        _ => false,
    };
    listed_now || reviewed.is_some_and(|reviewed| !reviewed.is_empty())
}

/// Why open pull requests found now refuse a deletion reviewed with
/// `reviewed`: one the review did not show, or more than the provider lists
/// where the review had them all.
fn pull_request_change(
    reviewed: &BranchDeletionFingerprint,
    current: &BranchDeletionFingerprint,
    branch: &str,
) -> Option<String> {
    let unreviewed = unreviewed_pull_requests(
        reviewed.pull_requests.as_deref(),
        current.pull_requests.as_deref(),
    );
    if let [single] = unreviewed.as_slice() {
        return Some(format!(
            "Pull request {single} now uses {branch}, and the review did not show it. Review the deletion again."
        ));
    }
    if !unreviewed.is_empty() {
        return Some(format!(
            "Pull requests {} now use {branch}, and the review did not show them. Review the deletion again.",
            unreviewed.join(", ")
        ));
    }
    (current.more_pull_requests && !reviewed.more_pull_requests).then(|| {
        format!(
            "More open pull requests now use {branch} than Repola can list, and the review did not show that. Review the deletion again."
        )
    })
}

/// The open pull requests found now that the review did not show. The review
/// showed none when the provider could not be asked; a provider that cannot be
/// asked now finds none.
fn unreviewed_pull_requests(
    reviewed: Option<&[String]>,
    current: Option<&[String]>,
) -> Vec<String> {
    let reviewed = reviewed.unwrap_or_default();
    current
        .unwrap_or_default()
        .iter()
        .filter(|key| !reviewed.contains(key))
        .cloned()
        .collect()
}

/// How the remote deletion ended, with Git's output and what Repola found.
struct RemoteDeletion {
    outcome: RemoteOutcome,
    output: String,
}

enum RemoteOutcome {
    Deleted,
    /// The push was interrupted after it started, so the remote may have
    /// deleted the branch.
    Unconfirmed,
    /// The remote no longer has the branch, but Git did not report deleting
    /// it: it was `already` gone, or the push lost the server's report. Its
    /// remote-tracking ref was removed as a deleting push removes it, unless it
    /// had changed, in which case this says why it was kept.
    Gone {
        already: bool,
        tracking_kept: Option<String>,
    },
    Failed,
}

/// Pushes the reviewed remote deletion. `git push` resolves the remote's push
/// URL again when it runs, so the URL is compared with the reviewed one right
/// before, keeping the window in which a changed URL could go unnoticed as short
/// as the push itself allows.
///
/// The lease rejects the push in the same way whether the branch moved or is
/// gone, so a rejection is followed by asking the remote which it was.
fn delete_remote(reviewed: &Review, remote: &RemoteBranchDeletion) -> RemoteDeletion {
    let worktree = Path::new(&reviewed.plan.worktree_path);
    let failed = |output| RemoteDeletion {
        outcome: RemoteOutcome::Failed,
        output,
    };
    let Some(url) = reviewed.push_url.as_deref() else {
        return failed(format!(
            "The URL {} pushes to could not be identified, so {} was not deleted.",
            remote.remote, remote.display_name
        ));
    };
    match single_push_url(worktree, &remote.remote) {
        Ok(current) if current == url => {}
        Ok(_) => {
            return failed(format!(
                "{} no longer pushes to {}, so {} was not deleted. Review the deletion again.",
                remote.remote, remote.push_url, remote.display_name
            ))
        }
        Err(reason) => return failed(format!("{reason} {} was not deleted.", remote.display_name)),
    }
    let reviewed_loss = reviewed.plan.fingerprint.remote_reachability.as_deref();
    match loss_unchanged(reviewed, &remote.expected_oid, reviewed_loss) {
        Ok(true) => {}
        Ok(false) => {
            return failed(format!(
                "The commits deleting {} would leave unreachable in this repository changed after this deletion was reviewed, so it was not deleted. Review the deletion again.",
                remote.display_name
            ))
        }
        Err(reason) => return failed(format!("{reason} {} was not deleted.", remote.display_name)),
    }
    let output = match command::git_at(worktree, remote_deletion_args(remote)) {
        Ok(output) => output,
        Err(error) if may_have_run(&error) => {
            return RemoteDeletion {
                outcome: RemoteOutcome::Unconfirmed,
                output: format!(
                    "{error}. The push had started, so {} may have deleted {}.",
                    remote.remote, remote.remote_ref
                ),
            }
        }
        Err(error) => return failed(error.to_string()),
    };
    let diagnostic = combined_output(&output.stdout, &output.stderr);
    // Git exits successfully only once the remote deleted the branch.
    if output.status.success() {
        return RemoteDeletion {
            outcome: RemoteOutcome::Deleted,
            output: diagnostic,
        };
    }
    let report = String::from_utf8_lossy(&output.stdout);
    match reported_deletion(&report, &remote.remote_ref) {
        // The lease rejects a branch that moved and one that is gone alike.
        Some(false) if report.contains("(stale info)") => {}
        Some(false) => return failed(diagnostic),
        // No status for the branch, or a deletion line from a push that still
        // failed, which a pre-push hook can print: the push may have failed
        // before sending anything, or lost the server's report after it
        // deleted the branch.
        Some(true) | None => {
            return match remote_has(worktree, &remote.remote, &remote.remote_ref) {
                Ok(true) => failed(diagnostic),
                Ok(false) => RemoteDeletion {
                    outcome: RemoteOutcome::Gone {
                        already: false,
                        tracking_kept: remove_tracking_ref(worktree, remote),
                    },
                    output: format!(
                        "{diagnostic}\nGit did not report the deletion, but {} no longer has {}.",
                        remote.remote, remote.remote_ref
                    ),
                },
                Err(reason) => RemoteDeletion {
                    outcome: RemoteOutcome::Unconfirmed,
                    output: format!(
                        "{diagnostic}\nGit did not report whether {} deleted {}, and asking it failed ({reason}).",
                        remote.remote, remote.remote_ref
                    ),
                },
            };
        }
    }
    match remote_has(worktree, &remote.remote, &remote.remote_ref) {
        Ok(false) => RemoteDeletion {
            outcome: RemoteOutcome::Gone {
                already: true,
                tracking_kept: remove_tracking_ref(worktree, remote),
            },
            output: format!(
                "{} no longer has {}; someone deleted it after your last fetch.",
                remote.remote, remote.remote_ref
            ),
        },
        Ok(true) => failed(match remote_tip(worktree, url, &remote.remote_ref) {
            Ok(Some(tip)) => format!(
                "{diagnostic}\n{} on {} is now at {tip}, not the reviewed {}. Fetch, then review again.",
                remote.remote_ref, remote.remote, remote.expected_oid
            ),
            _ => format!(
                "{diagnostic}\n{} on {} moved from the reviewed {}. Fetch, then review again.",
                remote.remote_ref, remote.remote, remote.expected_oid
            ),
        }),
        Err(reason) => failed(format!(
            "{diagnostic}\n{} on {} no longer matches the reviewed commit {}, or it no longer exists, and asking the remote failed ({reason}). Fetch, then review again.",
            remote.remote_ref, remote.remote, remote.expected_oid
        )),
    }
}

/// Whether the deletion still leaves unreachable exactly the commits from `tip`
/// the review found, whose evidence is `reviewed_loss`. Every ref the whole
/// deletion removes counts as removed, as in the review.
fn loss_unchanged(
    reviewed: &Review,
    tip: &str,
    reviewed_loss: Option<&str>,
) -> Result<bool, String> {
    let plan = &reviewed.plan;
    let local_ref = plan
        .local
        .as_ref()
        .map(|local| format!("refs/heads/{}", local.name));
    let tracking_ref = plan
        .remote
        .as_ref()
        .filter(|_| plan.delete_remote)
        .map(|remote| remote.tracking_ref.as_str());
    let removed: Vec<&str> = local_ref
        .as_deref()
        .into_iter()
        .chain(tracking_ref)
        .collect();
    let reachability =
        Reachability::read(Path::new(&plan.worktree_path), &reviewed.repository.path)?;
    let loss = exclusive_commits(&reachability, tip, &removed)?;
    Ok(reviewed_loss == Some(loss.evidence.as_str()))
}

/// What `git push --porcelain` reported for deleting `reference`: deleted,
/// rejected, or `None` when its report has no status for the branch.
fn reported_deletion(report: &str, reference: &str) -> Option<bool> {
    let deletion = format!(":{reference}");
    report.lines().find_map(|line| {
        let mut fields = line.split('\t');
        let (flag, refspec) = (fields.next()?, fields.next()?);
        if !refspec.ends_with(&deletion) {
            return None;
        }
        match flag {
            "-" => Some(true),
            "!" => Some(false),
            _ => None,
        }
    })
}

/// Removes the remote-tracking ref a deleted remote branch leaves behind, as a
/// deleting push does, only while it is still at the reviewed commit. Returns
/// why it was kept, if it was.
fn remove_tracking_ref(worktree: &Path, remote: &RemoteBranchDeletion) -> Option<String> {
    let tracking = command::git_at(
        worktree,
        [
            "update-ref",
            "--no-deref",
            "-d",
            remote.tracking_ref.as_str(),
            remote.expected_oid.as_str(),
        ],
    );
    match tracking {
        Ok(output) if output.status.success() => None,
        Ok(output) => Some(combined_output(&output.stdout, &output.stderr)),
        Err(error) => Some(error.to_string()),
    }
}

/// Whether `remote` has `reference`, as a push sees it: a dry run of the
/// deletion under a lease that expects the ref to be absent succeeds only when
/// it is. A server can hide a ref from fetching but not from pushing, so
/// `ls-remote` cannot answer this. The dry run goes through the remote's name,
/// which resolves its URL exactly as the deletion's push did; the URL itself
/// could be rewritten by `pushInsteadOf`. It changes nothing, so it skips the
/// pre-push hook.
fn remote_has(worktree: &Path, remote: &str, reference: &str) -> Result<bool, String> {
    let lease = format!("--force-with-lease={reference}:");
    let deletion = format!(":{reference}");
    let output = command::git_at(
        worktree,
        [
            "push",
            "--dry-run",
            "--porcelain",
            "--no-verify",
            lease.as_str(),
            "--",
            remote,
            deletion.as_str(),
        ],
    )
    .map_err(|error| error.to_string())?;
    if output.status.success() {
        return Ok(false);
    }
    if String::from_utf8_lossy(&output.stdout).contains("(stale info)") {
        return Ok(true);
    }
    Err(combined_output(&output.stdout, &output.stderr))
}

/// Whether a command failed after it may already have done its work: stopped
/// by cancellation or its deadline, or finished with more output than the
/// command layer keeps.
fn may_have_run(error: &command::CommandError) -> bool {
    matches!(
        error,
        command::CommandError::Cancelled { .. }
            | command::CommandError::Timeout { .. }
            | command::CommandError::OutputTooLarge { .. }
    )
}

/// The commit `reference` points to on the remote at `url`, as fetching sees it,
/// or `None` when it is not shown.
fn remote_tip(worktree: &Path, url: &str, reference: &str) -> Result<Option<String>, String> {
    let output = command::git_at(worktree, ["ls-remote", "--refs", "--", url, reference])
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(combined_output(&output.stdout, &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .find(|(_, name)| *name == reference)
        .map(|(oid, _)| oid.to_string()))
}

/// Runs the reviewed local deletion. Only the first command deletes the branch;
/// a failure after it is reported with the completed step, never as a failure.
///
/// `update-ref` deletes the branch only at the reviewed tip, but checks nothing
/// else `git branch` would, and the review can be seconds old by now, after
/// asking the remote. So right before deleting, the branch must still be a
/// regular ref that no worktree uses, it must leave unreachable exactly the
/// commits the review found, and, unless the deletion is forced, the reference
/// the review found the branch contained in must not have moved.
fn delete_local(
    reviewed: &Review,
    local: &LocalBranchDeletion,
) -> Result<BranchDeletionStep, String> {
    let Review {
        plan,
        local_commands,
        merge_reference,
        repository,
        ..
    } = reviewed;
    let worktree = Path::new(&plan.worktree_path);
    let (deletion, cleanup) = local_commands
        .split_first()
        .ok_or_else(|| format!("No command was reviewed to delete {}.", local.name))?;
    let reference = format!("refs/heads/{}", local.name);
    if let Some(usage) = branches_in_use(repository)?.get(&reference) {
        return Err(format!(
            "Nothing was deleted. {}",
            usage.blocker(&local.name)
        ));
    }
    if let Some(target) = symbolic_target(worktree, &reference)? {
        return Err(format!(
            "Nothing was deleted. {} became a symbolic ref to {} after this deletion was reviewed.",
            local.name,
            short_ref(&target)
        ));
    }
    if let Some(merge_reference) = merge_reference.as_deref().filter(|_| !plan.requires_force) {
        if resolve(worktree, merge_reference)? != local.merge_reference_oid {
            return Err(format!(
                "Nothing was deleted. {} moved after this deletion was reviewed, so {} may no longer be contained in it. Review the deletion again.",
                local.merge_reference, local.name
            ));
        }
    }
    if !loss_unchanged(
        reviewed,
        &local.tip,
        plan.fingerprint.local_reachability.as_deref(),
    )? {
        return Err(format!(
            "Nothing was deleted. The commits deleting {} would leave unreachable changed after this deletion was reviewed. Review the deletion again.",
            local.name
        ));
    }
    // Read before the cleanup removes it, so the recovery can restore it.
    let configuration = branch_configuration(worktree, &local.name)?;

    let recovery = git_command_line(
        worktree,
        ["branch", "--", local.name.as_str(), local.tip.as_str()],
    );
    let output = match command::git_at(worktree, deletion) {
        Ok(output) => output,
        // Restoring a branch that still exists is refused, so it is safe to offer.
        Err(error) if may_have_run(&error) => {
            return Ok(BranchDeletionStep {
                target: local.name.clone(),
                deleted_oid: local.tip.clone(),
                succeeded: false,
                unconfirmed: true,
                output: format!(
                    "{error}. The deletion had started, so {} may have been deleted, and its configuration was kept.",
                    local.name
                ),
                warning: None,
                finish_commands: Vec::new(),
                recovery_commands: vec![recovery],
            });
        }
        Err(error) => return Err(error.to_string()),
    };
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
    let mut warnings = Vec::new();
    let mut finish_commands = Vec::new();
    for args in cleanup {
        let failure = match command::git_at(worktree, args) {
            Ok(output) if output.status.success() => continue,
            Ok(output) => combined_output(&output.stdout, &output.stderr),
            Err(error) => error.to_string(),
        };
        warnings.push(format!(
            "{} was deleted, but its configuration was not removed ({}). A new branch with this name would inherit it.",
            local.name,
            failure.trim()
        ));
        finish_commands.push(git_command_line(worktree, args));
    }
    let configuration_removed = finish_commands.is_empty();

    let mut recovery_commands = vec![recovery];
    if configuration_removed {
        recovery_commands.extend(configuration.iter().map(|(key, value)| {
            git_command_line(
                worktree,
                ["config", "--local", "--add", key.as_str(), value.as_str()],
            )
        }));
    }
    Ok(BranchDeletionStep {
        target: local.name.clone(),
        deleted_oid: local.tip.clone(),
        succeeded: true,
        unconfirmed: false,
        output: diagnostic,
        warning: (!warnings.is_empty()).then(|| warnings.join(" ")),
        finish_commands,
        recovery_commands,
    })
}

/// The ref `reference` points to when it is symbolic.
fn symbolic_target(worktree: &Path, reference: &str) -> Result<Option<String>, String> {
    let output = command::successful_git_at(
        worktree,
        ["for-each-ref", "--format=%(refname)%00%(symref)", reference],
    )
    .map_err(|error| error.to_string())?;
    Ok(output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter_map(|line| {
            let line = String::from_utf8_lossy(line);
            let (name, target) = line.split_once('\0')?;
            (name == reference && !target.is_empty()).then(|| target.to_string())
        })
        .next())
}

/// The `branch.<name>.*` entries in the repository configuration, in file order,
/// which removing the `branch.<name>` section deletes.
fn branch_configuration(worktree: &Path, name: &str) -> Result<Vec<(String, String)>, String> {
    let output = command::successful_git_at(worktree, ["config", "--local", "--list", "-z"])
        .map_err(|error| error.to_string())?;
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| {
            let entry = String::from_utf8_lossy(entry);
            // A key without a value is listed with no newline. Git reads it as
            // true, and its command line cannot write a key without a value.
            let (key, value) = entry.split_once('\n').unwrap_or((&entry, "true"));
            is_branch_key(key, name).then(|| (key.to_string(), value.to_string()))
        })
        .collect())
}

/// Whether `key`, as `git config --list` prints it, is a variable in the
/// `branch.<name>` section. Git lowercases the section and variable names but
/// keeps the branch name, and a variable name has no dots.
fn is_branch_key(key: &str, name: &str) -> bool {
    key.strip_prefix("branch.")
        .and_then(|rest| rest.strip_prefix(name))
        .and_then(|rest| rest.strip_prefix('.'))
        .is_some_and(|variable| !variable.is_empty() && !variable.contains('.'))
}

/// The exact local deletion commands, both run and displayed: delete the branch
/// only at its reviewed tip, then remove its configuration as `git branch`
/// does. [`delete_local`] makes the checks `git branch` would make first.
fn local_deletion_commands(local: &LocalBranchDeletion, has_config: bool) -> Vec<Vec<String>> {
    let mut commands = vec![vec![
        "update-ref".into(),
        "--no-deref".into(),
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

/// Branches Git refuses to delete because a worktree uses them, found the way
/// `git branch -d` finds them: the branch each worktree's HEAD names, the branch
/// a rebase or bisect in progress there will return to, and the branches a
/// `rebase --update-refs` there will move. `git worktree list` reports a
/// worktree in the middle of a rebase or bisect as detached, so those states
/// are read from each worktree's administrative directory, as Git reads them.
fn branches_in_use(repository: &RepositoryContext) -> Result<BTreeMap<String, BranchUse>, String> {
    let seeds = list_worktrees(&repository.path)?;
    let mut uses = BTreeMap::new();
    for seed in seeds.iter().filter(|seed| !seed.bare) {
        if let Some(branch) = &seed.branch {
            uses.insert(
                format!("refs/heads/{branch}"),
                BranchUse {
                    worktree: seed.path.clone(),
                    activity: Activity::CheckedOut,
                },
            );
        }
    }

    let mut administrative = Vec::new();
    if let Some(primary) = seeds.iter().find(|seed| seed.is_primary && !seed.bare) {
        administrative.push((repository.git_dir.clone(), primary.path.clone()));
    }
    let linked = repository.git_dir.join("worktrees");
    match std::fs::read_dir(&linked) {
        Ok(entries) => {
            for entry in entries {
                let directory = entry.map_err(|error| error.to_string())?.path();
                if let Some(worktree) = linked_worktree_path(&directory)? {
                    administrative.push((directory, worktree));
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!("Could not read {}: {error}", linked.display()));
        }
    }

    for (directory, worktree) in administrative {
        for (reference, activity) in branches_in_progress(&directory)? {
            uses.entry(reference).or_insert_with(|| BranchUse {
                worktree: worktree.clone(),
                activity,
            });
        }
    }
    Ok(uses)
}

/// The worktree a linked worktree's administrative directory belongs to, read
/// from its `gitdir` file as Git reads it, or `None` when Git would skip it.
fn linked_worktree_path(directory: &Path) -> Result<Option<String>, String> {
    let gitdir = match std::fs::read_to_string(directory.join("gitdir")) {
        Ok(gitdir) => gitdir,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(None)
        }
        Err(error) => {
            return Err(format!(
                "Could not read {}: {error}",
                directory.join("gitdir").display()
            ))
        }
    };
    let gitdir = gitdir.trim_end();
    if gitdir.is_empty() {
        return Ok(None);
    }
    let worktree = gitdir.strip_suffix("/.git").unwrap_or(gitdir);
    let worktree = Path::new(worktree);
    // `worktree.useRelativePaths` records the path relative to this directory.
    let worktree = if worktree.is_absolute() {
        worktree.to_path_buf()
    } else {
        let joined = directory.join(worktree);
        dunce::canonicalize(&joined).unwrap_or(joined)
    };
    Ok(Some(native_path(&worktree.to_string_lossy())))
}

/// The branches, as full ref names, that a rebase or bisect recorded in a
/// worktree's administrative directory will return to or move. Mirrors Git's
/// `wt_status_check_rebase`, `wt_status_check_bisect`, and
/// `sequencer_get_update_refs_state`.
fn branches_in_progress(directory: &Path) -> Result<Vec<(String, Activity)>, String> {
    let mut found = Vec::new();
    let rebase_apply = directory.join("rebase-apply");
    let rebase_merge = directory.join("rebase-merge");
    // `git am` also uses rebase-apply, and marks it as applying.
    let head_name = if rebase_apply.exists() {
        (!rebase_apply.join("applying").exists()).then(|| rebase_apply.join("head-name"))
    } else if rebase_merge.exists() {
        Some(rebase_merge.join("head-name"))
    } else {
        None
    };
    if let Some(head_name) = head_name {
        if let Some(reference) = read_state(&head_name)?.as_deref().and_then(state_branch) {
            found.push((reference, Activity::Rebasing));
        }
    }
    if directory.join("BISECT_LOG").exists() {
        if let Some(reference) = read_state(&directory.join("BISECT_START"))?
            .as_deref()
            .and_then(state_branch)
        {
            found.push((reference, Activity::Bisecting));
        }
    }
    if let Some(records) = read_state(&rebase_merge.join("update-refs"))? {
        found.extend(
            parse_update_refs(&records)
                .into_iter()
                .map(|reference| (reference, Activity::Rebasing)),
        );
    }
    Ok(found)
}

/// The branch a rebase's `head-name` or bisect's `BISECT_START` names, as a full
/// ref name, read as Git's `get_branch` reads it: a full branch ref, or a short
/// name that Git puts under `refs/heads/`. An empty file or `detached HEAD`
/// names no branch, and neither does an object ID, which a rebase or bisect of
/// a detached HEAD records; Git abbreviates it and looks for a branch with that
/// name, but no branch is in use there.
fn state_branch(contents: &str) -> Option<String> {
    let contents = contents.trim_end_matches('\n');
    if contents.is_empty() || contents == "detached HEAD" || is_object_id(contents) {
        return None;
    }
    Some(if contents.starts_with("refs/heads/") {
        contents.to_string()
    } else {
        format!("refs/heads/{contents}")
    })
}

/// The refs a `rebase --update-refs` will move: each record is a ref name and
/// two object IDs on their own lines. Like Git, a malformed file names none.
fn parse_update_refs(contents: &str) -> Vec<String> {
    let mut references = Vec::new();
    let mut lines = contents.lines();
    while let Some(reference) = lines.next() {
        let (Some(before), Some(after)) = (lines.next(), lines.next()) else {
            return Vec::new();
        };
        if !is_object_id(before) || !is_object_id(after) {
            return Vec::new();
        }
        references.push(reference.to_string());
    }
    references
}

fn is_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn read_state(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(format!("Could not read {}: {error}", path.display())),
    }
}

/// Whether the repository configuration has a `branch.<name>` section.
fn has_branch_config(worktree: &Path, name: &str) -> Result<bool, String> {
    let output = command::successful_git_at(
        worktree,
        ["config", "--local", "--list", "--name-only", "-z"],
    )
    .map_err(|error| error.to_string())?;
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .any(|key| is_branch_key(&String::from_utf8_lossy(key), name)))
}

/// The one URL a deletion from `remote` pushes to, which can differ from the URL
/// it fetches from. A remote with several push URLs is refused: Git pushes to
/// each in turn, so the branch could be deleted from some while others refuse.
fn single_push_url(worktree: &Path, remote: &str) -> Result<String, String> {
    // Either one makes the push run a program of the remote's choosing, which
    // can act on a different repository than the URL Repola asks about.
    for setting in ["receivepack", "vcs"] {
        let key = format!("remote.{remote}.{setting}");
        let configured = command::git_at(worktree, ["config", "--get", key.as_str()])
            .map_err(|error| error.to_string())?;
        if configured.status.success() {
            return Err(format!(
                "{remote} sets {key}, so its push runs a program Repola cannot check, and Repola does not delete branches from it."
            ));
        }
    }
    let output = command::git_at(
        worktree,
        ["remote", "get-url", "--push", "--all", "--", remote],
    )
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Could not find where {remote} pushes. {}",
            combined_output(&output.stdout, &output.stderr)
        )
        .trim()
        .to_string());
    }
    let urls = String::from_utf8_lossy(&output.stdout);
    match urls
        .lines()
        .filter(|url| !url.is_empty())
        .collect::<Vec<_>>()
        .as_slice()
    {
        [url] => Ok((*url).to_string()),
        [] => Err(format!("{remote} has no URL to push to.")),
        urls => Err(format!(
            "{remote} pushes to {} URLs, and Git deletes the branch from each one separately, so some could lose it while others refuse. Delete it from each URL with Git instead.",
            urls.len()
        )),
    }
}

/// Asks the URL the remote deletion pushes to which branch its HEAD names,
/// because the local `refs/remotes/<remote>/HEAD` can be missing or stale.
/// Refuses when it cannot answer, answers without naming a branch, or does not
/// show its HEAD at all: a server can hide HEAD from fetching while it still
/// names the branch a push would delete.
///
/// `git push` has already applied any URL rewriting to the push URL, but
/// `ls-remote` rewrites the URL it is given with `url.<base>.insteadOf` again,
/// so a URL that would be rewritten a second time is refused rather than asked
/// on another server's behalf.
///
/// This is a check, not a lock: a push cannot be made conditional on the
/// remote's HEAD, so only the server can make this protection atomic.
fn confirm_not_remote_default(
    worktree: &Path,
    remote: &RemoteBranchDeletion,
    url: &str,
) -> Result<(), String> {
    let asked = command::successful_git_at(worktree, ["ls-remote", "--get-url", "--", url])
        .map_err(|error| error.to_string())?;
    if String::from_utf8_lossy(&asked.stdout).trim_end_matches(['\r', '\n']) != url {
        return Err(format!(
            "{} pushes to {}, which URL rewriting would change before Repola could ask it for its default branch, so nothing was deleted.",
            remote.remote, remote.push_url
        ));
    }
    let output = command::git_at(worktree, ["ls-remote", "--symref", "--", url, "HEAD"])
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Could not ask {} at {} which branch is its default, so nothing was deleted. {}",
            remote.remote,
            remote.push_url,
            combined_output(&output.stdout, &output.stderr)
        )
        .trim()
        .to_string());
    }
    match parse_remote_head(&String::from_utf8_lossy(&output.stdout)) {
        RemoteHead::Branch(target) if target == remote.remote_ref => Err(format!(
            "{} is the default branch of {} at {}. Repola does not delete a remote's default branch.",
            remote.display_name, remote.remote, remote.push_url
        )),
        RemoteHead::Branch(_) => Ok(()),
        RemoteHead::Unnamed => Err(format!(
            "{} at {} did not say which branch is its default, so nothing was deleted.",
            remote.remote, remote.push_url
        )),
        RemoteHead::Absent => Err(format!(
            "{} at {} did not show which branch is its default, so nothing was deleted.",
            remote.remote, remote.push_url
        )),
    }
}

#[derive(Debug, PartialEq)]
enum RemoteHead {
    /// HEAD is a symbolic ref to this branch.
    Branch(String),
    /// HEAD exists, but the remote did not report what it points to.
    Unnamed,
    /// The remote showed no HEAD: it has none, or hides it.
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

const REF_FORMAT: &str = "--format=%(refname)%00%(objectname)%00%(symref)%00%(upstream)%00%(upstream:remotename)%00%(upstream:remoteref)";

fn read_refs(worktree: &Path) -> Result<BTreeMap<String, RefRecord>, String> {
    let output = command::successful_git_at(
        worktree,
        ["for-each-ref", REF_FORMAT, "refs/heads", "refs/remotes"],
    )
    .map_err(|error| error.to_string())?;
    parse_refs(&output.stdout)
}

/// The one ref named `name`, which may live anywhere under `refs/`.
fn read_ref(worktree: &Path, name: &str) -> Result<Option<RefRecord>, String> {
    let output = command::successful_git_at(worktree, ["for-each-ref", REF_FORMAT, name])
        .map_err(|error| error.to_string())?;
    Ok(parse_refs(&output.stdout)?.remove(name))
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
        let [name, oid, symref, upstream, upstream_remote, upstream_remote_ref] =
            <[String; 6]>::try_from(fields)
                .map_err(|_| "Git returned malformed branch metadata.".to_string())?;
        refs.insert(
            name,
            RefRecord {
                oid,
                symref,
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
) -> Result<(LocalBranchDeletion, String, String), String> {
    let Inspection {
        worktree,
        refs,
        snapshot,
        ..
    } = *inspection;
    // `git branch -d` checks the upstream when it resolves, otherwise HEAD of the
    // worktree that runs it, which is the selected worktree.
    let (merge_reference_ref, merge_reference_kind, merge_reference, merge_reference_oid) =
        match refs
            .get(&record.upstream)
            .filter(|_| !record.upstream.is_empty())
        {
            Some(upstream) => (
                record.upstream.clone(),
                MergeReferenceKind::Upstream,
                short_ref(&record.upstream),
                Some(upstream.oid.clone()),
            ),
            None => (
                "HEAD".to_string(),
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
    let exclusive = exclusive_commits(inspection.reachability, &record.oid, &excluded)?;
    let local = LocalBranchDeletion {
        name: name.to_string(),
        tip: record.oid.clone(),
        merge_reference,
        merge_reference_kind,
        merge_reference_oid,
        contained_in_merge_reference,
        default_target: default_target.map(|(target, _)| short_ref(target)),
        contained_in_default_target,
        occupied_worktree_path: inspection
            .occupancy
            .get(&local_ref)
            .map(|usage| usage.worktree.clone()),
        is_default_branch: default_target
            .and_then(|(target, _)| target.strip_prefix("refs/remotes/origin/"))
            == Some(name),
        upstream: (!record.upstream.is_empty()).then(|| short_ref(&record.upstream)),
        exclusive_commit_count: exclusive.count,
        exclusive_commit_count_capped: exclusive.capped,
    };
    Ok((local, merge_reference_ref, exclusive.evidence))
}

fn remote_details(
    inspection: &Inspection,
    candidate: RemoteCandidate,
    push_url: &str,
    local_deleted: Option<&str>,
) -> Result<(RemoteBranchDeletion, String), String> {
    let worktree = inspection.worktree;
    let local_ref = local_deleted.map(|name| format!("refs/heads/{name}"));
    let mut excluded = vec![candidate.tracking_ref.as_str()];
    excluded.extend(local_ref.as_deref());
    let exclusive = exclusive_commits(inspection.reachability, &candidate.oid, &excluded)?;
    let tracked_by = inspection
        .refs
        .iter()
        .filter(|(_, record)| record.upstream == candidate.tracking_ref)
        .filter_map(|(reference, _)| {
            reference
                .strip_prefix("refs/heads/")
                .map(|name| (reference, name))
        })
        .filter(|(_, name)| Some(*name) != local_deleted)
        .map(
            |(reference, name)| match inspection.occupancy.get(reference) {
                Some(usage) => format!("{name} ({} at {})", usage.activity(), usage.worktree),
                None => name.to_string(),
            },
        )
        .collect();
    let symbolic_head = format!("refs/remotes/{}/HEAD", candidate.remote);
    let remote_head = command::git_at(worktree, ["symbolic-ref", "-q", symbolic_head.as_str()])
        .map_err(|error| error.to_string())?;
    let is_remote_default_branch = remote_head.status.success()
        && String::from_utf8_lossy(&remote_head.stdout).trim() == candidate.tracking_ref;
    let remote = RemoteBranchDeletion {
        display_name: short_ref(&candidate.tracking_ref),
        tracking_ref_updated_at: tracking_ref_updated_at(worktree, &candidate.tracking_ref)?,
        last_fetched_at: last_fetched_at(inspection.git_dir),
        is_remote_default_branch,
        tracked_by,
        exclusive_commit_count: exclusive.count,
        exclusive_commit_count_capped: exclusive.capped,
        expected_oid: candidate.oid,
        push_url: diagnostics::redact(push_url),
        pull_requests: None,
        remote: candidate.remote,
        remote_ref: candidate.remote_ref,
        tracking_ref: candidate.tracking_ref,
    };
    Ok((remote, exclusive.evidence))
}

/// The commit `reference` points to now, or `None` when it does not resolve.
fn resolve(worktree: &Path, reference: &str) -> Result<Option<String>, String> {
    let revision = format!("{reference}^{{commit}}");
    let output = command::git_at(
        worktree,
        [
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            revision.as_str(),
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string()))
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

fn digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// What, besides the refs `git rev-list --all` reads, decides which commits a
/// deletion leaves unreachable.
struct Reachability<'a> {
    worktree: &'a Path,
    repository_path: &'a Path,
    /// Every stash entry, which keeps its commits reachable like a ref does.
    stashes: Vec<String>,
    /// Every symbolic ref, by name, and the ref it points to.
    symbolic_refs: BTreeMap<String, String>,
}

impl<'a> Reachability<'a> {
    fn read(worktree: &'a Path, repository_path: &'a Path) -> Result<Self, String> {
        Ok(Self {
            worktree,
            repository_path,
            stashes: stash_entries(worktree)?,
            symbolic_refs: symbolic_refs(worktree)?,
        })
    }
}

/// The commits a deletion would leave unreachable.
struct Exclusive {
    count: u64,
    capped: bool,
    /// A digest that changes whenever which commits these are could change.
    evidence: String,
}

/// Commits reachable from `tip` but from no ref, stash entry, or worktree HEAD
/// that keeps them once the deletion removes `removed`. Listing stops just past
/// [`EXCLUSIVE_COMMIT_LIMIT`]. The evidence digests the listed commits; when the
/// list stops early, it also digests every ref, stash entry, and worktree HEAD
/// that remains, which decide the commits it did not list.
fn exclusive_commits(
    reachability: &Reachability,
    tip: &str,
    removed: &[&str],
) -> Result<Exclusive, String> {
    let exclusions = exclusions(reachability, removed);
    let mut args = vec![
        "rev-list".to_string(),
        format!("--max-count={}", EXCLUSIVE_COMMIT_LIMIT + 1),
        tip.to_string(),
        "--not".to_string(),
    ];
    args.extend(exclusions.iter().cloned());
    args.push("--all".to_string());
    args.extend(reachability.stashes.iter().cloned());
    let output = command::successful_git_at(reachability.worktree, &args)
        .map_err(|error| error.to_string())?;
    let listed = String::from_utf8_lossy(&output.stdout);
    let commits: Vec<&str> = listed.lines().filter(|line| !line.is_empty()).collect();
    let count = commits.len() as u64;
    let capped = count > EXCLUSIVE_COMMIT_LIMIT;

    let mut digest = Sha256::new();
    for commit in &commits {
        digest.update(commit.as_bytes());
        digest.update(b"\n");
    }
    if capped {
        let mut args = vec!["rev-parse".to_string()];
        args.extend(exclusions);
        args.push("--all".to_string());
        let refs = command::successful_git_at(reachability.worktree, &args)
            .map_err(|error| error.to_string())?;
        digest.update(b"remaining\n");
        digest.update(&refs.stdout);
        for stash in &reachability.stashes {
            digest.update(stash.as_bytes());
            digest.update(b"\n");
        }
        for seed in list_worktrees(reachability.repository_path)? {
            digest.update(seed.head.unwrap_or_default().as_bytes());
            digest.update(b"\n");
        }
    }
    Ok(Exclusive {
        count: count.min(EXCLUSIVE_COMMIT_LIMIT),
        capped,
        evidence: digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    })
}

/// `--exclude` arguments for the refs that do not keep commits once `removed`
/// are deleted: those refs themselves, symbolic refs that would be left pointing
/// at one of them, and the `refs/prefetch/` copies `git maintenance` keeps and
/// prunes on its own. Ref names cannot contain glob characters, so each name
/// matches only itself.
fn exclusions(reachability: &Reachability, removed: &[&str]) -> Vec<String> {
    let mut dangling: Vec<&str> = removed.to_vec();
    // Follow chains of symbolic refs until no more point at a removed ref.
    loop {
        let before = dangling.len();
        for (name, target) in &reachability.symbolic_refs {
            if dangling.contains(&target.as_str()) && !dangling.contains(&name.as_str()) {
                dangling.push(name);
            }
        }
        if dangling.len() == before {
            break;
        }
    }
    dangling
        .into_iter()
        .map(|reference| format!("--exclude={reference}"))
        .chain(std::iter::once("--exclude=refs/prefetch/*".to_string()))
        .collect()
}

/// Every symbolic ref under `refs/`, by name, and the ref it points to.
fn symbolic_refs(worktree: &Path) -> Result<BTreeMap<String, String>, String> {
    let output = command::successful_git_at(
        worktree,
        ["for-each-ref", "--format=%(refname)%00%(symref)", "refs/"],
    )
    .map_err(|error| error.to_string())?;
    Ok(output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter_map(|line| {
            let line = String::from_utf8_lossy(line);
            let (name, target) = line.split_once('\0')?;
            (!target.is_empty()).then(|| (name.to_string(), target.to_string()))
        })
        .collect())
}

/// Every stash entry's commit. `--all` includes only the newest, through
/// `refs/stash`; the others live in its reflog.
fn stash_entries(worktree: &Path) -> Result<Vec<String>, String> {
    let exists = command::git_at(worktree, ["show-ref", "--verify", "--quiet", "refs/stash"])
        .map_err(|error| error.to_string())?;
    if !exists.status.success() {
        return Ok(Vec::new());
    }
    let output = command::successful_git_at(
        worktree,
        ["log", "--walk-reflogs", "--format=%H", "refs/stash", "--"],
    )
    .map_err(|error| error.to_string())?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
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
    fn deletes_a_contained_local_branch_at_its_reviewed_tip() {
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
                [
                    "update-ref",
                    "--no-deref",
                    "-d",
                    "refs/heads/contained",
                    local.tip.as_str()
                ]
            )]
        );

        let result = execute(&plan, None).expect("delete contained branch");
        let step = result.local.expect("local step");
        assert!(step.succeeded);
        assert_eq!(step.deleted_oid, local.tip);
        let recovery = ["branch", "--", "contained", local.tip.as_str()];
        assert_eq!(
            step.recovery_commands,
            vec![git_command_line(Path::new(&plan.worktree_path), recovery)]
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
                    "--no-deref",
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
                    [
                        "update-ref",
                        "--no-deref",
                        "-d",
                        "refs/heads/forced",
                        local.tip.as_str()
                    ]
                ),
                git_command_line(
                    worktree,
                    ["config", "--local", "--remove-section", "branch.forced"]
                ),
            ]
        );

        // A branch that moves between the final review and the deletion keeps
        // its unreviewed commits: the reviewed command no longer matches it.
        let deletion = &local_deletion_commands(local, true)[0];
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
            warning.contains("configuration was not removed"),
            "{warning}"
        );
        let worktree = Path::new(&plan.worktree_path);
        assert_eq!(
            step.finish_commands,
            vec![git_command_line(
                worktree,
                ["config", "--local", "--remove-section", "branch.stuck"]
            )]
        );
        // The configuration is still there, so restoring adds none of it again.
        assert_eq!(
            step.recovery_commands,
            vec![git_command_line(
                worktree,
                ["branch", "--", "stuck", step.deleted_oid.as_str()]
            )]
        );
        assert!(has_branch_config(worktree, "stuck").expect("read configuration"));
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
    fn blocks_a_branch_being_rebased_in_another_worktree() {
        let fixture = Fixture::new();
        let linked = fixture.root.join("linked");
        git(
            &fixture.repository,
            &["worktree", "add", "-b", "rebasing", path(&linked)],
        );
        std::fs::write(linked.join("shared.txt"), "rebasing\n").expect("write file");
        git(&linked, &["add", "shared.txt"]);
        commit(&linked, "edits shared.txt on the branch");
        std::fs::write(fixture.repository.join("shared.txt"), "main\n").expect("write file");
        git(&fixture.repository, &["add", "shared.txt"]);
        commit(&fixture.repository, "edits shared.txt on main");
        let rebase = command::git_at(&linked, ["rebase", "main"]).expect("run rebase");
        assert!(!rebase.status.success(), "the rebase stops on its conflict");

        // The stopped rebase leaves the worktree detached, so only the rebase
        // state shows that it still uses the branch.
        let plan = fixture.plan("refs/heads/rebasing", true, false);
        assert!(plan.requires_force);
        let local = plan.local.as_ref().expect("local details");
        assert_eq!(local.occupied_worktree_path.as_deref(), Some(path(&linked)));
        assert!(
            plan.blockers
                .iter()
                .any(|blocker| blocker.contains("being rebased")),
            "{:?}",
            plan.blockers
        );
        execute(&plan, Some("rebasing")).expect_err("a branch being rebased must not be deleted");
        assert!(fixture.has_ref("refs/heads/rebasing"));
    }

    #[test]
    fn a_deletion_leaves_a_branch_that_moved_after_review() {
        let fixture = Fixture::new();
        commit(&fixture.repository, "second");
        git(&fixture.repository, &["branch", "contained"]);
        let reviewed =
            review(fixture.request("refs/heads/contained", true, false), None).expect("review");
        assert!(!reviewed.plan.requires_force);

        // Still contained, so `git branch -d` would delete it, but not at the
        // commit the review showed and the recovery command restores.
        git(
            &fixture.repository,
            &["update-ref", "refs/heads/contained", "HEAD~1"],
        );
        let local = reviewed.plan.local.as_ref().expect("local details");
        delete_local(&reviewed, local).expect_err("the branch moved to an unreviewed commit");
        assert!(fixture.has_ref("refs/heads/contained"));
    }

    #[test]
    fn a_contained_deletion_is_refused_once_its_merge_reference_moves() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "contained"]);
        let reviewed =
            review(fixture.request("refs/heads/contained", true, false), None).expect("review");
        assert_eq!(reviewed.merge_reference.as_deref(), Some("HEAD"));

        // HEAD moving on would still contain the branch, but rewinding it would
        // not, and nothing after the review has checked which happened.
        commit(&fixture.repository, "HEAD moves after the review");
        let local = reviewed.plan.local.as_ref().expect("local details");
        let error = delete_local(&reviewed, local).expect_err("the merge reference moved");
        assert!(
            error.contains("moved after this deletion was reviewed"),
            "{error}"
        );
        assert!(fixture.has_ref("refs/heads/contained"));
    }

    #[test]
    fn a_review_is_refused_once_the_push_url_changes() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "topic"]);
        git(&fixture.repository, &["push", "origin", "topic"]);
        let plan = fixture.plan("refs/remotes/origin/topic", false, true);
        assert_eq!(
            plan.fingerprint.push_destination,
            Some(digest(path(&fixture.remote)))
        );

        let mirror = fixture.root.join("mirror.git");
        git(
            &fixture.root,
            &["clone", "--bare", path(&fixture.remote), path(&mirror)],
        );
        git(
            &fixture.repository,
            &["config", "remote.origin.pushurl", path(&mirror)],
        );
        let error = execute(&plan, None).expect_err("the push now goes somewhere else");
        assert!(
            error.contains("changed after this deletion was reviewed"),
            "{error}"
        );
        assert!(fixture.remote_has_branch("topic"));
    }

    #[test]
    fn the_reviewed_push_url_hides_credentials() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "topic"]);
        git(&fixture.repository, &["push", "origin", "topic"]);
        git(
            &fixture.repository,
            &[
                "config",
                "remote.origin.pushurl",
                "https://user:secret-value@example.invalid/repository.git",
            ],
        );
        let plan = fixture.plan("refs/remotes/origin/topic", false, true);
        let remote = plan.remote.as_ref().expect("remote details");
        assert!(
            remote.push_url.contains("example.invalid"),
            "{}",
            remote.push_url
        );
        assert!(
            !remote.push_url.contains("secret-value"),
            "{}",
            remote.push_url
        );
        let destination = plan
            .fingerprint
            .push_destination
            .as_deref()
            .expect("the push destination is fingerprinted");
        assert!(!destination.contains("secret-value"), "{destination}");
    }

    #[test]
    fn push_urls_that_display_alike_are_still_told_apart() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "topic"]);
        git(&fixture.repository, &["push", "origin", "topic"]);
        // Credential redaction shows both of these as the same URL.
        let reviewed = fixture.root.join("ghs_reviewed.git");
        let other = fixture.root.join("ghs_other.git");
        for mirror in [&reviewed, &other] {
            git(
                &fixture.root,
                &["clone", "--bare", path(&fixture.remote), path(mirror)],
            );
        }
        git(
            &fixture.repository,
            &["config", "remote.origin.pushurl", path(&reviewed)],
        );
        let plan = fixture.plan("refs/remotes/origin/topic", false, true);

        git(
            &fixture.repository,
            &["config", "remote.origin.pushurl", path(&other)],
        );
        let now = fixture.plan("refs/remotes/origin/topic", false, true);
        assert_eq!(
            plan.remote.as_ref().expect("remote details").push_url,
            now.remote.as_ref().expect("remote details").push_url,
            "the redacted URLs read the same"
        );
        let error = execute(&plan, None).expect_err("the push now goes elsewhere");
        assert!(
            error.contains("changed after this deletion was reviewed"),
            "{error}"
        );
        let kept = command::git_at(
            &other,
            ["show-ref", "--verify", "--quiet", "refs/heads/topic"],
        )
        .expect("show-ref on the other URL");
        assert!(kept.status.success());
    }

    #[test]
    fn a_symbolic_branch_ref_is_not_deleted() {
        let fixture = Fixture::new();
        git(
            &fixture.repository,
            &["symbolic-ref", "refs/heads/alias", "refs/heads/main"],
        );
        let plan = fixture.plan("refs/heads/alias", true, false);
        assert!(
            plan.blockers
                .iter()
                .any(|blocker| blocker.contains("symbolic ref to main")),
            "{:?}",
            plan.blockers
        );
        execute(&plan, None).expect_err("a symbolic ref is refused");
        assert!(fixture.has_ref("refs/heads/alias"));
        assert!(fixture.has_ref("refs/heads/main"));
    }

    #[test]
    fn the_deletion_command_never_follows_a_symbolic_ref() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "alias"]);
        let reviewed =
            review(fixture.request("refs/heads/alias", true, false), None).expect("review");
        // Should the name become symbolic after the last check, the command
        // itself still deletes only that name.
        git(
            &fixture.repository,
            &["symbolic-ref", "refs/heads/alias", "refs/heads/main"],
        );
        let deletion: Vec<&str> = reviewed.local_commands[0]
            .iter()
            .map(String::as_str)
            .collect();
        git(&fixture.repository, &deletion);
        assert!(!fixture.has_ref("refs/heads/alias"));
        assert!(fixture.has_ref("refs/heads/main"));
    }

    #[test]
    fn a_branch_that_becomes_a_symbolic_ref_after_review_is_not_deleted() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "alias"]);
        let reviewed =
            review(fixture.request("refs/heads/alias", true, false), None).expect("review");
        assert!(
            reviewed.plan.blockers.is_empty(),
            "{:?}",
            reviewed.plan.blockers
        );

        // Same commit, but the name now leads to main.
        git(
            &fixture.repository,
            &["symbolic-ref", "refs/heads/alias", "refs/heads/main"],
        );
        let local = reviewed.plan.local.as_ref().expect("local details");
        let error = delete_local(&reviewed, local)
            .expect_err("a branch that became symbolic is not deleted");
        assert!(error.contains("symbolic ref to main"), "{error}");
        assert!(fixture.has_ref("refs/heads/alias"));
        assert!(fixture.has_ref("refs/heads/main"));
    }

    #[test]
    fn blocks_a_branch_being_bisected_in_another_worktree() {
        let fixture = Fixture::new();
        let linked = fixture.root.join("linked");
        git(
            &fixture.repository,
            &["worktree", "add", "-b", "bisected", path(&linked)],
        );
        commit(&linked, "second");
        commit(&linked, "third");
        git(&linked, &["bisect", "start", "HEAD", "HEAD~2"]);

        let plan = fixture.plan("refs/heads/bisected", true, false);
        let local = plan.local.as_ref().expect("local details");
        assert_eq!(local.occupied_worktree_path.as_deref(), Some(path(&linked)));
        assert!(
            plan.blockers
                .iter()
                .any(|blocker| blocker.contains("being bisected")),
            "{:?}",
            plan.blockers
        );
        execute(&plan, Some("bisected")).expect_err("a branch being bisected must not be deleted");
        assert!(fixture.has_ref("refs/heads/bisected"));
    }

    #[test]
    fn commits_kept_by_an_older_stash_entry_are_not_lost() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["switch", "--create", "stashed"]);
        commit(&fixture.repository, "only on stashed");
        std::fs::write(fixture.repository.join("work.txt"), "first\n").expect("write file");
        git(
            &fixture.repository,
            &["stash", "push", "--include-untracked"],
        );
        git(&fixture.repository, &["switch", "main"]);
        // A newer stash on main pushes the first one out of `refs/stash`.
        std::fs::write(fixture.repository.join("work.txt"), "second\n").expect("write file");
        git(
            &fixture.repository,
            &["stash", "push", "--include-untracked"],
        );

        let plan = fixture.plan("refs/heads/stashed", true, false);
        let local = plan.local.as_ref().expect("local details");
        assert_eq!(
            local.exclusive_commit_count, 0,
            "the older stash entry still reaches the branch's commit"
        );
    }

    #[test]
    fn a_review_is_refused_once_different_commits_would_be_lost() {
        let fixture = Fixture::new();
        // feature merges two side lines; keeping one or the other loses the
        // same number of commits, but not the same commits.
        git(&fixture.repository, &["switch", "--create", "left"]);
        commit(&fixture.repository, "left");
        git(
            &fixture.repository,
            &["switch", "--create", "right", "main"],
        );
        commit(&fixture.repository, "right");
        git(
            &fixture.repository,
            &["switch", "--create", "feature", "left"],
        );
        git(
            &fixture.repository,
            &["merge", "--no-ff", "--no-edit", "right"],
        );
        git(&fixture.repository, &["switch", "main"]);
        git(
            &fixture.repository,
            &["branch", "--delete", "--force", "right"],
        );
        let plan = fixture.plan("refs/heads/feature", true, false);
        assert_eq!(
            plan.local
                .as_ref()
                .expect("local details")
                .exclusive_commit_count,
            2
        );

        git(&fixture.repository, &["branch", "right", "feature^2"]);
        git(
            &fixture.repository,
            &["branch", "--delete", "--force", "left"],
        );
        let error = execute(&plan, Some("feature")).expect_err("other commits would be lost");
        assert!(
            error.contains("changed after this deletion was reviewed"),
            "{error}"
        );
        assert!(fixture.has_ref("refs/heads/feature"));
    }

    #[test]
    fn a_review_is_refused_once_its_commands_or_warnings_change() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "contained"]);
        let plan = fixture.plan("refs/heads/contained", true, false);
        assert_eq!(plan.commands.len(), 1);

        // The configuration cleanup the deletion would now run was never shown.
        git(
            &fixture.repository,
            &[
                "config",
                "branch.contained.description",
                "added after review",
            ],
        );
        let error = execute(&plan, None).expect_err("the commands changed");
        assert!(
            error.contains("changed after this deletion was reviewed"),
            "{error}"
        );

        git(&fixture.repository, &["branch", "topic"]);
        git(&fixture.repository, &["push", "origin", "topic"]);
        let plan = fixture.plan("refs/remotes/origin/topic", false, true);
        assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
        // A branch that starts tracking it would lose its upstream unwarned.
        git(
            &fixture.repository,
            &["branch", "--track", "follower", "origin/topic"],
        );
        let error = execute(&plan, None).expect_err("the warnings changed");
        assert!(
            error.contains("changed after this deletion was reviewed"),
            "{error}"
        );
        assert!(fixture.remote_has_branch("topic"));
    }

    #[test]
    fn the_recovery_restores_the_branch_and_its_configuration() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["switch", "--create", "configured"]);
        git(
            &fixture.repository,
            &["push", "--set-upstream", "origin", "configured"],
        );
        git(&fixture.repository, &["switch", "main"]);
        git(
            &fixture.repository,
            &["config", "branch.configured.description", "kept work"],
        );
        let plan = fixture.plan("refs/heads/configured", true, false);
        let tip = plan.local.as_ref().expect("local details").tip.clone();
        let result = execute(&plan, None).expect("delete the branch");
        let step = result.local.expect("local step");

        let worktree = Path::new(&plan.worktree_path);
        let recovery: [&[&str]; 4] = [
            &["branch", "--", "configured", tip.as_str()],
            &[
                "config",
                "--local",
                "--add",
                "branch.configured.remote",
                "origin",
            ],
            &[
                "config",
                "--local",
                "--add",
                "branch.configured.merge",
                "refs/heads/configured",
            ],
            &[
                "config",
                "--local",
                "--add",
                "branch.configured.description",
                "kept work",
            ],
        ];
        assert_eq!(
            step.recovery_commands,
            recovery
                .iter()
                .map(|args| git_command_line(worktree, *args))
                .collect::<Vec<_>>()
        );
        assert!(!has_branch_config(worktree, "configured").expect("read configuration"));

        for args in recovery {
            git(&fixture.repository, args);
        }
        let upstream = command::successful_git_at(
            &fixture.repository,
            ["rev-parse", "--abbrev-ref", "configured@{upstream}"],
        )
        .expect("upstream restored");
        assert_eq!(
            String::from_utf8_lossy(&upstream.stdout).trim(),
            "origin/configured"
        );
    }

    #[test]
    fn a_push_url_that_url_rewriting_would_change_again_is_not_asked() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "topic"]);
        git(&fixture.repository, &["push", "origin", "topic"]);
        let mirror = fixture.root.join("mirror.git");
        git(
            &fixture.root,
            &["clone", "--bare", path(&fixture.remote), path(&mirror)],
        );
        git(&mirror, &["symbolic-ref", "HEAD", "refs/heads/topic"]);
        // The push goes to the mirror, where topic is the default branch, but
        // asking the mirror's URL directly would be rewritten to origin.
        let mirror_key = format!("url.{}.insteadOf", path(&mirror));
        let origin_key = format!("url.{}.insteadOf", path(&fixture.remote));
        git(
            &fixture.repository,
            &["config", mirror_key.as_str(), "repola-test:push"],
        );
        git(
            &fixture.repository,
            &["config", origin_key.as_str(), path(&mirror)],
        );
        git(
            &fixture.repository,
            &["config", "remote.origin.pushurl", "repola-test:push"],
        );
        let plan = fixture.plan("refs/remotes/origin/topic", false, true);
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);

        let error = execute(&plan, None).expect_err("the check would ask another server");
        assert!(error.contains("URL rewriting"), "{error}");
        let kept = command::git_at(
            &mirror,
            ["show-ref", "--verify", "--quiet", "refs/heads/topic"],
        )
        .expect("show-ref on the push destination");
        assert!(kept.status.success());
    }

    #[test]
    fn prefetched_copies_do_not_keep_remote_commits() {
        let fixture = Fixture::new();
        let peer = fixture.peer();
        git(&peer, &["switch", "--create", "peer-only"]);
        commit(&peer, "exists only on the remote");
        git(&peer, &["push", "origin", "peer-only"]);
        git(&fixture.repository, &["fetch", "origin"]);
        // `git maintenance` keeps these and prunes them on its own schedule.
        git(
            &fixture.repository,
            &[
                "update-ref",
                "refs/prefetch/remotes/origin/peer-only",
                "refs/remotes/origin/peer-only",
            ],
        );

        let plan = fixture.plan("refs/remotes/origin/peer-only", false, true);
        let remote = plan.remote.as_ref().expect("remote details");
        assert_eq!(remote.exclusive_commit_count, 1);
        assert_eq!(
            plan.confirmation,
            BranchDeletionConfirmation::TypeBranchName
        );
    }

    #[test]
    fn a_symbolic_ref_to_the_branch_does_not_keep_its_commits() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["switch", "--create", "lonely"]);
        commit(&fixture.repository, "only on lonely");
        git(&fixture.repository, &["switch", "main"]);
        git(
            &fixture.repository,
            &["symbolic-ref", "refs/heads/alias", "refs/heads/lonely"],
        );
        git(
            &fixture.repository,
            &[
                "symbolic-ref",
                "refs/heads/alias-of-alias",
                "refs/heads/alias",
            ],
        );

        let plan = fixture.plan("refs/heads/lonely", true, false);
        assert_eq!(
            plan.local
                .as_ref()
                .expect("local details")
                .exclusive_commit_count,
            1,
            "both aliases dangle once lonely is deleted"
        );
    }

    #[test]
    fn an_upstream_outside_refs_remotes_is_checked_like_git_branch_d() {
        let fixture = Fixture::new();
        git(
            &fixture.repository,
            &["remote", "add", "up", path(&fixture.remote)],
        );
        git(
            &fixture.repository,
            &[
                "config",
                "--replace-all",
                "remote.up.fetch",
                "+refs/heads/*:refs/up/*",
            ],
        );
        git(&fixture.repository, &["fetch", "up"]);
        git(&fixture.repository, &["switch", "--create", "elsewhere"]);
        commit(&fixture.repository, "merged into HEAD but not the upstream");
        git(&fixture.repository, &["switch", "main"]);
        git(&fixture.repository, &["merge", "--ff-only", "elsewhere"]);
        git(
            &fixture.repository,
            &["config", "branch.elsewhere.remote", "up"],
        );
        git(
            &fixture.repository,
            &["config", "branch.elsewhere.merge", "refs/heads/main"],
        );

        let plan = fixture.plan("refs/heads/elsewhere", true, false);
        let local = plan.local.as_ref().expect("local details");
        assert_eq!(local.merge_reference_kind, MergeReferenceKind::Upstream);
        assert_eq!(local.merge_reference, "refs/up/main");
        assert!(!local.contained_in_merge_reference);
        assert!(plan.requires_force);
        let git_branch = command::git_at(&fixture.repository, ["branch", "-d", "elsewhere"])
            .expect("run git branch -d");
        assert!(!git_branch.status.success(), "git branch -d refuses it too");
    }

    #[test]
    fn a_configuration_key_without_a_value_is_restored_as_true() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "flagged"]);
        let config = fixture.repository.join(".git").join("config");
        let mut contents = std::fs::read_to_string(&config).expect("read configuration");
        contents.push_str("[branch \"flagged\"]\n\trebase\n");
        std::fs::write(&config, contents).expect("write configuration");

        let plan = fixture.plan("refs/heads/flagged", true, false);
        let result = execute(&plan, None).expect("delete the branch");
        let step = result.local.expect("local step");
        let worktree = Path::new(&plan.worktree_path);
        assert_eq!(
            step.recovery_commands.last(),
            Some(&git_command_line(
                worktree,
                [
                    "config",
                    "--local",
                    "--add",
                    "branch.flagged.rebase",
                    "true"
                ]
            ))
        );
    }

    #[test]
    fn the_push_goes_nowhere_once_the_push_url_changes_after_review() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "topic"]);
        git(&fixture.repository, &["push", "origin", "topic"]);
        let reviewed = review(
            fixture.request("refs/remotes/origin/topic", false, true),
            None,
        )
        .expect("review");
        let mirror = fixture.root.join("mirror.git");
        git(
            &fixture.root,
            &["clone", "--bare", path(&fixture.remote), path(&mirror)],
        );
        git(
            &fixture.repository,
            &["config", "remote.origin.pushurl", path(&mirror)],
        );

        let remote = reviewed.plan.remote.as_ref().expect("remote details");
        let deletion = delete_remote(&reviewed, remote);
        assert!(matches!(deletion.outcome, RemoteOutcome::Failed));
        assert!(
            deletion.output.contains("no longer pushes to"),
            "{}",
            deletion.output
        );
        assert!(fixture.remote_has_branch("topic"));
        let kept = command::git_at(
            &mirror,
            ["show-ref", "--verify", "--quiet", "refs/heads/topic"],
        )
        .expect("show-ref on the new push URL");
        assert!(kept.status.success());
    }

    #[test]
    fn a_loss_that_grows_past_the_cap_requires_another_review() {
        let fixture = Fixture::new();
        // More commits than the count follows, only on `deep`, with `keeper`
        // holding all but the last few of them.
        let total = EXCLUSIVE_COMMIT_LIMIT + 5;
        let mut stream = String::new();
        for mark in 1..=total {
            stream.push_str(&format!(
                "commit refs/heads/deep\nmark :{mark}\ncommitter Branch Deletion Test <branches@example.invalid> {mark} +0000\ndata 0\n"
            ));
            if mark > 1 {
                stream.push_str(&format!("from :{}\n", mark - 1));
            }
            stream.push('\n');
        }
        stream.push_str("reset refs/heads/keeper\nfrom :2\n\n");
        let imported = command::git_at_with_input(
            &fixture.repository,
            ["fast-import", "--quiet"],
            stream.as_bytes(),
        )
        .expect("run fast-import");
        assert!(
            imported.status.success(),
            "{}",
            String::from_utf8_lossy(&imported.stderr)
        );

        let plan = fixture.plan("refs/heads/deep", true, false);
        let local = plan.local.as_ref().expect("local details");
        assert!(local.exclusive_commit_count_capped);

        // Two more commits become unreachable, but the count stays capped.
        git(
            &fixture.repository,
            &["branch", "--delete", "--force", "keeper"],
        );
        let error = execute(&plan, Some("deep")).expect_err("the loss grew past the cap");
        assert!(
            error.contains("changed after this deletion was reviewed"),
            "{error}"
        );
        assert!(fixture.has_ref("refs/heads/deep"));
    }

    #[test]
    fn a_worktree_that_starts_using_the_branch_after_review_keeps_it() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["switch", "--create", "late"]);
        commit(&fixture.repository, "only on late");
        git(&fixture.repository, &["switch", "main"]);
        let reviewed =
            review(fixture.request("refs/heads/late", true, false), None).expect("review");
        assert!(
            reviewed.plan.blockers.is_empty(),
            "{:?}",
            reviewed.plan.blockers
        );
        assert!(reviewed.plan.requires_force);

        let linked = fixture.root.join("late");
        git(
            &fixture.repository,
            &["worktree", "add", path(&linked), "late"],
        );
        let local = reviewed.plan.local.as_ref().expect("local details");
        let error = delete_local(&reviewed, local).expect_err("the branch is checked out by now");
        assert!(error.contains("checked out"), "{error}");
        assert!(fixture.has_ref("refs/heads/late"));
    }

    #[test]
    fn a_local_branch_is_kept_once_its_loss_grows_after_review() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["switch", "--create", "feature"]);
        commit(&fixture.repository, "kept by keeper at review");
        git(&fixture.repository, &["branch", "keeper"]);
        git(&fixture.repository, &["switch", "main"]);
        let reviewed =
            review(fixture.request("refs/heads/feature", true, false), None).expect("review");
        let local = reviewed.plan.local.as_ref().expect("local details");
        assert_eq!(local.exclusive_commit_count, 0);

        git(
            &fixture.repository,
            &["branch", "--delete", "--force", "keeper"],
        );
        let error = delete_local(&reviewed, local).expect_err("the loss grew after review");
        assert!(error.contains("would leave unreachable changed"), "{error}");
        assert!(fixture.has_ref("refs/heads/feature"));
    }

    #[test]
    fn a_remote_branch_is_kept_once_its_loss_grows_after_review() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["switch", "--create", "topic"]);
        commit(&fixture.repository, "kept by the local branch at review");
        git(&fixture.repository, &["push", "origin", "topic"]);
        git(&fixture.repository, &["switch", "main"]);
        let reviewed = review(
            fixture.request("refs/remotes/origin/topic", false, true),
            None,
        )
        .expect("review");
        let remote = reviewed.plan.remote.as_ref().expect("remote details");
        assert_eq!(remote.exclusive_commit_count, 0);

        git(
            &fixture.repository,
            &["branch", "--delete", "--force", "topic"],
        );
        let deletion = delete_remote(&reviewed, remote);
        assert!(matches!(deletion.outcome, RemoteOutcome::Failed));
        assert!(
            deletion.output.contains("would leave unreachable"),
            "{}",
            deletion.output
        );
        assert!(fixture.remote_has_branch("topic"));
    }

    #[test]
    fn a_symbolic_ref_left_dangling_by_the_local_deletion_does_not_stop_the_remote_one() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["switch", "--create", "lonely"]);
        commit(&fixture.repository, "only on lonely");
        git(
            &fixture.repository,
            &["push", "--set-upstream", "origin", "lonely"],
        );
        git(&fixture.repository, &["switch", "main"]);
        git(
            &fixture.repository,
            &["symbolic-ref", "refs/heads/alias", "refs/heads/lonely"],
        );

        let plan = fixture.plan("refs/heads/lonely", true, true);
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
        let result = execute(&plan, Some("lonely")).expect("delete both copies");
        let remote = result.remote.expect("remote step");
        assert!(remote.succeeded, "{}", remote.output);
        assert!(!fixture.remote_has_branch("lonely"));
    }

    /// The full ref names `branches_in_progress` finds in `directory`.
    fn in_progress(directory: &Path) -> Vec<String> {
        branches_in_progress(directory)
            .expect("read state")
            .into_iter()
            .map(|(reference, _)| reference)
            .collect()
    }

    fn write_state(directory: &Path, file: &str, contents: &str) {
        let path = directory.join(file);
        std::fs::create_dir_all(path.parent().expect("state directory")).expect("create state");
        std::fs::write(path, contents).expect("write state");
    }

    #[test]
    fn a_rebase_returns_to_the_branch_its_head_name_records() {
        let temp = tempfile::tempdir().expect("temp directory");
        write_state(
            temp.path(),
            "rebase-merge/head-name",
            "refs/heads/rebased\n",
        );
        assert_eq!(in_progress(temp.path()), ["refs/heads/rebased"]);
    }

    #[test]
    fn rebase_apply_takes_precedence_over_rebase_merge_as_in_git() {
        let temp = tempfile::tempdir().expect("temp directory");
        write_state(
            temp.path(),
            "rebase-apply/head-name",
            "refs/heads/applied\n",
        );
        write_state(temp.path(), "rebase-merge/head-name", "refs/heads/merged\n");
        assert_eq!(in_progress(temp.path()), ["refs/heads/applied"]);

        // `git am` uses rebase-apply too, and leaves no branch in use.
        write_state(temp.path(), "rebase-apply/applying", "");
        assert!(in_progress(temp.path()).is_empty());
    }

    #[test]
    fn a_rebase_of_a_detached_head_uses_no_branch() {
        let temp = tempfile::tempdir().expect("temp directory");
        write_state(temp.path(), "rebase-merge/head-name", "detached HEAD\n");
        assert!(in_progress(temp.path()).is_empty());
        write_state(
            temp.path(),
            "rebase-merge/head-name",
            &format!("{}\n", "a".repeat(40)),
        );
        assert!(in_progress(temp.path()).is_empty());
    }

    #[test]
    fn a_bisect_is_in_progress_only_while_its_log_exists() {
        let temp = tempfile::tempdir().expect("temp directory");
        write_state(temp.path(), "BISECT_START", "bisected\n");
        assert!(in_progress(temp.path()).is_empty());
        write_state(temp.path(), "BISECT_LOG", "");
        assert_eq!(in_progress(temp.path()), ["refs/heads/bisected"]);
    }

    #[test]
    fn update_refs_names_every_ref_a_rebase_will_move_unless_malformed() {
        let temp = tempfile::tempdir().expect("temp directory");
        let oid = "a".repeat(40);
        write_state(
            temp.path(),
            "rebase-merge/update-refs",
            &format!("refs/heads/stacked\n{oid}\n{oid}\nrefs/heads/other\n{oid}\n{oid}\n"),
        );
        assert_eq!(
            in_progress(temp.path()),
            ["refs/heads/stacked", "refs/heads/other"]
        );

        // Git reads none of a file it cannot parse.
        write_state(
            temp.path(),
            "rebase-merge/update-refs",
            &format!("refs/heads/stacked\n{oid}\n{oid}\nrefs/heads/other\n{oid}\n"),
        );
        assert!(in_progress(temp.path()).is_empty());
    }

    #[test]
    fn linked_worktrees_are_found_from_their_gitdir_files_as_git_finds_them() {
        let temp = tempfile::tempdir().expect("temp directory");
        let directory = temp.path().join("worktrees").join("linked");
        std::fs::create_dir_all(&directory).expect("administrative directory");
        assert_eq!(linked_worktree_path(&directory).expect("read gitdir"), None);

        write_state(&directory, "gitdir", "\n");
        assert_eq!(linked_worktree_path(&directory).expect("read gitdir"), None);

        let absolute = temp.path().join("checkout");
        write_state(
            &directory,
            "gitdir",
            &format!("{}/.git\n", absolute.to_string_lossy().replace('\\', "/")),
        );
        assert_eq!(
            linked_worktree_path(&directory).expect("read gitdir"),
            Some(native_path(&absolute.to_string_lossy().replace('\\', "/")))
        );

        // `worktree.useRelativePaths` records the path from this directory.
        let relative = temp.path().join("relative");
        std::fs::create_dir_all(&relative).expect("relative checkout");
        write_state(&directory, "gitdir", "../../relative/.git\n");
        assert_eq!(
            linked_worktree_path(&directory).expect("read gitdir"),
            Some(native_path(
                &dunce::canonicalize(&relative)
                    .expect("canonical checkout")
                    .to_string_lossy()
            ))
        );

        let stray = temp.path().join("worktrees").join("stray");
        std::fs::write(&stray, "").expect("stray file");
        assert_eq!(linked_worktree_path(&stray).expect("skip a file"), None);
    }

    #[test]
    fn a_remote_that_pushes_to_several_urls_is_not_deleted_from() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "topic"]);
        git(&fixture.repository, &["push", "origin", "topic"]);
        let mirror = fixture.root.join("mirror.git");
        git(
            &fixture.root,
            &["clone", "--bare", path(&fixture.remote), path(&mirror)],
        );
        for url in [path(&fixture.remote), path(&mirror)] {
            git(
                &fixture.repository,
                &["config", "--add", "remote.origin.pushurl", url],
            );
        }
        let plan = fixture.plan("refs/remotes/origin/topic", false, true);
        assert!(plan.remote.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|blocker| blocker.contains("pushes to 2 URLs")),
            "{:?}",
            plan.blockers
        );
        execute(&plan, None).expect_err("a deletion across several URLs is refused");
        assert!(fixture.remote_has_branch("topic"));
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
            plan.commands.last(),
            Some(&git_command_line(Path::new(&plan.worktree_path), push))
        );

        let result = execute(&plan, Some("published")).expect("delete both copies");
        assert!(result.local.expect("local step").succeeded);
        let remote_step = result.remote.expect("remote step");
        assert!(remote_step.succeeded);
        assert!(!fixture.has_ref("refs/heads/published"));
        assert!(!fixture.has_ref("refs/remotes/origin/published"));
        assert!(!fixture.remote_has_branch("published"));

        let refspec = format!("{}:refs/heads/published", remote.expected_oid);
        let recovery = [
            "push",
            "--force-with-lease=refs/heads/published:",
            "--",
            "origin",
            refspec.as_str(),
        ];
        assert_eq!(
            remote_step.recovery_commands,
            vec![git_command_line(Path::new(&plan.worktree_path), recovery)]
        );

        // Someone recreates the branch at an older commit: restoring must not
        // move it.
        let peer = fixture.peer();
        git(&peer, &["push", "origin", "main:refs/heads/published"]);
        let refused = command::git_at(&fixture.repository, recovery).expect("run recovery");
        assert!(
            !refused.status.success(),
            "the restore leaves a recreated branch alone"
        );
        git(&peer, &["push", "origin", "--delete", "published"]);

        git(&fixture.repository, &recovery);
        assert!(
            fixture.remote_has_branch("published"),
            "the recovery command republishes the branch"
        );
    }

    #[test]
    fn reads_the_push_report_for_the_deleted_branch() {
        let deleted = "To origin\n-\t:refs/heads/topic\t[deleted]\nDone\n";
        assert_eq!(reported_deletion(deleted, "refs/heads/topic"), Some(true));
        let rejected = "To origin\n!\t(delete):refs/heads/topic\t[rejected] (stale info)\nDone\n";
        assert_eq!(reported_deletion(rejected, "refs/heads/topic"), Some(false));
        let declined =
            "To origin\n!\t:refs/heads/topic\t[remote rejected] (pre-receive hook declined)\nDone\n";
        assert_eq!(reported_deletion(declined, "refs/heads/topic"), Some(false));
        assert_eq!(reported_deletion(deleted, "refs/heads/other"), None);
        assert_eq!(reported_deletion("", "refs/heads/topic"), None);
    }

    /// Installs a pre-push hook running `body`, which then fails the push before
    /// Git sends anything or reports a status for the branch.
    fn failing_pre_push(repository: &Path, body: &str) {
        let hooks = repository.join(".git").join("hooks");
        std::fs::create_dir_all(&hooks).expect("hooks directory");
        let script = hooks.join("pre-push");
        std::fs::write(
            &script,
            format!("#!/bin/sh\nunset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE\n{body}\nexit 1\n"),
        )
        .expect("write hook");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("make the hook executable");
        }
    }

    #[test]
    fn a_push_without_a_report_is_settled_by_asking_the_remote() {
        let remote_path = |fixture: &Fixture| fixture.remote.to_string_lossy().replace('\\', "/");

        // Still there: the push did not delete it.
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "kept"]);
        git(&fixture.repository, &["push", "origin", "kept"]);
        let plan = fixture.plan("refs/remotes/origin/kept", false, true);
        failing_pre_push(&fixture.repository, ":");
        execute(&plan, None).expect_err("the remote still has the branch");
        assert!(fixture.remote_has_branch("kept"));

        // A hook that prints Git's deletion line and then stops the push.
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "claimed"]);
        git(&fixture.repository, &["push", "origin", "claimed"]);
        let plan = fixture.plan("refs/remotes/origin/claimed", false, true);
        failing_pre_push(
            &fixture.repository,
            "printf '%s\\t%s\\t%s\\n' - :refs/heads/claimed '[deleted]'",
        );
        execute(&plan, None).expect_err("the hook stopped the push");
        assert!(fixture.remote_has_branch("claimed"));

        // Gone: the report was lost, but the branch was deleted.
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "lost"]);
        git(&fixture.repository, &["push", "origin", "lost"]);
        let plan = fixture.plan("refs/remotes/origin/lost", false, true);
        failing_pre_push(
            &fixture.repository,
            &format!(
                "git -C '{}' update-ref -d refs/heads/lost",
                remote_path(&fixture)
            ),
        );
        let result = execute(&plan, None).expect("the branch is gone");
        assert_eq!(result.message, "Deleted remote branch origin/lost.");
        assert!(!fixture.has_ref("refs/remotes/origin/lost"));

        // Unreachable: whether it was deleted is unknown.
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "unknown"]);
        git(&fixture.repository, &["push", "origin", "unknown"]);
        let plan = fixture.plan("refs/remotes/origin/unknown", false, true);
        // Asking afterwards goes through the remote's name, now pointed at a
        // repository that does not exist. Moving the remote itself would fail
        // on Windows, where the running push keeps it open.
        let missing = fixture.root.join("missing.git");
        failing_pre_push(
            &fixture.repository,
            &format!(
                "git -C '{}' config remote.origin.pushurl '{}'",
                fixture.repository.to_string_lossy().replace('\\', "/"),
                missing.to_string_lossy().replace('\\', "/")
            ),
        );
        let result = execute(&plan, None).expect("an unknown outcome is not a refusal");
        assert_eq!(
            result.message,
            "Repola could not confirm whether remote branch origin/unknown was deleted."
        );
        assert!(result.remote.as_ref().expect("remote step").unconfirmed);
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

        let moved = command::successful_git_at(&peer, ["rev-parse", "HEAD"]).expect("peer tip");
        let moved = String::from_utf8_lossy(&moved.stdout).trim().to_string();
        let error = execute(&plan, None).expect_err("the lease must reject a moved branch");
        assert!(error.contains("stale info"), "{error}");
        assert!(
            error.contains(&format!("is now at {moved}")),
            "the error names where the branch moved: {error}"
        );
        assert!(fixture.remote_has_branch("shared"));
    }

    #[test]
    fn a_branch_hidden_from_fetching_is_not_taken_for_deleted() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "hidden"]);
        git(&fixture.repository, &["push", "origin", "hidden"]);
        let plan = fixture.plan("refs/remotes/origin/hidden", false, true);

        let peer = fixture.peer();
        git(&peer, &["switch", "hidden"]);
        commit(&peer, "pushed after review");
        git(&peer, &["push", "origin", "hidden"]);
        // Fetching no longer shows the branch, but pushing still does.
        git(
            &fixture.remote,
            &["config", "uploadpack.hideRefs", "refs/heads/hidden"],
        );

        let error = execute(&plan, None).expect_err("the branch moved");
        assert!(error.contains("moved from the reviewed"), "{error}");
        assert!(fixture.has_ref("refs/remotes/origin/hidden"));
        assert!(fixture.remote_has_branch("hidden"));
    }

    #[test]
    fn the_branch_is_looked_for_where_the_push_goes() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "rewritten"]);
        git(&fixture.repository, &["push", "origin", "rewritten"]);
        // A push URL is never rewritten by `pushInsteadOf`, but a URL given to
        // `git push` directly would be, here to a repository without the branch.
        let decoy = fixture.root.join("decoy.git");
        git(&fixture.root, &["init", "--bare", path(&decoy)]);
        let rewrite = format!("url.{}.pushInsteadOf", path(&decoy));
        git(
            &fixture.repository,
            &["config", rewrite.as_str(), path(&fixture.remote)],
        );
        git(
            &fixture.repository,
            &["config", "remote.origin.pushurl", path(&fixture.remote)],
        );
        let plan = fixture.plan("refs/remotes/origin/rewritten", false, true);

        let peer = fixture.peer();
        git(&peer, &["switch", "rewritten"]);
        commit(&peer, "pushed after review");
        git(&peer, &["push", "origin", "rewritten"]);

        let error = execute(&plan, None).expect_err("the branch moved");
        assert!(error.contains("is now at"), "{error}");
        assert!(fixture.has_ref("refs/remotes/origin/rewritten"));
        assert!(fixture.remote_has_branch("rewritten"));
    }

    /// Installs a `hook` that marks `marker` once it starts, then waits.
    fn blocking_hook(repository: &Path, hook: &str, marker: &Path) {
        let hooks = repository.join(".git").join("hooks");
        std::fs::create_dir_all(&hooks).expect("hooks directory");
        let script = hooks.join(hook);
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n: > '{}'\nsleep 5\n",
                marker.to_string_lossy().replace('\\', "/")
            ),
        )
        .expect("write hook");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("make the hook executable");
        }
    }

    /// Runs `run` as an operation that is cancelled once `marker` appears.
    fn cancelled_when<T>(marker: &Path, run: impl FnOnce() -> T) -> T {
        let token = crate::operation::OperationToken::new();
        let cancel = token.clone();
        let marker = marker.to_path_buf();
        let watcher = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while !marker.exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            cancel.cancel();
        });
        let result = crate::operation::with_operation(token, run);
        watcher.join().expect("watcher");
        result
    }

    #[test]
    fn a_push_interrupted_after_it_started_is_reported_as_unconfirmed() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "slow"]);
        git(&fixture.repository, &["push", "origin", "slow"]);
        let plan = fixture.plan("refs/remotes/origin/slow", false, true);
        let marker = fixture.root.join("push-started");
        blocking_hook(&fixture.repository, "pre-push", &marker);

        let result = cancelled_when(&marker, || execute(&plan, None))
            .expect("an interrupted push is not a refusal");
        assert_eq!(
            result.message,
            "Repola could not confirm whether remote branch origin/slow was deleted."
        );
        let remote = result.remote.as_ref().expect("remote step");
        assert!(remote.unconfirmed && !remote.succeeded);
        assert_eq!(remote.recovery_commands.len(), 1);
        // The hook held the push back, so the remote kept the branch here.
        assert!(fixture.remote_has_branch("slow"));
    }

    #[test]
    fn a_push_with_more_output_than_kept_is_reported_as_unconfirmed() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "noisy"]);
        git(&fixture.repository, &["push", "origin", "noisy"]);
        let plan = fixture.plan("refs/remotes/origin/noisy", false, true);
        // The hook lets the push go ahead after writing more than the command
        // layer keeps, so the push runs to the end and still fails to report.
        let hooks = fixture.repository.join(".git").join("hooks");
        std::fs::create_dir_all(&hooks).expect("hooks directory");
        let script = hooks.join("pre-push");
        std::fs::write(
            &script,
            "#!/bin/sh\nhead -c 34000000 /dev/zero >&2\nexit 0\n",
        )
        .expect("write hook");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("make the hook executable");
        }

        let result = execute(&plan, None).expect("an unreported push is not a refusal");
        let remote = result.remote.as_ref().expect("remote step");
        assert!(remote.unconfirmed, "{}", remote.output);
        assert!(remote.output.contains("more than"), "{}", remote.output);
        assert!(
            !fixture.remote_has_branch("noisy"),
            "the push did delete it"
        );
    }

    #[test]
    fn an_interrupted_local_deletion_is_unconfirmed_and_leaves_the_remote_alone() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["switch", "--create", "slow"]);
        git(
            &fixture.repository,
            &["push", "--set-upstream", "origin", "slow"],
        );
        git(&fixture.repository, &["switch", "main"]);
        let plan = fixture.plan("refs/heads/slow", true, true);
        let marker = fixture.root.join("deletion-started");
        blocking_hook(&fixture.repository, "reference-transaction", &marker);

        let result = cancelled_when(&marker, || execute(&plan, Some("slow")))
            .expect("an interrupted deletion is not a refusal");
        assert_eq!(
            result.message,
            "Repola could not confirm whether local branch slow was deleted. Remote branch origin/slow was not deleted."
        );
        let local = result.local.as_ref().expect("local step");
        assert!(local.unconfirmed && !local.succeeded);
        assert!(result.remote.is_none());
        assert!(fixture.remote_has_branch("slow"));
    }

    #[test]
    fn a_remote_branch_someone_else_deleted_is_reported_as_already_deleted() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "gone"]);
        git(&fixture.repository, &["push", "origin", "gone"]);
        let plan = fixture.plan("refs/remotes/origin/gone", false, true);
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);

        let peer = fixture.peer();
        git(&peer, &["push", "origin", "--delete", "gone"]);

        let result = execute(&plan, None).expect("the remote branch is gone either way");
        assert_eq!(
            result.message,
            "Remote branch origin/gone was already deleted."
        );
        let remote = result.remote.as_ref().expect("remote step");
        assert!(remote.succeeded);
        assert!(remote.warning.is_none(), "{:?}", remote.warning);
        // As a deleting push would, the stale remote-tracking ref goes too.
        assert!(!fixture.has_ref("refs/remotes/origin/gone"));
        let tip = plan
            .remote
            .as_ref()
            .expect("remote details")
            .expected_oid
            .clone();
        let restore = ["update-ref", "refs/remotes/origin/gone", tip.as_str()];
        assert_eq!(
            remote.recovery_commands,
            vec![git_command_line(Path::new(&plan.worktree_path), restore)]
        );
        git(&fixture.repository, &restore);
        assert!(fixture.has_ref("refs/remotes/origin/gone"));
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
        assert!(remote.recovery_commands.is_empty());
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
            .warnings
            .iter()
            .any(|warning| warning.contains("default branch")));
        let error = execute(&plan, None).expect_err("the remote default branch is protected");
        assert!(error.contains("is the default branch of origin"), "{error}");
        assert!(fixture.remote_has_branch("main"));
    }

    #[test]
    fn a_stale_local_default_branch_record_does_not_block_the_deletion() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "former"]);
        git(&fixture.repository, &["push", "origin", "former"]);
        // The last fetch recorded `former` as the default; the remote says `main`.
        git(
            &fixture.repository,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/former",
            ],
        );
        let plan = fixture.plan("refs/remotes/origin/former", false, true);
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
        assert!(
            plan.remote
                .as_ref()
                .expect("remote details")
                .is_remote_default_branch
        );

        execute(&plan, None).expect("the remote says former is not its default");
        assert!(!fixture.remote_has_branch("former"));
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
    fn the_default_branch_is_checked_where_the_deletion_pushes() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "topic"]);
        git(&fixture.repository, &["push", "origin", "topic"]);
        let mirror = fixture.root.join("mirror.git");
        git(
            &fixture.root,
            &["clone", "--bare", path(&fixture.remote), path(&mirror)],
        );
        git(&mirror, &["symbolic-ref", "HEAD", "refs/heads/topic"]);
        git(
            &fixture.repository,
            &["config", "remote.origin.pushurl", path(&mirror)],
        );
        let plan = fixture.plan("refs/remotes/origin/topic", false, true);
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);

        let error = execute(&plan, None).expect_err("topic is the push destination's default");
        assert!(error.contains("default branch"), "{error}");
        assert!(error.contains(path(&mirror)), "{error}");
        let kept = command::git_at(
            &mirror,
            ["show-ref", "--verify", "--quiet", "refs/heads/topic"],
        )
        .expect("show-ref on the push destination");
        assert!(kept.status.success());
    }

    #[test]
    fn a_review_is_refused_once_more_commits_would_become_unreachable() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["switch", "--create", "feature"]);
        commit(&fixture.repository, "also kept by another branch");
        git(&fixture.repository, &["branch", "keeper"]);
        commit(&fixture.repository, "only on feature");
        git(&fixture.repository, &["switch", "main"]);
        let plan = fixture.plan("refs/heads/feature", true, false);
        assert!(plan.requires_force);
        assert_eq!(
            plan.local
                .as_ref()
                .expect("local details")
                .exclusive_commit_count,
            1
        );

        // Nothing about feature itself changes, but deleting it would now lose
        // a commit the review never reported.
        git(
            &fixture.repository,
            &["branch", "--delete", "--force", "keeper"],
        );
        let error = execute(&plan, Some("feature")).expect_err("the loss grew after review");
        assert!(
            error.contains("changed after this deletion was reviewed"),
            "{error}"
        );
        assert!(fixture.has_ref("refs/heads/feature"));
    }

    #[test]
    fn only_a_pull_request_the_review_did_not_show_refuses_the_deletion() {
        let keys = |keys: &[&str]| keys.iter().map(|key| key.to_string()).collect::<Vec<_>>();
        let unreviewed = |reviewed: Option<&[&str]>, current: Option<&[&str]>| {
            unreviewed_pull_requests(reviewed.map(keys).as_deref(), current.map(keys).as_deref())
        };
        // One that opened since the review was never shown.
        assert_eq!(
            unreviewed(Some(&["octo/app#1"]), Some(&["octo/app#1", "octo/app#2"])),
            ["octo/app#2"]
        );
        // Numbers repeat across repositories: the same number elsewhere is new.
        assert_eq!(
            unreviewed(Some(&["octo/app#1"]), Some(&["upstream/app#1"])),
            ["upstream/app#1"]
        );
        // The review could not ask, so every open one is new.
        assert_eq!(unreviewed(None, Some(&["octo/app#3"])), ["octo/app#3"]);
        // Closed since, or nothing open: nothing unreviewed.
        assert!(unreviewed(Some(&["octo/app#1", "octo/app#2"]), Some(&["octo/app#1"])).is_empty());
        assert!(unreviewed(None, Some(&[])).is_empty());
        // The provider cannot be asked now; the review's answer stands.
        assert!(unreviewed(Some(&["octo/app#1"]), None).is_empty());
    }

    #[test]
    fn a_remote_without_a_provider_reports_no_pull_request_check() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "topic"]);
        git(&fixture.repository, &["push", "origin", "topic"]);
        let plan = fixture.plan("refs/remotes/origin/topic", false, true);
        let remote = plan.remote.as_ref().expect("remote details");
        assert_eq!(remote.pull_requests, Some(BranchPullRequests::Unsupported));
        assert_eq!(plan.fingerprint.pull_requests, None);
        assert_eq!(plan.confirmation, BranchDeletionConfirmation::Confirm);

        // A local-only deletion does not ask.
        git(
            &fixture.repository,
            &["branch", "--set-upstream-to", "origin/topic", "topic"],
        );
        let plan = fixture.plan("refs/heads/topic", true, false);
        assert_eq!(
            plan.remote.as_ref().expect("remote details").pull_requests,
            None
        );
    }

    #[test]
    fn more_pull_requests_than_listed_after_a_complete_review_refuse_the_deletion() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "topic"]);
        git(&fixture.repository, &["push", "origin", "topic"]);
        let plan = fixture.plan("refs/remotes/origin/topic", false, true);
        let fingerprint = |keys: &[&str], more: bool| BranchDeletionFingerprint {
            pull_requests: Some(keys.iter().map(|key| key.to_string()).collect()),
            more_pull_requests: more,
            ..plan.fingerprint.clone()
        };
        let all = ["octo/app#1", "octo/app#2"];

        assert_eq!(
            pull_request_change(
                &fingerprint(&all, false),
                &fingerprint(&all, false),
                "topic"
            ),
            None
        );
        // The same pull requests come back, but now the provider has more.
        assert!(
            pull_request_change(&fingerprint(&all, false), &fingerprint(&all, true), "topic")
                .is_some_and(|change| change.contains("More open pull requests"))
        );
        // The review already said there were more.
        assert_eq!(
            pull_request_change(&fingerprint(&all, true), &fingerprint(&all, true), "topic"),
            None
        );
        assert!(pull_request_change(
            &fingerprint(&all[..1], false),
            &fingerprint(&all, false),
            "topic"
        )
        .is_some_and(|change| change.contains("Pull request octo/app#2 now uses topic")));
    }

    #[test]
    fn pull_requests_past_those_listed_require_the_typed_name() {
        let checked = |more_than_listed| BranchPullRequests::Checked {
            provider: crate::worktree::models::RemoteProvider::AzureDevOps,
            pulls: Vec::new(),
            more_than_listed,
        };
        assert!(!affects_pull_requests(Some(&checked(false)), None));
        assert!(affects_pull_requests(Some(&checked(true)), None));
        assert!(affects_pull_requests(
            Some(&BranchPullRequests::Unsupported),
            Some(&["octo/app#1".to_string()])
        ));
        assert!(!affects_pull_requests(None, Some(&[])));
    }

    #[test]
    fn pull_requests_shown_in_the_review_keep_requiring_the_typed_name() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "topic"]);
        git(&fixture.repository, &["push", "origin", "topic"]);
        let request = || fixture.request("refs/remotes/origin/topic", false, true);
        let reviewed = review(request(), None).expect("review");
        assert_eq!(
            reviewed.plan.confirmation,
            BranchDeletionConfirmation::Confirm
        );
        // At execution the provider cannot be asked, but the review showed one.
        let executing = review(request(), Some(&["octo/app#42".to_string()])).expect("review");
        assert_eq!(
            executing.plan.confirmation,
            BranchDeletionConfirmation::TypeBranchName
        );
    }

    #[test]
    fn a_remote_with_its_own_push_program_is_not_deleted_from() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["branch", "topic"]);
        git(&fixture.repository, &["push", "origin", "topic"]);
        git(
            &fixture.repository,
            &["config", "remote.origin.receivepack", "git-receive-pack"],
        );
        let plan = fixture.plan("refs/remotes/origin/topic", false, true);
        assert!(plan.remote.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|blocker| blocker.contains("remote.origin.receivepack")),
            "{:?}",
            plan.blockers
        );
        execute(&plan, None).expect_err("the push program cannot be checked");
        assert!(fixture.remote_has_branch("topic"));
    }

    #[test]
    fn a_remote_that_hides_its_head_is_not_deleted_from() {
        let fixture = Fixture::new();
        // Fetching no longer shows HEAD, which still names main.
        git(&fixture.remote, &["config", "uploadpack.hideRefs", "HEAD"]);
        git(
            &fixture.repository,
            &["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"],
        );
        let plan = fixture.plan("refs/remotes/origin/main", false, true);
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);

        let error = execute(&plan, None).expect_err("the default branch cannot be ruled out");
        assert!(
            error.contains("did not show which branch is its default"),
            "{error}"
        );
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
