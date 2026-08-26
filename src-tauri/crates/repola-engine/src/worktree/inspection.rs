use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use walkdir::WalkDir;

use crate::operation;

use super::agents::detect_worktree_origin;
use super::command;
use super::identity;
use super::models::{
    ChangeSummary, IntegrationEvidence, IntegrationKind, RegistrationKind, RegistrationState,
    RepositoryContext, SafetyAssessment, SafetyLevel, WorktreeChanges, WorktreeRecord,
    WorktreeSeed,
};

const MAX_PATCH_BYTES: usize = 1_500_000;

#[derive(Default)]
pub(crate) struct DirectoryMeasurement {
    pub allocated_bytes: u64,
    pub latest_modified_at_ms: Option<u64>,
    pub incomplete: bool,
}

enum GitContext {
    Worktree(PathBuf),
    Explicit {
        git_dir: PathBuf,
        work_tree: PathBuf,
    },
}

impl GitContext {
    fn output(&self, args: &[&str]) -> Result<Output, command::CommandError> {
        match self {
            Self::Worktree(path) => command::git_at(path, args),
            Self::Explicit { git_dir, work_tree } => {
                let mut full_args = vec![
                    OsString::from("--git-dir"),
                    git_dir.as_os_str().to_owned(),
                    OsString::from("--work-tree"),
                    work_tree.as_os_str().to_owned(),
                ];
                full_args.extend(args.iter().map(OsString::from));
                command::output("git", full_args)
            }
        }
    }
}

pub(crate) fn inspect_worktree(
    repository: &RepositoryContext,
    seed: &WorktreeSeed,
) -> WorktreeRecord {
    let mut record = inspect_worktree_shallow(repository, seed);
    if let Some(measurement) = measure_worktree(&record) {
        apply_measurement(&mut record, &measurement);
    }
    record
}

pub(crate) fn inspect_worktree_shallow(
    repository: &RepositoryContext,
    seed: &WorktreeSeed,
) -> WorktreeRecord {
    let path = PathBuf::from(&seed.path);
    let exists = path.is_dir();
    let (git_context, broken_link) = if exists && seed.prunable_reason.is_none() {
        resolve_git_context(repository, seed)
    } else {
        (None, false)
    };

    let status = git_context
        .as_ref()
        .and_then(|context| {
            context
                .output(&["status", "--porcelain=v1", "-z", "--untracked-files=normal"])
                .ok()
        })
        .filter(|output| output.status.success())
        .map(|output| parse_status_porcelain(&output.stdout))
        .unwrap_or_default();
    let unpushed_commit_count = unpushed_commit_count(repository, git_context.as_ref());

    let registration = registration_state(seed, exists, broken_link);
    let integration = integration_evidence(repository, seed);
    let safety = safety_assessment(exists, &status, &registration, unpushed_commit_count);
    let (head_commit_at_ms, head_subject) = head_metadata(repository, seed.head.as_deref());
    let created_at_ms = created_at_ms(&path);
    let last_activity_at_ms = [created_at_ms, head_commit_at_ms]
        .into_iter()
        .flatten()
        .max();

    let git_dir =
        dunce::canonicalize(&repository.git_dir).unwrap_or_else(|_| repository.git_dir.clone());
    let repository_id = identity::repository_id(&git_dir);
    let identity_path = dunce::canonicalize(&path).unwrap_or_else(|_| path.clone());
    WorktreeRecord {
        id: identity::worktree_id(&repository_id, &identity_path),
        repository_name: repository.name.clone(),
        repository_path: repository.path.to_string_lossy().into_owned(),
        path: seed.path.clone(),
        branch: seed.branch.clone(),
        head: seed.head.clone(),
        detached: seed.detached,
        is_primary: seed.is_primary,
        exists,
        created_at_ms,
        head_commit_at_ms,
        last_activity_at_ms,
        head_subject,
        unpushed_commit_count,
        size_bytes: None,
        size_incomplete: false,
        origin: detect_worktree_origin(&path, seed.is_primary),
        status,
        registration,
        integration,
        safety,
    }
}

pub(crate) fn measure_worktree(record: &WorktreeRecord) -> Option<DirectoryMeasurement> {
    if record.exists && !record.is_primary {
        Some(measure_directory(Path::new(&record.path)))
    } else {
        None
    }
}

