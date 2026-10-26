use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::operation;

use super::command;
use super::discovery::{inspect_registered_repository, with_inspection_workers, ScanError};
use super::git_metadata::metadata_line;
use super::identity;
use super::inspection::{inspect_worktree_with_status, measure_worktree, worktree_membership};
use super::models::{
    ChangeSummary, IntegrationKind, MeasureWorktreeRequest, RefreshError, RefreshRequest,
    RefreshResult, RepositoryContext, RepositoryGitSignature, RepositorySummary,
    WorkingCopySnapshot, WorktreeMeasurement, WorktreeRecord, WorktreeSeed,
};
use super::working_copy::{
    detect_operation_at_git_dir, parse_status, stamp_worktree_entries, working_copy_snapshot,
};

// Reuse immutable HEAD/graph facts across file edits. A content signature of
// refs, effective configuration and registration metadata invalidates them.
// The bound avoids retaining every repository ever opened by this process.
type DerivedCache = BTreeMap<(String, String, String), WorktreeRecord>;

fn derived_cache() -> &'static Mutex<DerivedCache> {
    static CACHE: OnceLock<Mutex<DerivedCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

#[derive(Clone)]
struct CachedWorkingCopyMetadata {
    upstream: Option<String>,
    remote: Option<String>,
    upstream_head: Option<String>,
}

#[derive(Clone)]
struct CachedRepository {
    repository: RepositoryContext,
    seeds: Vec<WorktreeSeed>,
    signature: String,
    identities: BTreeMap<String, String>,
    snapshots: BTreeMap<String, CachedWorkingCopyMetadata>,
}

