//! Recovery points: content Repola saves inside the repository before it
//! discards or overwrites anything in a working copy.
//!
//! A recovery point is a tree named by `refs/repola/discarded/<id>` in the
//! repository's common Git directory. The engine writes it on the machine that
//! owns the working copy, so local and SSH working copies recover the same
//! way and nothing depends on a desktop trash. The tree holds:
//!
//! - `summary.json`: what the recovery list shows.
//! - `manifest.json`: every saved path's index entries and working-tree entry.
//! - `worktree/<path>`: the exact working-tree bytes, read without Git filters.
//! - `index/<stage>/<path>`: the exact index entries.
//!
//! The reference names a tree rather than a commit, so history walks such as
//! `git log --all` never show it, writing it needs no identity or signing, and
//! every saved object stays reachable until the user deletes the point.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io::{BufReader, Read, Seek};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::command;
use super::models::{
    DeleteRecoveryPointsRequest, GitPath, RecoveryFileDiff, RecoveryFileDiffRequest, RecoveryPoint,
    RecoveryPointKind, RecoveryPointReference, RecoveryPointRequest, RecoveryRestoreEntry,
    RecoveryRestorePlan, RecoveryRestoreRequest, RecoveryRestoreResult, RepositoryOperation,
    RestoreEffect, WorkingCopyRequest,
};
use super::working_copy::{
    decode_path_token_bytes, hex, literal_pathspec, null_device, os_string_from_path_bytes,
    truncate_file_patch, working_copy_snapshot,
};

const RECOVERY_REF_NAMESPACE: &str = "refs/repola/discarded";
const MANIFEST_VERSION: u32 = 1;
/// Paths listed in a summary for display; the manifest holds all of them.
const SUMMARY_SAMPLE_PATHS: usize = 20;
const MAX_SUMMARY_BYTES: usize = 64 * 1024;
/// Summaries read per Git invocation: at most 16 MiB, half the capture limit.
const SUMMARIES_PER_READ: usize = 256;
const MAX_RECOVERY_ID_BYTES: usize = 128;
/// Paths per Git invocation stay well inside the Windows command-line limit.
const ARGUMENT_BUDGET_BYTES: usize = 16 * 1024;
/// Either side of a restore preview larger than this is not diffed.
const MAX_PREVIEW_SIDE_BYTES: u64 = 8 * 1024 * 1024;

/// Git's default for `core.symlinks` on this platform: Git for Windows
/// checks symbolic links out as plain files unless configured otherwise.
#[cfg(windows)]
const DEFAULT_CORE_SYMLINKS: bool = false;
#[cfg(not(windows))]
const DEFAULT_CORE_SYMLINKS: bool = true;

/// The observed state of one path: its index entries (every stage when it is
/// conflicted) and what is on disk at that path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PathState {
    pub path: GitPath,
    pub index: Vec<IndexEntry>,
    pub worktree: Option<WorktreeEntry>,
    /// A directory occupies the path. Git sees no entry there and reports the
    /// directory's contents as changes of their own, so it is never saved,
    /// replaced, or removed; it is still fingerprinted, so a directory that
    /// appears after a review invalidates it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub directory: bool,
}

impl PathState {
    fn has_content(&self) -> bool {
        !self.index.is_empty() || self.worktree.is_some()
    }

    /// Whether restoring `saved` over this state changes anything. A
    /// directory is never restored, so only index entries and the working-tree
    /// entry count.
    fn differs_from(&self, saved: &PathState) -> bool {
        self.index != saved.index || self.worktree != saved.worktree
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct IndexEntry {
    pub mode: String,
    pub oid: String,
    pub stage: u8,
    /// Recorded with `git add --intent-to-add`. The index stores such an entry
    /// as an empty blob, so only this flag tells it apart from a staged empty
    /// file.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub intent_to_add: bool,
    /// Set with `git update-index --assume-unchanged`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub assume_unchanged: bool,
    /// Set with `git update-index --skip-worktree`, as sparse checkouts do.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skip_worktree: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorktreeEntry {
    pub kind: WorktreeEntryKind,
    /// The blob ID of the exact bytes on disk (for a symbolic link, of its
    /// target), hashed without Git's clean filters.
    pub oid: String,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum WorktreeEntryKind {
    File,
    Executable,
    Symlink,
}

impl WorktreeEntryKind {
    fn mode(self) -> &'static str {
        match self {
            Self::File => "100644",
            Self::Executable => "100755",
            Self::Symlink => "120000",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Summary {
    version: u32,
    kind: RecoveryPointKind,
    summary: String,
    created_at: String,
    worktree_path: String,
    head: Option<String>,
    path_count: u64,
    paths: Vec<GitPath>,
    stored_bytes: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    version: u32,
    paths: Vec<PathState>,
}

/// Digest of everything a destructive action depends on: HEAD, any
/// in-progress operation, the exact state of every path it touches, and the
/// reviewed plan when that state alone does not determine it.
pub(super) fn fingerprint<P: Serialize + ?Sized>(
    head: Option<&str>,
    operation: Option<RepositoryOperation>,
    states: &[PathState],
    plan: &P,
) -> Result<String, String> {
    #[derive(Serialize)]
    struct Fingerprinted<'a, P: ?Sized> {
        head: Option<&'a str>,
        operation: Option<RepositoryOperation>,
        paths: &'a [PathState],
        plan: &'a P,
    }
    let bytes = serde_json::to_vec(&Fingerprinted {
        head,
        operation,
        paths: states,
        plan,
    })
    .map_err(|error| format!("Could not fingerprint the working copy: {error}"))?;
    Ok(hex(Sha256::digest(&bytes).as_slice()))
}

/// Working-tree bytes a recovery point adds to the object store. Content that
/// matches the path's index entry is already stored.
pub(super) fn stored_bytes(states: &[PathState]) -> u64 {
    states
        .iter()
        .filter_map(|state| {
            state
                .worktree
                .as_ref()
                .filter(|entry| !state.index.iter().any(|index| index.oid == entry.oid))
        })
        .map(|entry| entry.size)
        .sum()
}

/// A Git path checked for use inside a working tree: relative, built only
/// from normal components, and never naming Git's own directory.
struct WorktreePath {
    bytes: Vec<u8>,
    components: Vec<OsString>,
}

impl WorktreePath {
    fn new(path: &GitPath) -> Result<Self, String> {
        let bytes = decode_path_token_bytes(&path.token)?;
        let unsafe_path = || format!("{} is not a safe path inside a working copy.", path.display);
        if bytes.is_empty() || bytes.contains(&0) {
            return Err(unsafe_path());
        }
        let mut components = Vec::new();
        for component in bytes.split(|byte| *byte == b'/') {
            if component.is_empty()
                || component == b"."
                || component == b".."
                || names_git_directory(component)
                || has_platform_separator(component)
            {
                return Err(unsafe_path());
            }
            components.push(os_string_from_path_bytes(component.to_vec())?);
        }
        Ok(Self { bytes, components })
    }

    fn relative(&self) -> PathBuf {
        self.components.iter().collect()
    }
}

/// Matches the names Git refuses as path components: `.git` in any case, with
/// the trailing dots and spaces Windows ignores, and its short-name alias.
fn names_git_directory(component: &[u8]) -> bool {
    let end = component
        .iter()
        .rposition(|byte| !matches!(byte, b'.' | b' '))
        .map_or(0, |index| index + 1);
    let trimmed = &component[..end];
    trimmed.eq_ignore_ascii_case(b".git") || trimmed.eq_ignore_ascii_case(b"git~1")
}

#[cfg(windows)]
fn has_platform_separator(component: &[u8]) -> bool {
    component.iter().any(|byte| matches!(byte, b'\\' | b':'))
}

#[cfg(not(windows))]
fn has_platform_separator(_component: &[u8]) -> bool {
    false
}

/// Joins `path` onto the worktree without following any symbolic link.
/// Git sees no entry at a path whose parent is missing, a file, or a link, so
/// inspection reports `None` for those.
fn inspect_path(worktree: &Path, path: &WorktreePath) -> Result<Option<PathBuf>, String> {
    let Some((name, parents)) = path.components.split_last() else {
        return Ok(None);
    };
    Ok(real_directory(worktree, parents)?.map(|mut current| {
        current.push(name);
        current
    }))
}

/// Joins `components` onto the worktree when every one of them is a real
/// directory, never a symbolic link, a file, or missing.
fn real_directory(worktree: &Path, components: &[OsString]) -> Result<Option<PathBuf>, String> {
    let mut current = worktree.to_path_buf();
    for component in components {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(format!(
                    "{} could not be inspected: {error}",
                    current.display()
                ))
            }
        }
    }
    Ok(Some(current))
}

/// Like `inspect_path`, but creates missing parent directories one at a time
/// and refuses any parent that is not a real directory, so a restore can never
/// write through a symbolic link or outside the working copy.
fn prepare_path(worktree: &Path, path: &WorktreePath, display: &str) -> Result<PathBuf, String> {
    let mut current = worktree.to_path_buf();
    let (name, parents) = path
        .components
        .split_last()
        .ok_or_else(|| format!("{display} is not a safe path inside a working copy."))?;
    for component in parents {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(format!(
                    "{display} cannot be restored because {} is not a directory.",
                    current.display()
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|error| {
                    format!("{} could not be created: {error}", current.display())
                })?;
            }
            Err(error) => {
                return Err(format!(
                    "{} could not be inspected: {error}",
                    current.display()
                ))
            }
        }
    }
    current.push(name);
    Ok(current)
}

/// Observes the index entries and working-tree content of `paths`. With
/// `store`, every working-tree blob is also written to the object store, so
/// the oids returned are exactly the content a recovery point will hold.
/// `head` is the commit HEAD names, or `None` on an unborn branch.
pub(super) fn observe_paths(
    worktree: &Path,
    head: Option<&str>,
    paths: &[GitPath],
    store: bool,
) -> Result<Vec<PathState>, String> {
    let validated = paths
        .iter()
        .map(WorktreePath::new)
        .collect::<Result<Vec<_>, _>>()?;
    let mut index = index_entries(worktree, head, &validated)?;

    enum Source {
        File(OsString),
        Link(Vec<u8>),
    }
    let mut observed = Vec::with_capacity(paths.len());
    for (path, valid) in paths.iter().zip(&validated) {
        let entries = index.remove(&valid.bytes).unwrap_or_default();
        let Some(location) = inspect_path(worktree, valid)? else {
            observed.push((entries, None, false));
            continue;
        };
        let metadata = match fs::symlink_metadata(&location) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                observed.push((entries, None, false));
                continue;
            }
            Err(error) => return Err(format!("{} could not be inspected: {error}", path.display)),
        };
        let file_type = metadata.file_type();
        if file_type.is_dir() {
            observed.push((entries, None, true));
            continue;
        }
        let entry = if file_type.is_symlink() {
            let target = fs::read_link(&location)
                .map_err(|error| format!("{} could not be read: {error}", path.display))?;
            let target = link_target_bytes(target, &path.display)?;
            let size = target.len() as u64;
            (WorktreeEntryKind::Symlink, size, Source::Link(target))
        } else if file_type.is_file() {
            let kind = if is_executable(&metadata, &entries) {
                WorktreeEntryKind::Executable
            } else {
                WorktreeEntryKind::File
            };
            (
                kind,
                metadata.len(),
                Source::File(valid.relative().into_os_string()),
            )
        } else {
            return Err(format!(
                "{} is neither a file nor a directory in the working copy, so Repola cannot save it.",
                path.display
            ));
        };
        observed.push((entries, Some(entry), false));
    }

    let files: Vec<OsString> = observed
        .iter()
        .filter_map(|(_, entry, _)| match entry {
            Some((_, _, Source::File(path))) => Some(path.clone()),
            _ => None,
        })
        .collect();
    let mut file_oids = hash_files(worktree, files, store)?.into_iter();
    let mut states = Vec::with_capacity(observed.len());
    for (path, (index, entry, directory)) in paths.iter().zip(observed) {
        let worktree_entry = match entry {
            None => None,
            Some((kind, size, source)) => {
                let oid = match source {
                    Source::File(_) => file_oids
                        .next()
                        .ok_or_else(|| "Git did not hash every working-tree file.".to_string())?,
                    Source::Link(target) => hash_bytes(worktree, &target, store)?,
                };
                Some(WorktreeEntry { kind, oid, size })
            }
        };
        states.push(PathState {
            path: path.clone(),
            index,
            worktree: worktree_entry,
            directory,
        });
    }
    Ok(states)
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata, _index: &[IndexEntry]) -> bool {
    use std::os::unix::fs::PermissionsExt;
    // Git records a file as executable when its owner may execute it.
    metadata.permissions().mode() & 0o100 != 0
}