pub(crate) fn apply_measurement(record: &mut WorktreeRecord, measurement: &DirectoryMeasurement) {
    record.size_bytes = Some(measurement.allocated_bytes);
    record.size_incomplete = measurement.incomplete;
    record.last_activity_at_ms = [
        record.last_activity_at_ms,
        measurement.latest_modified_at_ms,
    ]
    .into_iter()
    .flatten()
    .max();
}

pub fn worktree_changes(
    repository_path: &str,
    worktree_path: &str,
) -> Result<WorktreeChanges, String> {
    let repository = super::discovery::repository_context(Path::new(repository_path))?;
    let seed = super::discovery::list_worktrees(&repository.path)?
        .into_iter()
        .find(|seed| Path::new(&seed.path) == Path::new(worktree_path))
        .ok_or_else(|| "The worktree is not registered in this repository.".to_string())?;
    let (Some(context), _) = resolve_git_context(&repository, &seed) else {
        return Ok(WorktreeChanges {
            available: false,
            patch: String::new(),
            truncated: false,
            untracked: Vec::new(),
            reason: Some("The worktree could not be inspected.".to_string()),
        });
    };

    let diff = context
        .output(&[
            "diff",
            "HEAD",
            "--no-color",
            "--no-ext-diff",
            "--find-renames",
        ])
        .map_err(|error| error.to_string())?;
    if !diff.status.success() {
        return Ok(WorktreeChanges {
            available: false,
            patch: String::new(),
            truncated: false,
            untracked: Vec::new(),
            reason: Some(String::from_utf8_lossy(&diff.stderr).trim().to_string()),
        });
    }
    let (patch, truncated) = truncate_patch(&diff.stdout);

    let untracked = context
        .output(&["ls-files", "--others", "--exclude-standard", "-z"])
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            output
                .stdout
                .split(|byte| *byte == 0)
                .filter(|entry| !entry.is_empty())
                .map(|entry| String::from_utf8_lossy(entry).into_owned())
                .collect()
        })
        .unwrap_or_default();

    Ok(WorktreeChanges {
        available: true,
        patch,
        truncated,
        untracked,
        reason: None,
    })
}

fn truncate_patch(bytes: &[u8]) -> (String, bool) {
    if bytes.len() <= MAX_PATCH_BYTES {
        return (String::from_utf8_lossy(bytes).into_owned(), false);
    }
    let head = &bytes[..MAX_PATCH_BYTES];
    let boundary = head
        .windows(11)
        .rposition(|window| window == b"\ndiff --git")
        .map(|position| position + 1)
        .unwrap_or(0);
    (
        String::from_utf8_lossy(&head[..boundary]).into_owned(),
        true,
    )
}

fn unpushed_commit_count(
    repository: &RepositoryContext,
    git_context: Option<&GitContext>,
) -> Option<u64> {
    repository.remote_url.as_ref()?;
    let output = git_context?
        .output(&["rev-list", "--count", "HEAD", "--not", "--remotes"])
        .ok()
        .filter(|output| output.status.success())?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

fn resolve_git_context(
    repository: &RepositoryContext,
    seed: &WorktreeSeed,
) -> (Option<GitContext>, bool) {
    let path = PathBuf::from(&seed.path);
    if command::git_at(&path, ["rev-parse", "--is-inside-work-tree"])
        .is_ok_and(|output| output.status.success())
    {
        return (Some(GitContext::Worktree(path)), false);
    }

    let Some(admin_dir) = locate_admin_dir(&repository.git_dir, &path) else {
        return (None, false);
    };
    let context = GitContext::Explicit {
        git_dir: admin_dir,
        work_tree: path,
    };
    let valid = context
        .output(&["rev-parse", "--is-inside-work-tree"])
        .is_ok_and(|output| output.status.success());
    if valid {
        (Some(context), true)
    } else {
        (None, false)
    }
}

fn locate_admin_dir(repository_git_dir: &Path, worktree_path: &Path) -> Option<PathBuf> {
    let pointer_path = worktree_path.join(".git");
    if let Ok(pointer) = fs::read_to_string(&pointer_path) {
        if let Some(raw_path) = pointer.trim().strip_prefix("gitdir: ") {
            if let Some(name) = Path::new(raw_path).file_name() {
                let candidate = repository_git_dir.join("worktrees").join(name);
                if candidate.is_dir() {
                    return Some(candidate);
                }
            }
        }
    }

    let expected_pointer = pointer_path.to_string_lossy();
    let entries = fs::read_dir(repository_git_dir.join("worktrees")).ok()?;
    for entry in entries.flatten() {
        let admin_dir = entry.path();
        let Ok(registered_pointer) = fs::read_to_string(admin_dir.join("gitdir")) else {
            continue;
        };
        if registered_pointer.trim() == expected_pointer {
            return Some(admin_dir);
        }
    }
    None
}

pub(crate) fn parse_status_porcelain(bytes: &[u8]) -> ChangeSummary {
    let mut summary = ChangeSummary {
        available: true,
        ..ChangeSummary::default()
    };

    let mut entries = bytes
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty());
    while let Some(entry) = entries.next() {
        if entry.len() < 3 || entry[2] != b' ' {
            continue;
        }
        let x = entry[0] as char;
        let y = entry[1] as char;
        summary.total += 1;
        if x == '?' && y == '?' {
            summary.untracked += 1;
            continue;
        }
        if x != ' ' {
            summary.staged += 1;
        }
        if y != ' ' {
            summary.unstaged += 1;
        }
        if matches!(
            (x, y),
            ('D', 'D')
                | ('A', 'U')
                | ('U', 'D')
                | ('U', 'A')
                | ('D', 'U')
                | ('A', 'A')
                | ('U', 'U')
        ) {
            summary.conflicted += 1;
        }
        if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            let _original_path = entries.next();
        }
    }
    summary
}