fn metadata_cache() -> &'static Mutex<BTreeMap<String, CachedRepository>> {
    static CACHE: OnceLock<Mutex<BTreeMap<String, CachedRepository>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub fn refresh_worktrees(request: RefreshRequest) -> Result<RefreshResult, ScanError> {
    // Requests may coalesce file edits from several repositories. Validate
    // targets against their combined cached membership, not each repository
    // separately, while unknown targets still force real enumeration.
    let fast_targets_known = !request.refresh_metadata
        && metadata_cache().lock().ok().is_some_and(|cache| {
            request.worktree_paths.as_ref().is_none_or(|paths| {
                paths.iter().all(|path| {
                    request
                        .repository_paths
                        .iter()
                        .filter_map(|repository| cache.get(repository))
                        .any(|cached| {
                            cached
                                .seeds
                                .iter()
                                .any(|seed| Path::new(&seed.path) == Path::new(path))
                        })
                })
            })
        });
    let mut result = RefreshResult {
        refreshed_at_ms: now_ms(),
        repositories: Vec::new(),
        memberships: Vec::new(),
        worktrees: Vec::new(),
        successful_repository_paths: Vec::new(),
        errors: Vec::new(),
        git_signatures: Vec::new(),
        working_copy: None,
    };
    let mut seen = BTreeSet::new();
    for registered_path in &request.repository_paths {
        check_cancelled()?;
        if fast_targets_known {
            let cached = metadata_cache()
                .lock()
                .ok()
                .and_then(|cache| cache.get(registered_path).cloned());
            if let Some(cached) = cached {
                if seen.contains(&cached.repository.git_dir) {
                    continue;
                }
                if let Some((records, snapshot)) = fast_refresh(&cached, &request) {
                    seen.insert(cached.repository.git_dir);
                    result.worktrees.extend(records);
                    if snapshot.is_some() {
                        result.working_copy = snapshot;
                    }
                    continue;
                }
            }
        }
        let (repository, seeds) = match inspect_registered_repository(registered_path) {
            Ok(value) => value,
            Err(message) => {
                if let Ok(mut cache) = metadata_cache().lock() {
                    cache.remove(registered_path);
                }
                result.errors.push(RefreshError {
                    repository_path: registered_path.clone(),
                    worktree_path: None,
                    message,
                });
                continue;
            }
        };
        let repository_path = repository.path.to_string_lossy().into_owned();
        if !seen.insert(repository.git_dir.clone()) {
            continue;
        }
        result
            .successful_repository_paths
            .push(repository_path.clone());
        let signature = match git_signature(&repository, &seeds) {
            Ok(signature) => {
                result.git_signatures.push(RepositoryGitSignature {
                    repository_path: repository_path.clone(),
                    signature: signature.clone(),
                });
                Some(signature)
            }
            Err(message) => {
                if let Ok(mut cache) = metadata_cache().lock() {
                    cache.remove(registered_path);
                    cache.remove(&repository_path);
                }
                result.errors.push(RefreshError {
                    repository_path: repository_path.clone(),
                    worktree_path: None,
                    message,
                });
                None
            }
        };
        if let Some(signature) = &signature {
            let identities = seeds
                .iter()
                .filter_map(|seed| {
                    worktree_identity(&repository, seed)
                        .ok()
                        .map(|identity| (seed.path.clone(), identity))
                })
                .collect();
            let cached = CachedRepository {
                repository: repository.clone(),
                seeds: seeds.clone(),
                signature: signature.clone(),
                identities,
                snapshots: BTreeMap::new(),
            };
            if let Ok(mut cache) = metadata_cache().lock() {
                for path in [registered_path, &repository_path] {
                    if cache.len() >= 128 && !cache.contains_key(path) {
                        cache.pop_first();
                    }
                    cache.insert(path.clone(), cached.clone());
                }
            }
        }
        if let Some(selected) = request.working_copy.as_ref().filter(|selected| {
            seeds
                .iter()
                .any(|seed| Path::new(&seed.path) == Path::new(&selected.worktree_path))
        }) {
            match working_copy_snapshot(selected.clone()) {
                Ok(snapshot) => {
                    if let Ok(mut cache) = metadata_cache().lock() {
                        for path in [registered_path, &repository_path] {
                            if let Some(cached) = cache.get_mut(path) {
                                cached.snapshots.insert(
                                    snapshot.worktree_path.clone(),
                                    CachedWorkingCopyMetadata {
                                        upstream: snapshot.upstream.clone(),
                                        remote: snapshot.remote.clone(),
                                        upstream_head: snapshot.upstream_head.clone(),
                                    },
                                );
                            }
                        }
                    }
                    result.working_copy = Some(snapshot);
                }
                Err(message) => result.errors.push(RefreshError {
                    repository_path: repository_path.clone(),
                    worktree_path: Some(selected.worktree_path.clone()),
                    message,
                }),
            }
        }
        let start = result.memberships.len();
        for seed in &seeds {
            check_cancelled()?;
            let membership = worktree_membership(&repository, seed);
            let inspect = request.worktree_paths.as_ref().is_none_or(|paths| {
                paths
                    .iter()
                    .any(|path| Path::new(path) == Path::new(&seed.path))
            });
            if inspect {
                let supplied_status = result
                    .working_copy
                    .as_ref()
                    .filter(|snapshot| Path::new(&snapshot.worktree_path) == Path::new(&seed.path))
                    .map(|snapshot| {
                        let mut status = ChangeSummary {
                            available: true,
                            ..Default::default()
                        };
                        for change in &snapshot.changes {
                            if change.ignored {
                                continue;
                            }
                            status.total += 1;
                            status.staged += usize::from(change.staged);
                            status.unstaged += usize::from(change.unstaged);
                            status.untracked += usize::from(change.untracked);
                            status.conflicted += usize::from(change.conflicted);
                        }
                        status
                    });
                let key = signature.as_ref().map(|signature| {
                    (
                        repository_path.clone(),
                        signature.clone(),
                        seed.path.clone(),
                    )
                });
                let cached = key
                    .as_ref()
                    .and_then(|key| derived_cache().lock().ok()?.get(key).cloned());
                let token = operation::current_operation();
                let record = with_inspection_workers(|| {
                    operation::with_operation(token, || {
                        inspect_worktree_with_status(
                            &repository,
                            seed,
                            supplied_status.as_ref(),
                            cached.as_ref(),
                            true,
                        )
                    })
                });
                if record.exists && !record.status.available {
                    result.errors.push(RefreshError {
                        repository_path: repository_path.clone(),
                        worktree_path: Some(record.path.clone()),
                        message: "Git status is unavailable for this working copy.".into(),
                    });
                }
                if record.status.available
                    && (record.head.is_none() || record.head_subject.is_some())
                    && (repository.remote_url.is_none() || record.unpushed_commit_count.is_some())
                    && (repository.default_target.is_none()
                        || record.is_primary
                        || record.integration.kind != IntegrationKind::Unknown)
                {
                    if let Some(key) = key {
                        if let Ok(mut cache) = derived_cache().lock() {
                            if cache.len() >= 512 {
                                cache.pop_first();
                            }
                            cache.insert(key, record.clone());
                        }
                    }
                }
                result.worktrees.push(record);
            }
            result.memberships.push(membership);
        }
        let members = &result.memberships[start..];
        result.repositories.push(RepositorySummary {
            id: identity::repository_id(
                &dunce::canonicalize(&repository.git_dir)
                    .unwrap_or_else(|_| repository.git_dir.clone()),
            ),
            name: repository.name,
            path: repository_path,
            remote_url: repository.remote_url,
            provider: repository.provider,
            worktree_count: members.len(),
            attention_count: members
                .iter()
                .filter(|record| !record.exists || record.registration.reason.is_some())
                .count(),
            conflicted_count: 0,
            allocated_bytes: 0,
            allocation_incomplete: true,
        });
    }
    check_cancelled()?;
    Ok(result)
}