#[cfg(windows)]
fn is_executable(_metadata: &fs::Metadata, index: &[IndexEntry]) -> bool {
    // Windows has no executable bit; Git keeps the one the index records.
    index
        .iter()
        .any(|entry| entry.stage == 0 && entry.mode == "100755")
}

#[cfg(unix)]
fn link_target_bytes(target: PathBuf, _display: &str) -> Result<Vec<u8>, String> {
    use std::os::unix::ffi::OsStringExt;
    Ok(target.into_os_string().into_vec())
}

#[cfg(windows)]
fn link_target_bytes(target: PathBuf, display: &str) -> Result<Vec<u8>, String> {
    // Git stores link targets with forward slashes on every platform.
    target
        .into_os_string()
        .into_string()
        .map(|target| target.replace('\\', "/").into_bytes())
        .map_err(|_| format!("The link target of {display} is not valid Unicode."))
}

/// Observes only the index entries of `paths`, for changes that leave their
/// working tree untouched. Whatever occupies such a path on disk is not part
/// of the change, so it is neither fingerprinted nor saved.
pub(super) fn observe_index(
    worktree: &Path,
    head: Option<&str>,
    paths: &[GitPath],
) -> Result<Vec<PathState>, String> {
    let validated = paths
        .iter()
        .map(WorktreePath::new)
        .collect::<Result<Vec<_>, _>>()?;
    let mut index = index_entries(worktree, head, &validated)?;
    Ok(paths
        .iter()
        .zip(&validated)
        .map(|(path, valid)| PathState {
            path: path.clone(),
            index: index.remove(&valid.bytes).unwrap_or_default(),
            worktree: None,
            directory: false,
        })
        .collect())
}

fn index_entries(
    worktree: &Path,
    head: Option<&str>,
    paths: &[WorktreePath],
) -> Result<HashMap<Vec<u8>, Vec<IndexEntry>>, String> {
    let base = match head {
        Some(head) => head.to_string(),
        None => empty_tree(worktree)?,
    };
    let wanted: HashSet<&[u8]> = paths.iter().map(|path| path.bytes.as_slice()).collect();
    let arguments = paths
        .iter()
        .map(|path| os_string_from_path_bytes(path.bytes.clone()).map(literal_pathspec))
        .collect::<Result<Vec<_>, _>>()?;
    let mut entries: HashMap<Vec<u8>, Vec<IndexEntry>> = HashMap::new();
    for chunk in argument_chunks(arguments) {
        let intent_to_add = intent_to_add_paths(worktree, &base, &chunk)?;
        let mut args = vec![
            OsString::from("ls-files"),
            OsString::from("--stage"),
            // Prefixes each entry with a tag that carries its flags: `S` for
            // skip-worktree, and lowercase for assume-unchanged.
            OsString::from("-v"),
            OsString::from("-z"),
            OsString::from("--"),
        ];
        args.extend(chunk);
        let output = git_stdout(worktree, args, "read the index entries")?;
        for record in output
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
        {
            let (fields, path) = split_once(record, b'\t')
                .ok_or_else(|| "Git returned a malformed index entry.".to_string())?;
            if !wanted.contains(path) {
                continue;
            }
            let fields = String::from_utf8_lossy(fields);
            let mut fields = fields.split(' ');
            let (Some(tag), Some(mode), Some(oid), Some(stage), None) = (
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
            ) else {
                return Err("Git returned a malformed index entry.".into());
            };
            let stage = stage
                .parse()
                .map_err(|_| "Git returned a malformed index stage.".to_string())?;
            entries.entry(path.to_vec()).or_default().push(IndexEntry {
                mode: mode.to_string(),
                oid: oid.to_string(),
                stage,
                intent_to_add: stage == 0 && intent_to_add.contains(path),
                assume_unchanged: tag.chars().all(|tag| tag.is_ascii_lowercase()),
                skip_worktree: tag.eq_ignore_ascii_case("S"),
            });
        }
    }
    Ok(entries)
}

/// The paths among `pathspecs` whose index entry was recorded with `git add
/// --intent-to-add`, which `ls-files` does not report. Comparing the index with
/// `base` lists such an entry only when asked to show it, whether or not its
/// file is still on disk.
fn intent_to_add_paths(
    worktree: &Path,
    base: &str,
    pathspecs: &[OsString],
) -> Result<HashSet<Vec<u8>>, String> {
    let listed = |visibility: &str| -> Result<HashSet<Vec<u8>>, String> {
        let mut args = [
            "diff-index",
            "--cached",
            "--name-only",
            "-z",
            visibility,
            base,
            "--",
        ]
        .map(OsString::from)
        .to_vec();
        args.extend(pathspecs.iter().cloned());
        let output = git_stdout(worktree, args, "read the intent-to-add entries")?;
        Ok(output
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(<[u8]>::to_vec)
            .collect())
    };
    let visible = listed("--ita-visible-in-index")?;
    let hidden = listed("--ita-invisible-in-index")?;
    Ok(visible.difference(&hidden).cloned().collect())
}

/// The empty tree's object ID in this repository's hash format, which Git
/// knows without storing it.
fn empty_tree(worktree: &Path) -> Result<String, String> {
    let output =
        command::git_at_with_input(worktree, ["hash-object", "-t", "tree", "--stdin"], b"")
            .map_err(|error| error.to_string())?;
    let oid = successful_stdout(output, "name the empty tree")?;
    Ok(String::from_utf8_lossy(&oid).trim().to_string())
}

fn hash_files(worktree: &Path, files: Vec<OsString>, store: bool) -> Result<Vec<String>, String> {
    let mut oids = Vec::with_capacity(files.len());
    for chunk in argument_chunks(files) {
        let expected = chunk.len();
        let mut args = vec![OsString::from("hash-object")];
        if store {
            args.push(OsString::from("-w"));
        }
        args.push(OsString::from("--no-filters"));
        args.push(OsString::from("--"));
        args.extend(chunk);
        let output = git_stdout(worktree, args, "hash the working-tree files")?;
        let hashed: Vec<String> = String::from_utf8_lossy(&output)
            .lines()
            .map(str::to_string)
            .collect();
        if hashed.len() != expected || !hashed.iter().all(|oid| is_object_id(oid)) {
            return Err("Git did not hash every working-tree file.".into());
        }
        oids.extend(hashed);
    }
    Ok(oids)
}