pub(crate) fn registration_state(
    seed: &WorktreeSeed,
    exists: bool,
    broken_link: bool,
) -> RegistrationState {
    if seed.is_primary {
        return RegistrationState {
            kind: RegistrationKind::Primary,
            reason: None,
        };
    }
    if let Some(reason) = &seed.locked_reason {
        return RegistrationState {
            kind: RegistrationKind::Locked,
            reason: Some(reason.clone()),
        };
    }
    if let Some(reason) = &seed.prunable_reason {
        return RegistrationState {
            kind: RegistrationKind::Prunable,
            reason: Some(reason.clone()),
        };
    }
    if !exists {
        return RegistrationState {
            kind: RegistrationKind::Missing,
            reason: Some("The registered worktree directory does not exist.".to_string()),
        };
    }
    if broken_link {
        return RegistrationState {
            kind: RegistrationKind::BrokenLink,
            reason: Some("The worktree .git pointer references a moved repository.".to_string()),
        };
    }
    RegistrationState {
        kind: RegistrationKind::Healthy,
        reason: None,
    }
}

fn integration_evidence(
    repository: &RepositoryContext,
    seed: &WorktreeSeed,
) -> IntegrationEvidence {
    if seed.is_primary {
        return IntegrationEvidence {
            kind: IntegrationKind::NotApplicable,
            target: None,
            summary: "Primary worktree".to_string(),
        };
    }
    let (Some(head), Some(target)) = (&seed.head, &repository.default_target) else {
        return IntegrationEvidence {
            kind: IntegrationKind::Unknown,
            target: repository.default_target.clone(),
            summary: "No authoritative integration evidence loaded.".to_string(),
        };
    };

    let result = command::git_at(
        &repository.path,
        [
            "merge-base",
            "--is-ancestor",
            head.as_str(),
            target.as_str(),
        ],
    );
    let display_target = target
        .strip_prefix("refs/remotes/")
        .unwrap_or(target)
        .to_string();
    match result {
        Ok(output) if output.status.success() => IntegrationEvidence {
            kind: IntegrationKind::HeadContained,
            target: Some(display_target.clone()),
            summary: format!("HEAD is contained in {display_target}."),
        },
        Ok(output) if output.status.code() == Some(1) => IntegrationEvidence {
            kind: IntegrationKind::HeadNotContained,
            target: Some(display_target.clone()),
            summary: format!("HEAD is not contained in {display_target}; PR evidence may still show integration."),
        },
        _ => IntegrationEvidence {
            kind: IntegrationKind::Unknown,
            target: Some(display_target),
            summary: "Commit-containment check was unavailable.".to_string(),
        },
    }
}