pub fn measure_worktree_directory(
    request: MeasureWorktreeRequest,
) -> Result<WorktreeMeasurement, String> {
    let (repository, seeds) = inspect_registered_repository(&request.repository_path)?;
    let seed = seeds
        .iter()
        .find(|seed| Path::new(&seed.path) == Path::new(&request.worktree_path))
        .ok_or_else(|| "The worktree is not registered in this repository.".to_string())?;
    let record = worktree_membership(&repository, seed);
    let measurement = measure_worktree(&record);
    Ok(WorktreeMeasurement {
        repository_path: repository.path.to_string_lossy().into_owned(),
        worktree_path: seed.path.clone(),
        size_bytes: measurement.as_ref().map(|value| value.allocated_bytes),
        size_incomplete: measurement.as_ref().is_some_and(|value| value.incomplete),
        last_activity_at_ms: measurement.and_then(|value| value.latest_modified_at_ms),
    })
}

/// Cached metadata is an optimization, never a new membership observation.
/// Validate exact administration pointers and the status-reported identity on
/// both sides of the read before returning a record from that generation.
fn fast_refresh(
    cached: &CachedRepository,
    request: &RefreshRequest,
) -> Option<(Vec<WorktreeRecord>, Option<WorkingCopySnapshot>)> {
    let repository = &cached.repository;
    let mut records = Vec::new();
    let mut selected_snapshot = None;
    for seed in &cached.seeds {
        if operation::is_cancelled() {
            return None;
        }
        if request.worktree_paths.as_ref().is_some_and(|paths| {
            !paths
                .iter()
                .any(|path| Path::new(path) == Path::new(&seed.path))
        }) {
            continue;
        }
        let identity = cached.identities.get(&seed.path)?;
        if &worktree_identity(repository, seed).ok()? != identity {
            return None;
        }
        let derived = derived_cache()
            .lock()
            .ok()?
            .get(&(
                repository.path.to_string_lossy().into_owned(),
                cached.signature.clone(),
                seed.path.clone(),
            ))
            .cloned()?;
        let selected = request
            .working_copy
            .as_ref()
            .filter(|selected| Path::new(&selected.worktree_path) == Path::new(&seed.path));
        let previous_snapshot = match selected {
            Some(selected) => {
                if Path::new(&selected.repository_path) != repository.path {
                    return None;
                }
                Some(cached.snapshots.get(&seed.path)?)
            }
            None => None,
        };
        let snapshot_start = SystemTime::now();
        let token = operation::current_operation();
        let output = with_inspection_workers(|| {
            operation::with_operation(token, || {
                command::successful_inspection_git_at(
                    Path::new(&seed.path),
                    [
                        "status",
                        "--porcelain=v2",
                        "--branch",
                        "-z",
                        if selected.is_some() {
                            "--untracked-files=all"
                        } else {
                            "--untracked-files=normal"
                        },
                    ],
                )
            })
        })
        .ok()?;
        let mut snapshot = parse_status(&output.stdout).ok()?;
        if snapshot.head != seed.head
            || snapshot.branch != seed.branch
            || &worktree_identity(repository, seed).ok()? != identity
        {
            return None;
        }
        let summary = status_summary(&snapshot);
        if let Some(previous) = previous_snapshot {
            if snapshot.upstream != previous.upstream {
                return None;
            }
            stamp_worktree_entries(Path::new(&seed.path), snapshot_start, &mut snapshot.changes);
            snapshot.repository_path = repository.path.to_string_lossy().into_owned();
            snapshot.worktree_path = seed.path.clone();
            snapshot.remote = previous.remote.clone();
            snapshot.upstream_head = previous.upstream_head.clone();
            snapshot.operation =
                detect_operation_at_git_dir(&worktree_admin_directory(repository, seed).ok()?);
            selected_snapshot = Some(snapshot);
        }
        records.push(inspect_worktree_with_status(
            repository,
            seed,
            Some(&summary),
            Some(&derived),
            true,
        ));
    }
    Some((records, selected_snapshot))
}