fn hash_bytes(worktree: &Path, bytes: &[u8], store: bool) -> Result<String, String> {
    let mut args = vec!["hash-object"];
    if store {
        args.push("-w");
    }
    args.extend(["--no-filters", "--stdin"]);
    let output =
        command::git_at_with_input(worktree, args, bytes).map_err(|error| error.to_string())?;
    let stdout = successful_stdout(output, "hash the saved content")?;
    let oid = String::from_utf8_lossy(&stdout).trim().to_string();
    if !is_object_id(&oid) {
        return Err("Git returned a malformed object ID.".into());
    }
    Ok(oid)
}

fn argument_chunks(arguments: Vec<OsString>) -> Vec<Vec<OsString>> {
    let mut chunks = Vec::new();
    let mut current = Vec::new();
    let mut size = 0;
    for argument in arguments {
        let length = argument.len() + 1;
        if !current.is_empty() && size + length > ARGUMENT_BUDGET_BYTES {
            chunks.push(std::mem::take(&mut current));
            size = 0;
        }
        size += length;
        current.push(argument);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Saves `states` as a new recovery point. The reference is created only if
/// it does not exist yet, so no recovery point can ever be overwritten.
pub(super) fn store_recovery_point(
    worktree: &Path,
    kind: RecoveryPointKind,
    summary: String,
    head: Option<&str>,
    states: &[PathState],
) -> Result<RecoveryPoint, String> {
    let (stamp, created_at) = utc_timestamps(SystemTime::now());
    let mut record = Summary {
        version: MANIFEST_VERSION,
        kind,
        summary,
        created_at,
        worktree_path: worktree.to_string_lossy().into_owned(),
        head: head.map(str::to_string),
        path_count: states.len() as u64,
        paths: Vec::new(),
        stored_bytes: stored_bytes(states),
    };
    // Sample only as many paths as keep the summary readable by `list`.
    for state in states.iter().take(SUMMARY_SAMPLE_PATHS) {
        record.paths.push(state.path.clone());
        if serde_json::to_vec(&record)
            .map_err(|error| error.to_string())?
            .len()
            > MAX_SUMMARY_BYTES
        {
            record.paths.pop();
            break;
        }
    }
    let manifest = Manifest {
        version: MANIFEST_VERSION,
        paths: states.to_vec(),
    };
    let summary_oid = hash_bytes(
        worktree,
        &serde_json::to_vec(&record).map_err(|error| error.to_string())?,
        true,
    )?;
    let manifest_oid = hash_bytes(
        worktree,
        &serde_json::to_vec(&manifest).map_err(|error| error.to_string())?,
        true,
    )?;
    // Intent-to-add index entries name the empty blob, which Git does not
    // otherwise store; the tree below must only reference stored objects.
    hash_bytes(worktree, b"", true)?;

    let mut input = Vec::new();
    push_index_info(&mut input, "100644", &summary_oid, b"summary.json");
    push_index_info(&mut input, "100644", &manifest_oid, b"manifest.json");
    for state in states {
        let path = decode_path_token_bytes(&state.path.token)?;
        if let Some(entry) = &state.worktree {
            push_index_info(
                &mut input,
                entry.kind.mode(),
                &entry.oid,
                &[b"worktree/".as_slice(), &path].concat(),
            );
        }
        for entry in &state.index {
            push_index_info(
                &mut input,
                &entry.mode,
                &entry.oid,
                &[format!("index/{}/", entry.stage).as_bytes(), &path].concat(),
            );
        }
    }
    let temporary = tempfile::tempdir()
        .map_err(|error| format!("Could not create a temporary Git index: {error}"))?;
    let index_path = temporary.path().join("index");
    let environment = [("GIT_INDEX_FILE", index_path.as_os_str())];
    let output = command::git_at_with_input_and_env(
        worktree,
        ["update-index", "-z", "--index-info"],
        &input,
        environment,
    )
    .map_err(|error| error.to_string())?;
    successful_stdout(output, "assemble the recovery point")?;
    let output = command::git_at_with_env(worktree, ["write-tree"], environment)
        .map_err(|error| error.to_string())?;
    let tree = String::from_utf8_lossy(&successful_stdout(output, "write the recovery point")?)
        .trim()
        .to_string();
    if !is_object_id(&tree) {
        return Err("Git returned a malformed recovery-point tree.".into());
    }

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let id = format!("{RECOVERY_REF_NAMESPACE}/{stamp}-{}", &suffix[..12]);
    create_reference(worktree, &id, &tree)?;
    Ok(point_from_summary(id, tree, record))
}

/// Creates `id` pointing at `tree`. The all-zero old value makes the update
/// create-only, so an existing recovery point can never be overwritten.
fn create_reference(worktree: &Path, id: &str, tree: &str) -> Result<(), String> {
    let absent = "0".repeat(tree.len());
    git_stdout(
        worktree,
        ["update-ref", id, tree, absent.as_str()],
        "create the recovery point",
    )
    .map(|_| ())
}

fn push_index_info(input: &mut Vec<u8>, mode: &str, oid: &str, path: &[u8]) {
    input.extend_from_slice(format!("{mode} {oid}\t").as_bytes());
    input.extend_from_slice(path);
    input.push(0);
}

fn point_from_summary(id: String, oid: String, summary: Summary) -> RecoveryPoint {
    RecoveryPoint {
        id,
        oid,
        kind: summary.kind,
        summary: summary.summary,
        created_at: summary.created_at,
        worktree_path: summary.worktree_path,
        head: summary.head,
        path_count: summary.path_count,
        paths: summary.paths,
        stored_bytes: summary.stored_bytes,
    }
}

/// Lists every recovery point in the repository, newest first. Points saved
/// from every worktree of the repository share one list.
pub fn list_recovery_points(request: WorkingCopyRequest) -> Result<Vec<RecoveryPoint>, String> {
    let snapshot = working_copy_snapshot(request)?;
    list(Path::new(&snapshot.worktree_path))
}

fn list(worktree: &Path) -> Result<Vec<RecoveryPoint>, String> {
    let output = git_stdout(
        worktree,
        [
            "for-each-ref",
            "--format=%(objecttype) %(objectname) %(refname)",
            RECOVERY_REF_NAMESPACE,
        ],
        "list the recovery points",
    )?;
    let mut references = Vec::new();
    for line in String::from_utf8_lossy(&output).lines() {
        let mut fields = line.splitn(3, ' ');
        let (Some(kind), Some(oid), Some(id)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let reference = RecoveryPointReference {
            id: id.to_string(),
            oid: oid.to_string(),
        };
        if kind == "tree" && validate_reference(&reference).is_ok() {
            references.push(reference);
        }
    }
    if references.is_empty() {
        return Ok(Vec::new());
    }
    // Sizes come first so only summaries small enough to use are read, a
    // bounded number at a time, keeping each read within the capture limit.
    let input: String = references
        .iter()
        .map(|reference| format!("{}:summary.json\n", reference.oid))
        .collect();
    let output = command::git_at_with_input(
        worktree,
        [
            "cat-file",
            "--batch-check=%(objectname) %(objecttype) %(objectsize)",
        ],
        input.as_bytes(),
    )
    .map_err(|error| error.to_string())?;
    let sizes = successful_stdout(output, "read the recovery points")?;
    let mut readable = Vec::new();
    for (index, line) in String::from_utf8_lossy(&sizes).lines().enumerate() {
        let mut fields = line.split(' ');
        if let (Some(oid), Some("blob"), Some(size)) = (fields.next(), fields.next(), fields.next())
        {
            if size
                .parse::<usize>()
                .is_ok_and(|size| size <= MAX_SUMMARY_BYTES)
            {
                readable.push((index, oid.to_string()));
            }
        }
    }
    let mut objects = vec![None; references.len()];
    for chunk in readable.chunks(SUMMARIES_PER_READ) {
        let input: String = chunk.iter().map(|(_, oid)| format!("{oid}\n")).collect();
        let output =
            command::git_at_with_input(worktree, ["cat-file", "--batch"], input.as_bytes())
                .map_err(|error| error.to_string())?;
        let read = parse_batch(&successful_stdout(output, "read the recovery points")?)?;
        if read.len() != chunk.len() {
            return Err("Git returned malformed recovery-point data.".into());
        }
        for ((index, _), object) in chunk.iter().zip(read) {
            objects[*index] = object;
        }
    }
    let mut points: Vec<RecoveryPoint> = references
        .into_iter()
        .zip(objects)
        .filter_map(|(reference, object)| {
            let summary: Summary = serde_json::from_slice(object?.as_slice()).ok()?;
            (summary.version == MANIFEST_VERSION)
                .then(|| point_from_summary(reference.id, reference.oid, summary))
        })
        .collect();
    points.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    Ok(points)
}

/// Splits `git cat-file --batch` output into one entry per requested object;
/// missing objects and oversized summaries are `None`.
fn parse_batch(mut output: &[u8]) -> Result<Vec<Option<Vec<u8>>>, String> {
    let malformed = || "Git returned malformed recovery-point data.".to_string();
    let mut objects = Vec::new();
    while !output.is_empty() {
        let (header, rest) = split_once(output, b'\n').ok_or_else(malformed)?;
        let header = String::from_utf8_lossy(header);
        if header.ends_with(" missing") || header.ends_with(" ambiguous") {
            objects.push(None);
            output = rest;
            continue;
        }
        let size: usize = header
            .rsplit(' ')
            .next()
            .and_then(|size| size.parse().ok())
            .ok_or_else(malformed)?;
        if rest.len() < size + 1 {
            return Err(malformed());
        }
        objects.push(
            (size <= MAX_SUMMARY_BYTES && header.split(' ').nth(1) == Some("blob"))
                .then(|| rest[..size].to_vec()),
        );
        output = &rest[size + 1..];
    }
    Ok(objects)
}

fn validate_reference(reference: &RecoveryPointReference) -> Result<(), String> {
    let name = reference
        .id
        .strip_prefix(RECOVERY_REF_NAMESPACE)
        .and_then(|name| name.strip_prefix('/'))
        .ok_or_else(|| "The selected recovery point reference is invalid.".to_string())?;
    if name.is_empty()
        || reference.id.len() > MAX_RECOVERY_ID_BYTES
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || !is_object_id(&reference.oid)
    {
        return Err("The selected recovery point reference is invalid.".into());
    }
    Ok(())
}

fn is_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Loads a reviewed recovery point, refusing if its reference moved, and
/// verifies its manifest against the saved tree so a damaged or crafted point
/// cannot name content or paths it does not hold.
fn load_point(
    worktree: &Path,
    reference: &RecoveryPointReference,
) -> Result<(RecoveryPoint, Vec<PathState>), String> {
    validate_reference(reference)?;
    let output = command::git_at(
        worktree,
        [
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            reference.id.as_str(),
        ],
    )
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err("The recovery point no longer exists. Refresh and try again.".into());
    }
    if String::from_utf8_lossy(&output.stdout).trim() != reference.oid {
        return Err(
            "The recovery point changed after it was reviewed. Refresh and try again.".into(),
        );
    }
    let summary_bytes = git_stdout(
        worktree,
        [
            "cat-file",
            "blob",
            &format!("{}:summary.json", reference.oid),
        ],
        "read the recovery point",
    )?;
    let summary: Summary = serde_json::from_slice(&summary_bytes)
        .map_err(|_| "The recovery point summary is unreadable.".to_string())?;
    let manifest: Manifest = serde_json::from_reader(BufReader::new(git_to_temporary_file(
        worktree,
        [
            "cat-file",
            "blob",
            &format!("{}:manifest.json", reference.oid),
        ],
        "read the recovery point",
    )?))
    .map_err(|_| "The recovery point manifest is unreadable.".to_string())?;
    if summary.version != MANIFEST_VERSION || manifest.version != MANIFEST_VERSION {
        return Err("This recovery point was saved by a newer version of Repola.".into());
    }
    verify_manifest(worktree, &reference.oid, &manifest.paths)?;
    Ok((
        point_from_summary(reference.id.clone(), reference.oid.clone(), summary),
        manifest.paths,
    ))
}

fn verify_manifest(worktree: &Path, tree: &str, states: &[PathState]) -> Result<(), String> {
    let damaged = || {
        "The recovery point is damaged: its manifest does not match its saved content.".to_string()
    };
    let mut listing = Vec::new();
    git_to_temporary_file(
        worktree,
        ["ls-tree", "-r", "-z", "--full-tree", tree],
        "read the recovery point",
    )?
    .read_to_end(&mut listing)
    .map_err(|error| error.to_string())?;
    let mut entries: HashMap<&[u8], (&str, &str)> = HashMap::new();
    for record in listing
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let (fields, path) = split_once(record, b'\t').ok_or_else(damaged)?;
        let fields = std::str::from_utf8(fields).map_err(|_| damaged())?;
        let mut fields = fields.split(' ');
        let (Some(mode), Some(_), Some(oid)) = (fields.next(), fields.next(), fields.next()) else {
            return Err(damaged());
        };
        entries.insert(path, (mode, oid));
    }
    let mut seen = HashSet::new();
    for state in states {
        WorktreePath::new(&state.path)?;
        if !seen.insert(state.path.token.as_str()) {
            return Err(damaged());
        }
        let path = decode_path_token_bytes(&state.path.token)?;
        if let Some(entry) = &state.worktree {
            let key = [b"worktree/".as_slice(), &path].concat();
            if entries.get(key.as_slice()) != Some(&(entry.kind.mode(), entry.oid.as_str())) {
                return Err(damaged());
            }
        }
        for entry in &state.index {
            let key = [format!("index/{}/", entry.stage).as_bytes(), &path].concat();
            if entry.stage > 3
                || !matches!(
                    entry.mode.as_str(),
                    "100644" | "100755" | "120000" | "160000"
                )
                || entries.get(key.as_slice()) != Some(&(entry.mode.as_str(), entry.oid.as_str()))
            {
                return Err(damaged());
            }
        }
    }
    Ok(())
}

pub fn plan_recovery_restore(request: RecoveryPointRequest) -> Result<RecoveryRestorePlan, String> {
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path,
        worktree_path: request.worktree_path,
    })?;
    if snapshot.operation.is_some() {
        return Err(
            "Finish or abort the in-progress Git operation before restoring discarded changes."
                .into(),
        );
    }
    let worktree = Path::new(&snapshot.worktree_path);
    let (point, saved) = load_point(worktree, &request.point)?;
    let paths: Vec<GitPath> = saved.iter().map(|state| state.path.clone()).collect();
    let current = observe_paths(worktree, snapshot.head.as_deref(), &paths, false)?;
    refuse_replacing_directories(&saved, &current)?;
    let entries = saved
        .iter()
        .zip(&current)
        .map(|(saved, current)| RecoveryRestoreEntry {
            path: saved.path.clone(),
            worktree: restore_effect(saved.worktree.as_ref(), current.worktree.as_ref()),
            index_changes: saved.index != current.index,
        })
        .collect();
    Ok(RecoveryRestorePlan {
        point,
        entries,
        fingerprint: fingerprint(snapshot.head.as_deref(), snapshot.operation, &current, &())?,
    })
}