fn safety_assessment(
    exists: bool,
    status: &ChangeSummary,
    registration: &RegistrationState,
    unpushed_commit_count: Option<u64>,
) -> SafetyAssessment {
    let (level, label, mut reasons) = match registration.kind {
        RegistrationKind::Primary => (
            SafetyLevel::Protected,
            "Primary".to_string(),
            vec!["Primary worktrees cannot be removed with git worktree remove.".to_string()],
        ),
        RegistrationKind::Prunable => (
            SafetyLevel::MetadataOnly,
            "Prune metadata".to_string(),
            vec![registration
                .reason
                .clone()
                .unwrap_or_else(|| "Git marked this registration as prunable.".to_string())],
        ),
        RegistrationKind::Locked => (
            SafetyLevel::Protected,
            "Locked".to_string(),
            vec![registration
                .reason
                .clone()
                .unwrap_or_else(|| "Git has locked this worktree.".to_string())],
        ),
        RegistrationKind::Missing => (
            SafetyLevel::MetadataOnly,
            "Missing".to_string(),
            vec![
                "The directory is already missing; only registration metadata remains.".to_string(),
            ],
        ),
        RegistrationKind::BrokenLink => {
            let mut reasons = vec!["Repair the worktree link before managing it.".to_string()];
            if status.total > 0 {
                reasons.push(format!("It also contains {} changed paths.", status.total));
            }
            (SafetyLevel::Repair, "Repair first".to_string(), reasons)
        }
        RegistrationKind::Healthy if !exists => (
            SafetyLevel::Protected,
            "Unavailable".to_string(),
            vec!["The worktree could not be inspected.".to_string()],
        ),
        RegistrationKind::Healthy if !status.available => (
            SafetyLevel::Protected,
            "Inspection failed".to_string(),
            vec!["Git status was unavailable, so removal is blocked.".to_string()],
        ),
        RegistrationKind::Healthy if status.total > 0 => (
            SafetyLevel::Protected,
            "Local changes".to_string(),
            vec![format!("{} changed paths must be reviewed.", status.total)],
        ),
        RegistrationKind::Healthy => (
            SafetyLevel::Review,
            "Review".to_string(),
            vec!["The worktree is clean; integration and activity still need review.".to_string()],
        ),
    };
    if let Some(count) = unpushed_commit_count.filter(|count| *count > 0) {
        reasons.push(format!(
            "{count} commit{} exist{} only in this clone and {} not on any remote.",
            if count == 1 { "" } else { "s" },
            if count == 1 { "s" } else { "" },
            if count == 1 { "is" } else { "are" },
        ));
    }
    SafetyAssessment {
        level,
        label,
        reasons,
    }
}

fn head_metadata(
    repository: &RepositoryContext,
    head: Option<&str>,
) -> (Option<u64>, Option<String>) {
    let Some(head) = head else {
        return (None, None);
    };
    let Ok(output) = command::git_at(&repository.path, ["show", "-s", "--format=%ct%x00%s", head])
    else {
        return (None, None);
    };
    if !output.status.success() {
        return (None, None);
    }
    let mut fields = output.stdout.splitn(2, |byte| *byte == 0);
    let timestamp = fields
        .next()
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|seconds| seconds.saturating_mul(1_000));
    let subject = fields
        .next()
        .map(|value| String::from_utf8_lossy(value).trim().to_string())
        .filter(|value| !value.is_empty());
    (timestamp, subject)
}

fn created_at_ms(path: &Path) -> Option<u64> {
    let metadata = fs::metadata(path).ok()?;
    let created = metadata.created().or_else(|_| metadata.modified()).ok()?;
    system_time_ms(created)
}

fn system_time_ms(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
}

fn measure_directory(path: &Path) -> DirectoryMeasurement {
    let mut measurement = DirectoryMeasurement::default();
    for entry in WalkDir::new(path).follow_links(false) {
        if operation::is_cancelled() {
            measurement.incomplete = true;
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                measurement.incomplete = true;
                continue;
            }
        };
        let metadata = match fs::symlink_metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(_) => {
                measurement.incomplete = true;
                continue;
            }
        };
        measurement.allocated_bytes = measurement
            .allocated_bytes
            .saturating_add(allocated_bytes(&metadata));
        if let Ok(modified) = metadata.modified() {
            if let Some(modified_at_ms) = system_time_ms(modified) {
                measurement.latest_modified_at_ms = Some(
                    measurement
                        .latest_modified_at_ms
                        .map_or(modified_at_ms, |current| current.max(modified_at_ms)),
                );
            }
        }
    }
    measurement
}

#[cfg(unix)]
fn allocated_bytes(metadata: &fs::Metadata) -> u64 {
    metadata.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
fn allocated_bytes(metadata: &fs::Metadata) -> u64 {
    metadata.len()
}