fn status_summary(snapshot: &WorkingCopySnapshot) -> ChangeSummary {
    let mut summary = ChangeSummary {
        available: true,
        ..Default::default()
    };
    for change in &snapshot.changes {
        if change.ignored {
            continue;
        }
        summary.total += 1;
        summary.staged += usize::from(change.staged);
        summary.unstaged += usize::from(change.unstaged);
        summary.untracked += usize::from(change.untracked);
        summary.conflicted += usize::from(change.conflicted);
    }
    summary
}

fn worktree_admin_directory(
    repository: &RepositoryContext,
    seed: &WorktreeSeed,
) -> Result<std::path::PathBuf, String> {
    let root = Path::new(&seed.path);
    if !root.is_dir() {
        return Err("The worktree directory is unavailable.".into());
    }
    let pointer = root.join(".git");
    let admin = if pointer.is_dir() {
        pointer.clone()
    } else {
        let mut value = String::new();
        fs::File::open(&pointer)
            .map_err(|error| error.to_string())?
            .take(32 * 1024 + 1)
            .read_to_string(&mut value)
            .map_err(|error| error.to_string())?;
        if value.len() > 32 * 1024 {
            return Err("The worktree pointer is too long.".into());
        }
        let raw = metadata_line(&value)
            .strip_prefix("gitdir: ")
            .ok_or_else(|| "The worktree pointer is malformed.".to_string())?;
        root.join(raw)
    };
    let admin = dunce::canonicalize(admin).map_err(|error| error.to_string())?;
    let common = if seed.is_primary {
        admin.clone()
    } else {
        let mut value = String::new();
        fs::File::open(admin.join("commondir"))
            .map_err(|error| error.to_string())?
            .take(32 * 1024 + 1)
            .read_to_string(&mut value)
            .map_err(|error| error.to_string())?;
        if value.len() > 32 * 1024 {
            return Err("The worktree common-directory pointer is too long.".into());
        }
        dunce::canonicalize(admin.join(metadata_line(&value))).map_err(|error| error.to_string())?
    };
    if common != dunce::canonicalize(&repository.git_dir).map_err(|error| error.to_string())? {
        return Err("The worktree's repository pointer changed.".into());
    }
    Ok(admin)
}