/// A restore never replaces a directory with a saved file; the directory's
/// contents are changes of their own, so the user moves it aside first.
fn refuse_replacing_directories(saved: &[PathState], current: &[PathState]) -> Result<(), String> {
    match saved
        .iter()
        .zip(current)
        .find(|(saved, current)| saved.worktree.is_some() && current.directory)
    {
        Some((saved, _)) => Err(format!(
            "{} is now a directory in the working copy, so Repola will not replace it. Move it aside and review the restore again.",
            saved.path.display
        )),
        None => Ok(()),
    }
}

fn restore_effect(saved: Option<&WorktreeEntry>, current: Option<&WorktreeEntry>) -> RestoreEffect {
    match (saved, current) {
        (saved, current) if saved == current => RestoreEffect::Unchanged,
        (Some(_), None) => RestoreEffect::Create,
        (Some(_), Some(_)) => RestoreEffect::Replace,
        (None, _) => RestoreEffect::Remove,
    }
}

/// Puts every saved path back exactly as it was. Whatever the restore
/// replaces is first saved as a recovery point of its own, so restoring is
/// itself recoverable.
pub fn restore_recovery_point(
    request: RecoveryRestoreRequest,
) -> Result<RecoveryRestoreResult, String> {
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path.clone(),
        worktree_path: request.worktree_path.clone(),
    })?;
    if snapshot.operation.is_some() {
        return Err(
            "Finish or abort the in-progress Git operation before restoring discarded changes."
                .into(),
        );
    }
    let worktree = PathBuf::from(&snapshot.worktree_path);
    let (point, saved) = load_point(&worktree, &request.point)?;
    let paths: Vec<GitPath> = saved.iter().map(|state| state.path.clone()).collect();
    let current = observe_paths(&worktree, snapshot.head.as_deref(), &paths, true)?;
    if fingerprint(snapshot.head.as_deref(), snapshot.operation, &current, &())?
        != request.fingerprint
    {
        return Err(
            "The working copy changed after this restore was reviewed. Review it again.".into(),
        );
    }
    refuse_replacing_directories(&saved, &current)?;
    let changed: Vec<(&PathState, &PathState)> = saved
        .iter()
        .zip(&current)
        .filter(|(saved, current)| current.differs_from(saved))
        .collect();
    if changed.is_empty() {
        return Err("The working copy already matches this recovery point.".into());
    }
    let replaced_states: Vec<PathState> = changed
        .iter()
        .map(|(_, current)| (*current).clone())
        .collect();
    let replaced = if replaced_states.iter().any(PathState::has_content) {
        Some(store_recovery_point(
            &worktree,
            RecoveryPointKind::Restore,
            format!("Replaced while restoring “{}”", point.summary),
            snapshot.head.as_deref(),
            &replaced_states,
        )?)
    } else {
        None
    };

    apply_restore(&worktree, &point, &changed).map_err(|error| match &replaced {
        Some(replaced) => format!(
            "{error}\n\nThe content this restore was replacing is saved in recovery point {}.",
            replaced.id
        ),
        None => error,
    })?;
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: snapshot.repository_path,
        worktree_path: snapshot.worktree_path,
    })?;
    Ok(RecoveryRestoreResult { snapshot, replaced })
}

fn apply_restore(
    worktree: &Path,
    point: &RecoveryPoint,
    changed: &[(&PathState, &PathState)],
) -> Result<(), String> {
    let symlinks = core_symlinks(worktree)?;
    for (saved, current) in changed {
        if saved.worktree != current.worktree {
            restore_worktree_entry(worktree, &saved.path, saved.worktree.as_ref(), symlinks)?;
        }
    }
    let absent = "0".repeat(point.oid.len());
    let mut input = Vec::new();
    let mut intents = Vec::new();
    let mut placeholders = Vec::new();
    for (saved, current) in changed {
        if saved.index == current.index {
            continue;
        }
        let path = decode_path_token_bytes(&saved.path.token)?;
        // A zero mode removes every stage of the path before the saved
        // entries are added back.
        push_index_info(&mut input, "0", &absent, &path);
        for entry in &saved.index {
            if entry.intent_to_add {
                intents.extend_from_slice(b":(literal)");
                intents.extend_from_slice(&path);
                intents.push(0);
                if saved.worktree.is_none() {
                    placeholders.push(&saved.path);
                }
                continue;
            }
            input.extend_from_slice(
                format!("{} {} {}\t", entry.mode, entry.oid, entry.stage).as_bytes(),
            );
            input.extend_from_slice(&path);
            input.push(0);
        }
    }
    if !input.is_empty() {
        let output =
            command::git_at_with_input(worktree, ["update-index", "-z", "--index-info"], &input)
                .map_err(|error| error.to_string())?;
        successful_stdout(output, "restore the saved index entries")?;
    }
    if !intents.is_empty() {
        restore_intents(worktree, &intents, &placeholders)?;
    }
    restore_index_flags(worktree, changed)
}

/// Records intent-to-add entries the way they were made, since plumbing cannot
/// set that flag. `add -N` needs a file on disk: saved files are already back,
/// and an entry whose file was deleted gets an empty stand-in for the moment
/// `add -N` runs. `--force` covers a path an ignore rule matches.
fn restore_intents(
    worktree: &Path,
    intents: &[u8],
    placeholders: &[&GitPath],
) -> Result<(), String> {
    let mut created = Vec::new();
    let recorded = placeholders
        .iter()
        .try_for_each(|path| {
            let target = prepare_path(worktree, &WorktreePath::new(path)?, &path.display)?;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map_err(|error| format!("{} could not be restored: {error}", path.display))?;
            created.push(*path);
            Ok(())
        })
        .and_then(|()| {
            let output = command::git_at_with_input(
                worktree,
                [
                    "add",
                    "--force",
                    "--intent-to-add",
                    "--pathspec-from-file=-",
                    "--pathspec-file-nul",
                ],
                intents,
            )
            .map_err(|error| error.to_string())?;
            successful_stdout(output, "restore the intent-to-add entries").map(drop)
        });
    for path in created {
        remove_worktree_entry(worktree, path)?;
    }
    recorded
}

fn core_symlinks(worktree: &Path) -> Result<bool, String> {
    let output = command::git_at(
        worktree,
        ["config", "--type=bool", "--get", "core.symlinks"],
    )
    .map_err(|error| error.to_string())?;
    match output.status.code() {
        Some(0) => Ok(String::from_utf8_lossy(&output.stdout).trim() == "true"),
        Some(1) => Ok(DEFAULT_CORE_SYMLINKS),
        _ => Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
    }
}

fn restore_worktree_entry(
    worktree: &Path,
    path: &GitPath,
    entry: Option<&WorktreeEntry>,
    symlinks: bool,
) -> Result<(), String> {
    let valid = WorktreePath::new(path)?;
    let Some(entry) = entry else {
        return remove_worktree_entry(worktree, path);
    };
    let target = prepare_path(worktree, &valid, &path.display)?;
    if fs::symlink_metadata(&target).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        return Err(format!(
            "{} is now a directory, so Repola will not replace it.",
            path.display
        ));
    }
    if entry.kind == WorktreeEntryKind::Symlink && symlinks {
        let link = git_stdout(
            worktree,
            ["cat-file", "blob", &entry.oid],
            "read the saved link",
        )?;
        replace_with_symlink(&target, &link, &path.display)
    } else {
        // With `core.symlinks=false` Git checks a link out as a plain file
        // holding its target; a restore does the same.
        replace_with_blob(
            worktree,
            &entry.oid,
            &target,
            entry.kind == WorktreeEntryKind::Executable,
            &path.display,
        )
    }
}

/// Removes the file or symbolic link at `path`, never a directory and never
/// anything reached through a symbolic link.
pub(super) fn remove_worktree_entry(worktree: &Path, path: &GitPath) -> Result<(), String> {
    let valid = WorktreePath::new(path)?;
    let Some(target) = inspect_path(worktree, &valid)? else {
        return Ok(());
    };
    match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.file_type().is_dir() => Err(format!(
            "{} is a directory, so Repola will not remove it.",
            path.display
        )),
        Ok(_) => {
            fs::remove_file(&target)
                .map_err(|error| format!("{} could not be removed: {error}", path.display))?;
            remove_empty_parents(worktree, &valid);
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{} could not be inspected: {error}", path.display)),
    }
}

/// Removes the directories a removed file leaves empty, innermost first and
/// never the worktree root, the way `git clean -d` does. `remove_dir` only
/// removes empty directories, every directory is re-checked to be a real one
/// on the way down, and the walk stops at the first directory that is not
/// empty or cannot be removed, so no content and nothing reached through a
/// link is ever deleted.
fn remove_empty_parents(worktree: &Path, path: &WorktreePath) {
    let parents = &path.components[..path.components.len().saturating_sub(1)];
    for depth in (1..=parents.len()).rev() {
        let Ok(Some(directory)) = real_directory(worktree, &parents[..depth]) else {
            return;
        };
        if fs::remove_dir(&directory).is_err() {
            return;
        }
    }
}

fn sibling_temporary(target: &Path) -> Result<PathBuf, String> {
    let parent = target
        .parent()
        .ok_or_else(|| "The restore target has no parent directory.".to_string())?;
    Ok(parent.join(format!(".repola-restore-{}", uuid::Uuid::new_v4().simple())))
}

fn replace_with_blob(
    worktree: &Path,
    oid: &str,
    target: &Path,
    executable: bool,
    display: &str,
) -> Result<(), String> {
    let temporary = sibling_temporary(target)?;
    let written = write_blob(worktree, oid, &temporary, executable)
        .and_then(|()| fs::rename(&temporary, target).map_err(|error| error.to_string()));
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(format!("{display} could not be restored: {error}"));
    }
    Ok(())
}

fn write_blob(
    worktree: &Path,
    oid: &str,
    destination: &Path,
    executable: bool,
) -> Result<(), String> {
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| error.to_string())?;
    let sink = file.try_clone().map_err(|error| error.to_string())?;
    let output = command::git_at_to_file(worktree, ["cat-file", "blob", oid], sink)
        .map_err(|error| error.to_string())?;
    successful_stdout(output, "read the saved file")?;
    file.sync_all().map_err(|error| error.to_string())?;
    if executable {
        mark_executable(&file).map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(unix)]
fn mark_executable(file: &fs::File) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = file.metadata()?.permissions();
    let mode = permissions.mode();
    // Grant execute wherever read is granted, as Git does on checkout.
    permissions.set_mode(mode | ((mode & 0o444) >> 2));
    file.set_permissions(permissions)
}

#[cfg(windows)]
fn mark_executable(_file: &fs::File) -> std::io::Result<()> {
    // Windows has no executable bit to restore.
    Ok(())
}

fn replace_with_symlink(target: &Path, link: &[u8], display: &str) -> Result<(), String> {
    let temporary = sibling_temporary(target)?;
    let created = create_symlink(link, &temporary)
        .and_then(|()| fs::rename(&temporary, target))
        .map_err(|error| error.to_string());
    if let Err(error) = created {
        let _ = fs::remove_file(&temporary);
        return Err(format!("{display} could not be restored: {error}"));
    }
    Ok(())
}

#[cfg(unix)]
fn create_symlink(link: &[u8], path: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(link), path)
}

#[cfg(windows)]
fn create_symlink(link: &[u8], path: &Path) -> std::io::Result<()> {
    let link = std::str::from_utf8(link)
        .map_err(|_| std::io::Error::other("the saved link target is not valid Unicode"))?
        .replace('/', "\\");
    // Like Git for Windows, link to a directory only when the target is one.
    let points_at_directory = path
        .parent()
        .is_some_and(|parent| parent.join(&link).is_dir());
    if points_at_directory {
        std::os::windows::fs::symlink_dir(&link, path)
    } else {
        std::os::windows::fs::symlink_file(&link, path)
    }
}

/// Sets the assume-unchanged and skip-worktree flags of restored entries,
/// which `update-index --index-info` cannot record.
fn restore_index_flags(
    worktree: &Path,
    changed: &[(&PathState, &PathState)],
) -> Result<(), String> {
    set_index_flag(worktree, changed, "--assume-unchanged", |entry| {
        entry.assume_unchanged
    })?;
    set_index_flag(worktree, changed, "--skip-worktree", |entry| {
        entry.skip_worktree
    })
}

fn set_index_flag(
    worktree: &Path,
    changed: &[(&PathState, &PathState)],
    flag: &str,
    flagged: impl Fn(&IndexEntry) -> bool,
) -> Result<(), String> {
    let mut input = Vec::new();
    for (saved, current) in changed {
        if saved.index != current.index && saved.index.iter().any(&flagged) {
            input.extend(decode_path_token_bytes(&saved.path.token)?);
            input.push(0);
        }
    }
    if input.is_empty() {
        return Ok(());
    }
    let output =
        command::git_at_with_input(worktree, ["update-index", "-z", flag, "--stdin"], &input)
            .map_err(|error| error.to_string())?;
    successful_stdout(output, "restore the saved index flags").map(drop)
}