fn worktree_identity(
    repository: &RepositoryContext,
    seed: &WorktreeSeed,
) -> Result<String, String> {
    let admin = worktree_admin_directory(repository, seed)?;
    let mut hash = Sha256::new();
    hash_metadata(&mut hash, &Path::new(&seed.path).join(".git"))?;
    for file in ["HEAD", "locked", "gitdir", "commondir", "config.worktree"] {
        hash.update(file.as_bytes());
        hash_metadata(&mut hash, &admin.join(file))?;
    }
    for file in [
        "config",
        "info/attributes",
        "info/exclude",
        "shallow",
        "info/grafts",
    ] {
        hash.update(file.as_bytes());
        hash_metadata(&mut hash, &repository.git_dir.join(file))?;
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn git_signature(repository: &RepositoryContext, seeds: &[WorktreeSeed]) -> Result<String, String> {
    let mut hash = Sha256::new();
    // Git resolves packed refs and alternate ref backends itself; enumerating
    // the ref files would miss both packed and reftable changes.
    for args in [
        vec![
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(symref)",
        ],
        vec!["config", "--null", "--list", "--show-origin"],
    ] {
        let output = command::successful_inspection_git_at(&repository.path, args)
            .map_err(|error| error.to_string())?;
        hash.update(output.stdout);
        hash.update([0]);
    }
    for seed in seeds {
        hash.update(seed.path.as_bytes());
        hash.update([0]);
        for value in [
            &seed.head,
            &seed.branch,
            &seed.locked_reason,
            &seed.prunable_reason,
        ] {
            hash.update(value.as_deref().unwrap_or_default().as_bytes());
            hash.update([0]);
        }
        hash.update([u8::from(seed.detached), u8::from(seed.is_primary)]);
        hash_metadata(&mut hash, &Path::new(&seed.path).join(".git"))?;
    }
    hash_metadata(&mut hash, &repository.git_dir.join("HEAD"))?;
    for file in ["shallow", "info/grafts", "info/attributes"] {
        hash.update(file.as_bytes());
        hash_metadata(&mut hash, &repository.git_dir.join(file))?;
    }
    // These are Git administration directories, never working directories or
    // objects. Only known, small registration/configuration files are read.
    if let Ok(entries) = fs::read_dir(repository.git_dir.join("worktrees")) {
        let mut paths: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
        paths.sort();
        for path in paths {
            for file in ["HEAD", "gitdir", "commondir", "locked", "config.worktree"] {
                hash.update(
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .as_bytes(),
                );
                hash.update(file.as_bytes());
                hash_metadata(&mut hash, &path.join(file))?;
                hash.update([0]);
            }
        }
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn hash_metadata(hash: &mut Sha256, path: &Path) -> Result<(), String> {
    // A primary .git is a directory; its HEAD/refs are hashed separately.
    if path.is_dir() {
        hash.update(b"directory\0");
        return Ok(());
    }
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            hash.update(b"missing\0");
            return Ok(());
        }
        Err(error) => {
            return Err(format!(
                "Could not read Git metadata {}: {error}",
                path.display()
            ))
        }
    };
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read Git metadata {}: {error}", path.display()))?;
    if bytes.len() > 1024 * 1024 {
        return Err(format!(
            "Git metadata {} exceeds the inspection limit.",
            path.display()
        ));
    }
    hash.update(b"file\0");
    hash.update(bytes);
    hash.update([0]);
    Ok(())
}

fn check_cancelled() -> Result<(), ScanError> {
    if operation::is_cancelled() {
        Err(ScanError::Cancelled)
    } else {
        Ok(())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::super::models::WorkingCopyRequest;
    use super::*;

    fn git(repository: &Path, args: &[&str]) {
        command::successful_git_at(repository, args).expect("fixture Git command");
    }

    fn fixture() -> (tempfile::TempDir, String, String) {
        let temp = tempfile::tempdir().unwrap();
        let repository = temp.path().join("repository");
        fs::create_dir(&repository).unwrap();
        git(&repository, &["init", "-b", "main"]);
        git(
            &repository,
            &["config", "user.email", "repola@example.test"],
        );
        git(&repository, &["config", "user.name", "Repola Test"]);
        git(&repository, &["config", "commit.gpgsign", "false"]);
        fs::write(repository.join("tracked.txt"), "original\n").unwrap();
        git(&repository, &["add", "tracked.txt"]);
        git(&repository, &["commit", "-m", "initial"]);
        let worktree = temp.path().join("linked");
        git(
            &repository,
            &[
                "worktree",
                "add",
                "-b",
                "linked",
                worktree.to_str().unwrap(),
            ],
        );
        let repository = dunce::canonicalize(repository)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let worktree = dunce::canonicalize(worktree)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        (temp, repository, worktree)
    }

    fn request(repository: &str, worktree: Option<&str>) -> RefreshRequest {
        RefreshRequest {
            repository_paths: vec![repository.into()],
            worktree_paths: worktree.map(|path| vec![path.into()]),
            working_copy: None,
            refresh_metadata: true,
        }
    }

    #[test]
    fn targeted_refresh_preserves_complete_membership_without_inspecting_unrelated_status() {
        let (_temp, repository, linked) = fixture();
        fs::write(Path::new(&repository).join("unrelated.txt"), "dirty\n").unwrap();
        fs::write(Path::new(&linked).join("tracked.txt"), "changed\n").unwrap();
        let result = refresh_worktrees(request(&repository, Some(&linked))).unwrap();
        assert!(result.errors.is_empty());
        assert_eq!(
            result.successful_repository_paths,
            std::slice::from_ref(&repository)
        );
        assert_eq!(result.memberships.len(), 2);
        assert_eq!(result.worktrees.len(), 1);
        assert_eq!(result.worktrees[0].path, linked);
        assert_eq!(result.worktrees[0].status.total, 1);
        assert!(result
            .memberships
            .iter()
            .all(|record| !record.status.available));
        assert!(result
            .worktrees
            .iter()
            .chain(&result.memberships)
            .all(|record| record.size_bytes.is_none()));
        let primary = result
            .memberships
            .iter()
            .find(|record| record.is_primary)
            .unwrap();
        assert_eq!(
            primary.id,
            identity::worktree_id(&result.repositories[0].id, Path::new(&repository))
        );
    }

    #[test]
    fn selected_snapshot_and_refresh_leave_index_untouched() {
        let (_temp, repository, linked) = fixture();
        let context = super::super::discovery::repository_context(Path::new(&linked)).unwrap();
        let admin =
            command::successful_git_at(Path::new(&linked), ["rev-parse", "--absolute-git-dir"])
                .unwrap();
        let index = Path::new(String::from_utf8_lossy(&admin.stdout).trim()).join("index");
        let before = fs::read(&index).unwrap();
        let modified = fs::metadata(&index).unwrap().modified().unwrap();
        fs::write(Path::new(&linked).join("tracked.txt"), "changed\n").unwrap();
        let mut selected = request(&repository, Some(&linked));
        selected.working_copy = Some(WorkingCopyRequest {
            repository_path: repository,
            worktree_path: linked.clone(),
        });
        let result = refresh_worktrees(selected).unwrap();
        assert!(result.errors.is_empty());
        assert_eq!(result.working_copy.unwrap().changes.len(), 1);
        assert_eq!(result.worktrees[0].status.total, 1);
        assert_eq!(fs::read(&index).unwrap(), before);
        assert_eq!(fs::metadata(&index).unwrap().modified().unwrap(), modified);
        assert!(!context.git_dir.join("index.lock").exists());
        assert!(!index.with_extension("lock").exists());
    }

    #[test]
    fn signatures_follow_refs_config_and_registration_but_ignore_working_file_edits() {
        let (_temp, repository, linked) = fixture();
        let signature = || {
            refresh_worktrees(RefreshRequest {
                repository_paths: vec![repository.clone()],
                worktree_paths: Some(vec![]),
                working_copy: None,
                refresh_metadata: true,
            })
            .unwrap()
            .git_signatures[0]
                .signature
                .clone()
        };
        let initial = signature();
        fs::write(Path::new(&linked).join("tracked.txt"), "edit\n").unwrap();
        assert_eq!(signature(), initial);
        git(Path::new(&repository), &["tag", "new-tag"]);
        let tagged = signature();
        assert_ne!(tagged, initial);
        git(Path::new(&repository), &["pack-refs", "--all"]);
        assert_eq!(signature(), tagged);
        git(
            Path::new(&repository),
            &["config", "branch.main.remote", "origin"],
        );
        let configured = signature();
        assert_ne!(configured, tagged);
        fs::write(
            Path::new(&repository)
                .join(".git")
                .join("info")
                .join("attributes"),
            "*.txt diff=text\n",
        )
        .unwrap();
        let attributed = signature();
        assert_ne!(attributed, configured);
        git(
            Path::new(&repository),
            &["worktree", "lock", "--reason", "fixture lock", &linked],
        );
        assert_ne!(signature(), attributed);
    }

    #[test]
    fn membership_detects_external_additions_and_keeps_failed_scope_separate() {
        let (_temp, repository, linked) = fixture();
        let first = refresh_worktrees(request(&repository, Some(&linked))).unwrap();
        let another = Path::new(&repository).parent().unwrap().join("another");
        git(
            Path::new(&repository),
            &[
                "worktree",
                "add",
                "-b",
                "another",
                another.to_str().unwrap(),
            ],
        );
        let mut next = request(&repository, Some(&linked));
        next.repository_paths.push(
            Path::new(&repository)
                .parent()
                .unwrap()
                .join("missing")
                .to_string_lossy()
                .into(),
        );
        let next = refresh_worktrees(next).unwrap();
        assert_eq!(first.memberships.len(), 2);
        assert_eq!(next.memberships.len(), 3);
        assert_eq!(next.worktrees.len(), 1);
        assert_eq!(next.errors.len(), 1);
        assert_eq!(next.successful_repository_paths, [repository]);
    }

    #[test]
    fn measurements_are_explicit_and_validate_membership() {
        let (_temp, repository, linked) = fixture();
        let measured = measure_worktree_directory(MeasureWorktreeRequest {
            repository_path: repository.clone(),
            worktree_path: linked,
        })
        .unwrap();
        assert!(measured.size_bytes.unwrap() > 0);
        assert!(!measured.size_incomplete);
        assert!(measure_worktree_directory(MeasureWorktreeRequest {
            repository_path: repository.clone(),
            worktree_path: Path::new(&repository)
                .join("not-registered")
                .to_string_lossy()
                .into(),
        })
        .is_err());
        let primary = measure_worktree_directory(MeasureWorktreeRequest {
            repository_path: repository.clone(),
            worktree_path: repository,
        })
        .unwrap();
        assert_eq!(primary.size_bytes, None);
    }

    #[test]
    fn graph_facts_invalidate_after_local_commits_and_remote_ref_changes() {
        let (_temp, repository, linked) = fixture();
        let repository_path = Path::new(&repository);
        git(
            repository_path,
            &[
                "config",
                "remote.origin.url",
                "https://example.test/repository",
            ],
        );
        git(
            repository_path,
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
        );
        git(
            repository_path,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        let refresh = || {
            refresh_worktrees(request(&repository, Some(&linked)))
                .unwrap()
                .worktrees
                .remove(0)
        };
        let initial = refresh();
        assert_eq!(initial.unpushed_commit_count, Some(0));
        assert_eq!(initial.integration.kind, IntegrationKind::HeadContained);
        fs::write(Path::new(&linked).join("tracked.txt"), "new commit\n").unwrap();
        git(Path::new(&linked), &["add", "tracked.txt"]);
        git(Path::new(&linked), &["commit", "-m", "next"]);
        let changed = refresh();
        assert_eq!(changed.head_subject.as_deref(), Some("next"));
        assert_eq!(changed.unpushed_commit_count, Some(1));
        assert_eq!(changed.integration.kind, IntegrationKind::HeadNotContained);
        git(
            repository_path,
            &[
                "update-ref",
                "refs/remotes/origin/main",
                changed.head.as_deref().unwrap(),
            ],
        );
        let published = refresh();
        assert_eq!(published.unpushed_commit_count, Some(0));
        assert_eq!(published.integration.kind, IntegrationKind::HeadContained);
    }

    #[test]
    fn membership_reports_broken_pointers_without_inspecting_status() {
        let (_temp, repository, linked) = fixture();
        fs::write(
            Path::new(&linked).join(".git"),
            "gitdir: missing-administration\n",
        )
        .unwrap();
        let result = refresh_worktrees(RefreshRequest {
            repository_paths: vec![repository],
            worktree_paths: Some(vec![]),
            working_copy: None,
            refresh_metadata: true,
        })
        .unwrap();
        let member = result
            .memberships
            .iter()
            .find(|record| record.path == linked)
            .unwrap();
        assert!(matches!(
            member.registration.kind,
            super::super::models::RegistrationKind::BrokenLink
                | super::super::models::RegistrationKind::Prunable
        ));
        assert!(!member.status.available);
        assert!(result.worktrees.is_empty());
    }

    #[test]
    fn cached_file_refresh_reads_only_target_status_and_never_republishes_membership() {
        let (_temp, repository, linked) = fixture();
        let initial = refresh_worktrees(request(&repository, Some(&linked))).unwrap();
        let initial_id = initial.worktrees[0].id.clone();
        for edit in ["first edit\n", "second edit\n"] {
            fs::write(Path::new(&linked).join("tracked.txt"), edit).unwrap();
            let mut next = request(&repository, Some(&linked));
            next.refresh_metadata = false;
            let result = refresh_worktrees(next).unwrap();
            assert!(result.errors.is_empty());
            assert!(result.repositories.is_empty());
            assert!(result.memberships.is_empty());
            assert!(result.successful_repository_paths.is_empty());
            assert!(result.git_signatures.is_empty());
            assert_eq!(result.worktrees.len(), 1);
            assert_eq!(result.worktrees[0].id, initial_id);
            assert_eq!(result.worktrees[0].status.total, 1);
            assert_eq!(result.worktrees[0].head_subject.as_deref(), Some("initial"));
            assert_eq!(result.worktrees[0].size_bytes, None);
        }
    }

    #[test]
    fn cached_selected_snapshot_restamps_files_without_repeating_metadata_reads() {
        let (_temp, repository, linked) = fixture();
        let mut selected = request(&repository, Some(&linked));
        selected.working_copy = Some(WorkingCopyRequest {
            repository_path: repository.clone(),
            worktree_path: linked.clone(),
        });
        let initial = refresh_worktrees(selected.clone()).unwrap();
        assert!(initial.working_copy.unwrap().changes.is_empty());
        let index_output = command::successful_inspection_git_at(
            Path::new(&linked),
            ["rev-parse", "--absolute-git-dir"],
        )
        .unwrap();
        let index_path =
            Path::new(String::from_utf8_lossy(&index_output.stdout).trim()).join("index");
        let original_index = fs::read(&index_path).unwrap();
        let mut last_stamp = None;
        selected.refresh_metadata = false;
        for edit in ["edit\n", "longer next edit\n"] {
            fs::write(Path::new(&linked).join("tracked.txt"), edit).unwrap();
            let result = refresh_worktrees(selected.clone()).unwrap();
            assert!(result.errors.is_empty());
            assert!(result.memberships.is_empty());
            let snapshot = result.working_copy.unwrap();
            assert_eq!(snapshot.changes.len(), 1);
            assert_eq!(snapshot.branch.as_deref(), Some("linked"));
            assert!(snapshot.changes[0].worktree_stamp.is_some());
            assert_ne!(snapshot.changes[0].worktree_stamp, last_stamp);
            last_stamp = snapshot.changes[0].worktree_stamp.clone();
            assert_eq!(fs::read(&index_path).unwrap(), original_index);
        }
    }

    #[test]
    fn cached_file_refresh_falls_back_after_same_oid_branch_checkout() {
        let (_temp, repository, linked) = fixture();
        let initial = refresh_worktrees(request(&repository, Some(&linked))).unwrap();
        git(
            Path::new(&linked),
            &["checkout", "-b", "same-commit-new-branch"],
        );
        let mut next = request(&repository, Some(&linked));
        next.refresh_metadata = false;
        let result = refresh_worktrees(next).unwrap();
        assert!(result.errors.is_empty());
        assert_eq!(result.memberships.len(), 2);
        assert_eq!(result.successful_repository_paths, [repository]);
        assert_eq!(result.worktrees[0].head, initial.worktrees[0].head);
        assert_eq!(
            result.worktrees[0].branch.as_deref(),
            Some("same-commit-new-branch")
        );
        assert_ne!(
            result.git_signatures[0].signature,
            initial.git_signatures[0].signature
        );
    }

    #[test]
    fn omitted_metadata_option_is_authoritative_by_default() {
        let request: RefreshRequest =
            serde_json::from_value(serde_json::json!({ "repositoryPaths": [] })).unwrap();
        assert!(request.refresh_metadata);
        assert!(RefreshRequest::default().refresh_metadata);
    }

    #[test]
    fn refresh_preserves_spaces_in_linked_administration_paths() {
        let (_temp, repository, linked) = fixture();
        let output = command::successful_inspection_git_at(
            Path::new(&linked),
            ["rev-parse", "--absolute-git-dir"],
        )
        .unwrap();
        let admin =
            std::path::PathBuf::from(metadata_line(&String::from_utf8_lossy(&output.stdout)));
        let name = if cfg!(windows) {
            "linked admin with spaces"
        } else {
            "linked admin with trailing space "
        };
        let exact_admin = admin.parent().unwrap().join(name);
        fs::rename(&admin, &exact_admin).unwrap();
        fs::write(
            Path::new(&linked).join(".git"),
            format!("gitdir: {}\n", exact_admin.display()),
        )
        .unwrap();
        let first = refresh_worktrees(request(&repository, Some(&linked))).unwrap();
        assert!(first.errors.is_empty());
        assert_eq!(
            first.worktrees[0].registration.kind,
            super::super::models::RegistrationKind::Healthy
        );
        assert!(first.worktrees[0].status.available);
        fs::write(Path::new(&linked).join("tracked.txt"), "edit\n").unwrap();
        let mut next = request(&repository, Some(&linked));
        next.refresh_metadata = false;
        let next = refresh_worktrees(next).unwrap();
        assert!(next.errors.is_empty());
        assert!(next.memberships.is_empty());
        assert_eq!(next.worktrees[0].status.total, 1);
    }

    #[test]
    fn coalesced_fast_file_targets_reuse_their_own_repository_metadata() {
        let (_first_temp, first_repository, first_linked) = fixture();
        let (_second_temp, second_repository, second_linked) = fixture();
        refresh_worktrees(request(&first_repository, Some(&first_linked))).unwrap();
        refresh_worktrees(request(&second_repository, Some(&second_linked))).unwrap();
        fs::write(Path::new(&first_linked).join("tracked.txt"), "first\n").unwrap();
        fs::write(Path::new(&second_linked).join("tracked.txt"), "second\n").unwrap();
        let result = refresh_worktrees(RefreshRequest {
            repository_paths: vec![first_repository, second_repository],
            worktree_paths: Some(vec![first_linked, second_linked]),
            working_copy: None,
            refresh_metadata: false,
        })
        .unwrap();
        assert!(result.errors.is_empty());
        assert!(result.memberships.is_empty());
        assert!(result.git_signatures.is_empty());
        assert_eq!(result.worktrees.len(), 2);
        assert!(result
            .worktrees
            .iter()
            .all(|record| record.status.total == 1));
    }

    #[test]
    fn a_failed_metadata_read_requires_authoritative_recovery_before_fast_reuse() {
        let (_temp, repository, linked) = fixture();
        refresh_worktrees(request(&repository, Some(&linked))).unwrap();
        let config = Path::new(&repository).join(".git").join("config");
        let original = fs::read(&config).unwrap();
        fs::write(&config, "[malformed\n").unwrap();
        let failed = refresh_worktrees(request(&repository, Some(&linked))).unwrap();
        assert!(!failed.errors.is_empty());
        fs::write(&config, original).unwrap();
        let mut recovered = request(&repository, Some(&linked));
        recovered.refresh_metadata = false;
        let recovered = refresh_worktrees(recovered).unwrap();
        assert!(recovered.errors.is_empty());
        assert_eq!(recovered.memberships.len(), 2);
        assert_eq!(recovered.successful_repository_paths, [repository]);
    }
}