/// The change restoring one saved path would make, as a patch from the
/// current working-tree content to the saved content.
pub fn recovery_file_diff(request: RecoveryFileDiffRequest) -> Result<RecoveryFileDiff, String> {
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path,
        worktree_path: request.worktree_path,
    })?;
    let worktree = Path::new(&snapshot.worktree_path);
    let (_, saved) = load_point(worktree, &request.point)?;
    let state = saved
        .iter()
        .find(|state| state.path.token == request.path.token)
        .ok_or_else(|| {
            format!(
                "{} is not part of this recovery point.",
                request.path.display
            )
        })?;
    let too_large = RecoveryFileDiff {
        patch: String::new(),
        binary: false,
        truncated: true,
    };
    let temporary =
        tempfile::tempdir().map_err(|error| format!("Could not prepare the preview: {error}"))?;

    let valid = WorktreePath::new(&state.path)?;
    let current = match inspect_path(worktree, &valid)? {
        None => null_device(),
        Some(location) => match fs::symlink_metadata(&location) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => null_device(),
            Err(error) => {
                return Err(format!(
                    "{} could not be inspected: {error}",
                    state.path.display
                ))
            }
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let target = fs::read_link(&location).map_err(|error| error.to_string())?;
                let copy = temporary.path().join("current");
                fs::write(&copy, link_target_bytes(target, &state.path.display)?)
                    .map_err(|error| error.to_string())?;
                copy.into_os_string()
            }
            Ok(metadata) if metadata.is_file() => {
                if metadata.len() > MAX_PREVIEW_SIDE_BYTES {
                    return Ok(too_large);
                }
                location.into_os_string()
            }
            Ok(_) => return Err(format!("{} is now a directory.", state.path.display)),
        },
    };
    let saved_side = match &state.worktree {
        None => null_device(),
        Some(entry) => {
            if entry.size > MAX_PREVIEW_SIDE_BYTES {
                return Ok(too_large);
            }
            let copy = temporary.path().join("saved");
            write_blob(worktree, &entry.oid, &copy, false)?;
            copy.into_os_string()
        }
    };
    let diff = |extra: &str| -> Result<Vec<u8>, String> {
        let output = command::git_at(
            worktree,
            [
                OsString::from("diff"),
                OsString::from("--no-index"),
                OsString::from("--no-color"),
                OsString::from("--no-ext-diff"),
                OsString::from("--no-textconv"),
                OsString::from(extra),
                OsString::from("--"),
                current.clone(),
                saved_side.clone(),
            ],
        )
        .map_err(|error| error.to_string())?;
        if output.status.success() || output.status.code() == Some(1) {
            Ok(output.stdout)
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    };
    let binary = diff("--numstat")?
        .split(|byte| *byte == b'\n')
        .any(|line| line.starts_with(b"-\t-\t"));
    let (patch, truncated) = truncate_file_patch(&diff("--patch")?);
    Ok(RecoveryFileDiff {
        patch: relabel_patch(&patch, &state.path.display),
        binary,
        truncated,
    })
}

/// Replaces the temporary file names `git diff --no-index` prints in a patch
/// header with the saved path, so the preview names the file being restored.
fn relabel_patch(patch: &str, display: &str) -> String {
    let old = patch_label("a/", display);
    let new = patch_label("b/", display);
    let mut relabeled = String::with_capacity(patch.len());
    let mut header = true;
    for line in patch.split_inclusive('\n') {
        if line.starts_with("@@") {
            header = false;
        }
        let body = line.trim_end_matches('\n');
        if !header || body == "--- /dev/null" || body == "+++ /dev/null" {
            relabeled.push_str(line);
            continue;
        }
        let replacement = if body.starts_with("diff --git ") {
            Some(format!("diff --git {old} {new}"))
        } else if body.starts_with("--- ") {
            Some(format!("--- {old}"))
        } else if body.starts_with("+++ ") {
            Some(format!("+++ {new}"))
        } else if body.starts_with("Binary files ") {
            let from = if body.starts_with("Binary files /dev/null ") {
                "/dev/null"
            } else {
                old.as_str()
            };
            let to = if body.ends_with(" and /dev/null differ") {
                "/dev/null"
            } else {
                new.as_str()
            };
            Some(format!("Binary files {from} and {to} differ"))
        } else {
            None
        };
        match replacement {
            Some(replacement) => {
                relabeled.push_str(&replacement);
                relabeled.push_str(&line[body.len()..]);
            }
            None => relabeled.push_str(line),
        }
    }
    relabeled
}

/// Quotes a patch file name the way Git does when it holds special characters.
fn patch_label(prefix: &str, display: &str) -> String {
    if !display
        .chars()
        .any(|character| matches!(character, '"' | '\\') || character.is_control())
    {
        return format!("{prefix}{display}");
    }
    let mut quoted = String::from("\"");
    quoted.push_str(prefix);
    for character in display.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            character if character.is_control() => {
                quoted.push_str(&format!("\\{:03o}", u32::from(character)))
            }
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

/// Deletes the reviewed recovery points in one transaction; if any reference
/// moved or vanished, none is deleted.
pub fn delete_recovery_points(
    request: DeleteRecoveryPointsRequest,
) -> Result<Vec<RecoveryPoint>, String> {
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path,
        worktree_path: request.worktree_path,
    })?;
    let worktree = Path::new(&snapshot.worktree_path);
    if request.points.is_empty() {
        return Err("Select at least one recovery point to delete.".into());
    }
    let mut seen = HashSet::new();
    let mut input = Vec::new();
    for point in &request.points {
        validate_reference(point)?;
        if !seen.insert(point.id.as_str()) {
            return Err("The selection contains a recovery point twice.".into());
        }
        input.extend_from_slice(format!("delete {}\0{}\0", point.id, point.oid).as_bytes());
    }
    let output = command::git_at_with_input(worktree, ["update-ref", "--stdin", "-z"], &input)
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Nothing was deleted: a selected recovery point changed or no longer exists. Refresh and review the selection again.\n\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    list(worktree)
}

fn split_once(bytes: &[u8], delimiter: u8) -> Option<(&[u8], &[u8])> {
    let position = bytes.iter().position(|byte| *byte == delimiter)?;
    Some((&bytes[..position], &bytes[position + 1..]))
}

fn successful_stdout(output: std::process::Output, intent: &str) -> Result<Vec<u8>, String> {
    if output.status.success() {
        return Ok(output.stdout);
    }
    let diagnostic = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if diagnostic.is_empty() {
        format!("Git could not {intent}.")
    } else {
        diagnostic
    })
}

fn git_stdout<I, S>(worktree: &Path, args: I, intent: &str) -> Result<Vec<u8>, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = command::git_at(worktree, args).map_err(|error| error.to_string())?;
    successful_stdout(output, intent)
}

/// Runs Git with its output in an anonymous temporary file, for output that
/// may exceed the in-memory capture bound.
fn git_to_temporary_file<I, S>(worktree: &Path, args: I, intent: &str) -> Result<fs::File, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut file = tempfile::tempfile().map_err(|error| error.to_string())?;
    let sink = file.try_clone().map_err(|error| error.to_string())?;
    let output =
        command::git_at_to_file(worktree, args, sink).map_err(|error| error.to_string())?;
    successful_stdout(output, intent)?;
    file.rewind().map_err(|error| error.to_string())?;
    Ok(file)
}

/// The compact form used in reference names and the RFC 3339 form shown to
/// people, both in UTC.
fn utc_timestamps(time: SystemTime) -> (String, String) {
    let elapsed = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = elapsed.as_secs();
    let (year, month, day) = civil_from_days(seconds / 86_400);
    let of_day = seconds % 86_400;
    let (hour, minute, second) = (of_day / 3_600, of_day % 3_600 / 60, of_day % 60);
    (
        format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z"),
        format!(
            "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z",
            elapsed.subsec_millis()
        ),
    )
}

/// Converts days since 1970-01-01 to a proleptic Gregorian date (Howard
/// Hinnant's `civil_from_days`, restricted to dates after the epoch).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    (year_of_era + era * 400 + u64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worktree::models::{BranchRequest, HistoryRequest, ReflogRequest, TagRequest};
    use crate::worktree::working_copy::git_path;
    use crate::worktree::{branches, history, reflog, tags};

    fn git(path: &Path, args: &[&str]) -> String {
        let output = command::successful_git_at(path, args).expect("git");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// A repository with one commit, returned with its canonical root.
    fn repository() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().expect("temp repository");
        let path = dunce::canonicalize(directory.path()).expect("canonical");
        git(&path, &["init", "--initial-branch=main"]);
        git(&path, &["config", "core.autocrlf", "false"]);
        git(&path, &["config", "user.name", "Repola Test"]);
        git(&path, &["config", "user.email", "repola@example.invalid"]);
        fs::write(path.join("base.txt"), "base\n").expect("base");
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        (directory, path)
    }

    fn request(path: &Path) -> WorkingCopyRequest {
        WorkingCopyRequest {
            repository_path: path.to_string_lossy().into_owned(),
            worktree_path: path.to_string_lossy().into_owned(),
        }
    }

    fn reference(point: &RecoveryPoint) -> RecoveryPointReference {
        RecoveryPointReference {
            id: point.id.clone(),
            oid: point.oid.clone(),
        }
    }

    fn saved_file(path: &Path, name: &str, kind: WorktreeEntryKind, bytes: &[u8]) -> PathState {
        PathState {
            path: git_path(name.as_bytes()),
            index: Vec::new(),
            worktree: Some(WorktreeEntry {
                kind,
                oid: hash_bytes(path, bytes, true).expect("blob"),
                size: bytes.len() as u64,
            }),
            directory: false,
        }
    }

    fn store(path: &Path, states: &[PathState]) -> RecoveryPoint {
        store_recovery_point(
            path,
            RecoveryPointKind::DiscardFile,
            "Discarded test content".into(),
            None,
            states,
        )
        .expect("store")
    }

    fn plan(path: &Path, point: &RecoveryPointReference) -> Result<RecoveryRestorePlan, String> {
        plan_recovery_restore(RecoveryPointRequest {
            repository_path: request(path).repository_path,
            worktree_path: request(path).worktree_path,
            point: point.clone(),
        })
    }

    fn restore(
        path: &Path,
        point: &RecoveryPointReference,
    ) -> Result<RecoveryRestoreResult, String> {
        let fingerprint = plan(path, point)?.fingerprint;
        restore_recovery_point(RecoveryRestoreRequest {
            repository_path: request(path).repository_path,
            worktree_path: request(path).worktree_path,
            point: point.clone(),
            fingerprint,
        })
    }

    /// Writes a recovery point whose manifest and tree are chosen by the test,
    /// the way a damaged or hostile repository could contain one.
    fn craft(
        path: &Path,
        name: &str,
        manifest: &[u8],
        tree: &[(&str, String, Vec<u8>)],
    ) -> RecoveryPointReference {
        let summary = Summary {
            version: MANIFEST_VERSION,
            kind: RecoveryPointKind::DiscardFile,
            summary: "Crafted".into(),
            created_at: "2026-01-01T00:00:00.000Z".into(),
            worktree_path: path.to_string_lossy().into_owned(),
            head: None,
            path_count: 1,
            paths: Vec::new(),
            stored_bytes: 0,
        };
        let mut input = Vec::new();
        let summary =
            hash_bytes(path, &serde_json::to_vec(&summary).expect("json"), true).expect("summary");
        push_index_info(&mut input, "100644", &summary, b"summary.json");
        push_index_info(
            &mut input,
            "100644",
            &hash_bytes(path, manifest, true).expect("manifest"),
            b"manifest.json",
        );
        for (mode, oid, entry) in tree {
            push_index_info(&mut input, mode, oid, entry);
        }
        let temporary = tempfile::tempdir().expect("index directory");
        let index = temporary.path().join("index");
        let environment = [("GIT_INDEX_FILE", index.as_os_str())];
        let output = command::git_at_with_input_and_env(
            path,
            ["update-index", "-z", "--index-info"],
            &input,
            environment,
        )
        .expect("index");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = command::git_at_with_env(path, ["write-tree"], environment).expect("tree");
        let tree = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let id = format!("{RECOVERY_REF_NAMESPACE}/{name}");
        create_reference(path, &id, &tree).expect("reference");
        RecoveryPointReference { id, oid: tree }
    }

    fn manifest_for(states: &[PathState]) -> Vec<u8> {
        serde_json::to_vec(&Manifest {
            version: MANIFEST_VERSION,
            paths: states.to_vec(),
        })
        .expect("manifest json")
    }

    #[test]
    fn recovery_refs_never_appear_in_branch_tag_history_or_reflog_queries() {
        let (_directory, path) = repository();
        fs::write(path.join("base.txt"), "edited\n").expect("edit");
        let states =
            observe_paths(&path, Some("HEAD"), &[git_path(b"base.txt")], true).expect("observe");
        let point = store(&path, &states);
        assert!(point.id.starts_with("refs/repola/discarded/"));
        assert_eq!(git(&path, &["cat-file", "-t", &point.id]).trim(), "tree");

        let branches = branches(BranchRequest {
            repository_path: request(&path).repository_path,
            worktree_path: request(&path).worktree_path,
        })
        .expect("branches");
        assert_eq!(
            branches
                .iter()
                .map(|branch| branch.full_name.as_str())
                .collect::<Vec<_>>(),
            ["refs/heads/main"]
        );
        let tags = tags(TagRequest {
            repository_path: request(&path).repository_path,
            worktree_path: request(&path).worktree_path,
        })
        .expect("tags");
        assert!(tags.is_empty());
        let page = history(HistoryRequest {
            repository_path: request(&path).repository_path,
            worktree_path: request(&path).worktree_path,
            cursor: None,
            query: None,
            comparison_base: None,
            limit: 50,
        })
        .expect("history");
        assert_eq!(page.commits.len(), 1);
        let entries = reflog(ReflogRequest {
            repository_path: request(&path).repository_path,
            worktree_path: request(&path).worktree_path,
            limit: 50,
        })
        .expect("reflog");
        assert!(entries
            .iter()
            .all(|entry| entry.oid != point.oid && !entry.selector.contains("repola")));
        // Tree references are skipped by every commit walk, even with --all.
        assert_eq!(git(&path, &["rev-list", "--all"]).lines().count(), 1);
        assert!(
            !git(&path, &["log", "--all", "--decorate=full", "--format=%D"]).contains("repola")
        );
    }

    #[test]
    fn recovery_references_are_create_only() {
        let (_directory, path) = repository();
        let first = store(
            &path,
            &[saved_file(
                &path,
                "a.txt",
                WorktreeEntryKind::File,
                b"first\n",
            )],
        );
        let second = store(
            &path,
            &[saved_file(
                &path,
                "a.txt",
                WorktreeEntryKind::File,
                b"second\n",
            )],
        );
        assert_ne!(first.id, second.id);
        let error =
            create_reference(&path, &first.id, &second.oid).expect_err("existing reference");
        assert!(error.contains("already exists"), "{error}");
        assert_eq!(git(&path, &["rev-parse", &first.id]).trim(), first.oid);
    }

    #[test]
    fn crafted_paths_that_escape_the_working_copy_are_refused() {
        let (directory, path) = repository();
        let blob = hash_bytes(&path, b"payload\n", true).expect("blob");
        for (index, name) in [
            "../escape.txt",
            ".git/hooks/pre-commit",
            "sub/.GIT. /config",
            "a//b",
            "./a",
            "/absolute",
        ]
        .iter()
        .enumerate()
        {
            let state = PathState {
                path: git_path(name.as_bytes()),
                index: Vec::new(),
                worktree: Some(WorktreeEntry {
                    kind: WorktreeEntryKind::File,
                    oid: blob.clone(),
                    size: 8,
                }),
                directory: false,
            };
            let point = craft(
                &path,
                &format!("crafted-{index}"),
                &manifest_for(&[state]),
                &[],
            );
            let error = plan(&path, &point).expect_err("unsafe path");
            assert!(error.contains("is not a safe path"), "{name}: {error}");
            let error = restore_recovery_point(RecoveryRestoreRequest {
                repository_path: request(&path).repository_path,
                worktree_path: request(&path).worktree_path,
                point,
                fingerprint: String::new(),
            })
            .expect_err("unsafe path");
            assert!(error.contains("is not a safe path"), "{name}: {error}");
        }
        assert!(!directory
            .path()
            .parent()
            .expect("parent")
            .join("escape.txt")
            .exists());
        assert!(!path.join(".git/hooks/pre-commit").exists());
    }

    #[test]
    fn a_manifest_that_disagrees_with_its_tree_is_refused() {
        let (_directory, path) = repository();
        let saved = hash_bytes(&path, b"saved\n", true).expect("saved");
        let other = hash_bytes(&path, b"other\n", true).expect("other");
        let state = PathState {
            path: git_path(b"a.txt"),
            index: Vec::new(),
            worktree: Some(WorktreeEntry {
                kind: WorktreeEntryKind::File,
                oid: saved,
                size: 6,
            }),
            directory: false,
        };
        let point = craft(
            &path,
            "mismatch",
            &manifest_for(&[state]),
            &[("100644", other, b"worktree/a.txt".to_vec())],
        );
        let error = plan(&path, &point).expect_err("damaged");
        assert!(error.contains("damaged"), "{error}");
    }

    #[test]
    fn restores_never_write_through_a_file_or_link_in_place_of_a_directory() {
        let (directory, path) = repository();
        let state = saved_file(
            &path,
            "blocker/file.txt",
            WorktreeEntryKind::File,
            b"saved\n",
        );
        fs::write(path.join("blocker"), "a file, not a directory\n").expect("blocker");
        let point = reference(&store(&path, &[state]));
        assert_eq!(
            plan(&path, &point).expect("plan").entries[0].worktree,
            RestoreEffect::Create
        );
        let error = restore(&path, &point).expect_err("parent is a file");
        assert!(error.contains("is not a directory"), "{error}");
        assert_eq!(
            fs::read(path.join("blocker")).expect("blocker"),
            b"a file, not a directory\n"
        );

        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().expect("outside");
            std::os::unix::fs::symlink(outside.path(), path.join("link")).expect("link");
            let state = saved_file(&path, "link/file.txt", WorktreeEntryKind::File, b"saved\n");
            let point = reference(&store(&path, &[state]));
            let error = restore(&path, &point).expect_err("parent is a link");
            assert!(error.contains("is not a directory"), "{error}");
            assert_eq!(fs::read_dir(outside.path()).expect("outside").count(), 0);
        }
        drop(directory);
    }

    #[test]
    fn symbolic_links_restore_the_way_git_checks_them_out() {
        let (_directory, path) = repository();
        let link = || saved_file(&path, "alias", WorktreeEntryKind::Symlink, b"base.txt");

        git(&path, &["config", "core.symlinks", "false"]);
        let point = reference(&store(&path, &[link()]));
        restore(&path, &point).expect("restore without symlinks");
        let metadata = fs::symlink_metadata(path.join("alias")).expect("alias");
        assert!(metadata.file_type().is_file());
        assert_eq!(fs::read(path.join("alias")).expect("alias"), b"base.txt");
        fs::remove_file(path.join("alias")).expect("remove");

        // Creating links needs privileges Windows runners do not grant, so
        // the link-creating side is exercised where links always work.
        #[cfg(unix)]
        {
            git(&path, &["config", "core.symlinks", "true"]);
            let point = reference(&store(&path, &[link()]));
            restore(&path, &point).expect("restore with symlinks");
            assert_eq!(
                fs::read_link(path.join("alias")).expect("link"),
                Path::new("base.txt")
            );
        }
    }

    #[test]
    fn the_executable_bit_restores_where_the_platform_has_one() {
        let (_directory, path) = repository();
        let state = saved_file(
            &path,
            "tool.sh",
            WorktreeEntryKind::Executable,
            b"#!/bin/sh\n",
        );
        restore(&path, &reference(&store(&path, &[state]))).expect("restore");
        assert_eq!(
            fs::read(path.join("tool.sh")).expect("tool"),
            b"#!/bin/sh\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(path.join("tool.sh"))
                .expect("meta")
                .permissions()
                .mode();
            assert_ne!(mode & 0o100, 0);
        }
    }

    #[test]
    fn deleting_recovery_points_is_all_or_nothing() {
        let (_directory, path) = repository();
        let first = store(
            &path,
            &[saved_file(&path, "a.txt", WorktreeEntryKind::File, b"a\n")],
        );
        let second = store(
            &path,
            &[saved_file(&path, "b.txt", WorktreeEntryKind::File, b"b\n")],
        );
        let delete = |points: Vec<RecoveryPointReference>| {
            delete_recovery_points(DeleteRecoveryPointsRequest {
                repository_path: request(&path).repository_path,
                worktree_path: request(&path).worktree_path,
                points,
            })
        };
        let stale = RecoveryPointReference {
            id: second.id.clone(),
            oid: first.oid.clone(),
        };
        let error = delete(vec![reference(&first), stale]).expect_err("stale selection");
        assert!(error.starts_with("Nothing was deleted"), "{error}");
        assert_eq!(list_recovery_points(request(&path)).expect("list").len(), 2);
        assert!(delete(vec![reference(&first), reference(&first)]).is_err());
        let remaining = delete(vec![reference(&first), reference(&second)]).expect("delete");
        assert!(remaining.is_empty());
    }

    #[test]
    fn previews_show_what_restoring_would_change_under_the_saved_name() {
        let (_directory, path) = repository();
        let point = store(
            &path,
            &[saved_file(
                &path,
                "base.txt",
                WorktreeEntryKind::File,
                b"saved\n",
            )],
        );
        let diff = recovery_file_diff(RecoveryFileDiffRequest {
            repository_path: request(&path).repository_path,
            worktree_path: request(&path).worktree_path,
            point: reference(&point),
            path: git_path(b"base.txt"),
        })
        .expect("diff");
        assert!(!diff.binary && !diff.truncated);
        assert!(
            diff.patch.starts_with("diff --git a/base.txt b/base.txt\n"),
            "{}",
            diff.patch
        );
        assert!(diff.patch.contains("\n--- a/base.txt\n+++ b/base.txt\n"));
        assert!(diff.patch.contains("\n-base\n+saved\n"));
    }

    #[test]
    fn long_paths_never_push_a_summary_past_what_listing_reads() {
        let (_directory, path) = repository();
        // Each sampled path is stored as its name and its hex token.
        let deep = format!("{}/", "d".repeat(200)).repeat(20);
        let states: Vec<PathState> = (0..SUMMARY_SAMPLE_PATHS)
            .map(|index| {
                saved_file(
                    &path,
                    &format!("{deep}{index}.txt"),
                    WorktreeEntryKind::File,
                    b"x",
                )
            })
            .collect();
        let point = store(&path, &states);
        assert!(!point.paths.is_empty());
        assert!(point.paths.len() < SUMMARY_SAMPLE_PATHS);
        let listed = list_recovery_points(request(&path)).expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path_count, SUMMARY_SAMPLE_PATHS as u64);
    }

    #[test]
    fn listing_skips_foreign_references_and_sorts_newest_first() {
        let (_directory, path) = repository();
        let first = store(
            &path,
            &[saved_file(&path, "a.txt", WorktreeEntryKind::File, b"a\n")],
        );
        // Creation times have millisecond resolution.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let second = store(
            &path,
            &[saved_file(&path, "b.txt", WorktreeEntryKind::File, b"b\n")],
        );
        let head = git(&path, &["rev-parse", "HEAD"]);
        git(
            &path,
            &[
                "update-ref",
                "refs/repola/discarded/not-a-tree",
                head.trim(),
            ],
        );
        let points = list_recovery_points(request(&path)).expect("list");
        assert_eq!(
            points
                .iter()
                .map(|point| point.id.as_str())
                .collect::<Vec<_>>(),
            [second.id.as_str(), first.id.as_str()]
        );
        assert_eq!(points[0].paths, vec![git_path(b"b.txt")]);
    }

    #[test]
    fn listing_reads_every_point_and_skips_summaries_too_large_to_read() {
        let (_directory, path) = repository();
        let first = store(
            &path,
            &[saved_file(&path, "a.txt", WorktreeEntryKind::File, b"a\n")],
        );
        // A foreign tree whose summary alone exceeds the capture limit.
        let large = tempfile::NamedTempFile::new().expect("large summary");
        std::fs::write(large.path(), vec![b' '; 33 * 1024 * 1024]).expect("write");
        let blob = git(
            &path,
            &["hash-object", "-w", &large.path().to_string_lossy()],
        );
        let tree = command::git_at_with_input(
            &path,
            ["mktree"],
            format!("100644 blob {}\tsummary.json\n", blob.trim()).as_bytes(),
        )
        .expect("mktree");
        let large_tree = String::from_utf8_lossy(&tree.stdout).trim().to_string();
        let large_id = format!("{RECOVERY_REF_NAMESPACE}/point-0300");
        let mut copies = Vec::new();
        let mut updates = String::new();
        for index in 0..2 * SUMMARIES_PER_READ + 1 {
            let id = format!("{RECOVERY_REF_NAMESPACE}/point-{index:04}");
            let oid = if id == large_id {
                large_tree.as_str()
            } else {
                copies.push(id.clone());
                first.oid.as_str()
            };
            updates.push_str(&format!("create {id} {oid}\n"));
        }
        let output =
            command::git_at_with_input(&path, ["update-ref", "--stdin"], updates.as_bytes())
                .expect("update-ref");
        assert!(output.status.success());
        let mut listed: Vec<String> = list_recovery_points(request(&path))
            .expect("list")
            .into_iter()
            .map(|point| point.id)
            .collect();
        listed.sort();
        copies.push(first.id);
        copies.sort();
        assert_eq!(listed, copies);
    }

    #[test]
    fn git_directory_names_are_recognized_in_every_spelling() {
        for name in [".git", ".GIT", ".Git. ", ".git...", "git~1", "GIT~1"] {
            assert!(names_git_directory(name.as_bytes()), "{name}");
        }
        for name in [".github", "git", ".gitignore", "x.git"] {
            assert!(!names_git_directory(name.as_bytes()), "{name}");
        }
    }

    #[test]
    fn timestamps_are_utc_in_both_forms() {
        let at = |seconds: u64, millis: u64| {
            utc_timestamps(UNIX_EPOCH + std::time::Duration::from_millis(seconds * 1_000 + millis))
        };
        assert_eq!(
            at(0, 0),
            ("19700101T000000Z".into(), "1970-01-01T00:00:00.000Z".into())
        );
        assert_eq!(
            at(951_868_799, 5),
            ("20000229T235959Z".into(), "2000-02-29T23:59:59.005Z".into())
        );
        assert_eq!(
            at(1_791_037_501, 123),
            ("20261003T142501Z".into(), "2026-10-03T14:25:01.123Z".into())
        );
    }

    #[test]
    fn patch_labels_quote_special_names_like_git() {
        assert_eq!(patch_label("a/", "src/main.rs"), "a/src/main.rs");
        assert_eq!(
            patch_label("b/", "say \"hi\".txt"),
            "\"b/say \\\"hi\\\".txt\""
        );
        let relabeled = relabel_patch(
            "diff --git a/tmp/x b/tmp/y\nindex 1..2 100644\n--- a/tmp/x\n+++ b/tmp/y\n@@ -1 +1 @@\n--- a/kept\n",
            "file.txt",
        );
        assert_eq!(
            relabeled,
            "diff --git a/file.txt b/file.txt\nindex 1..2 100644\n--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n--- a/kept\n"
        );
    }
}
