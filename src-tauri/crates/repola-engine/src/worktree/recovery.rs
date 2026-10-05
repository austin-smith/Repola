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

use std::cell::OnceCell;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{BufReader, Read, Seek};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use caseless::Caseless;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use super::command;
use super::models::{
    DeleteRecoveryPointsRequest, GitPath, RecoveryFileDiff, RecoveryFileDiffRequest, RecoveryPoint,
    RecoveryPointKind, RecoveryPointList, RecoveryPointReference, RecoveryPointRequest,
    RecoveryRestoreEntry, RecoveryRestorePlan, RecoveryRestoreRequest, RecoveryRestoreResult,
    RepositoryOperation, RestoreEffect, WorkingCopyRequest,
};
use super::working_copy::{
    decode_path_token_bytes, git_path, hex, literal_pathspec, null_device,
    os_string_from_path_bytes, working_copy_snapshot,
};

const RECOVERY_REF_NAMESPACE: &str = "refs/repola/discarded";
const MANIFEST_VERSION: u32 = 1;
/// Paths listed in a summary for display; the manifest holds all of them.
const SUMMARY_SAMPLE_PATHS: usize = 20;
const MAX_SUMMARY_BYTES: usize = 64 * 1024;
/// For Git steps that run once a discard or restore has started changing
/// files: running out of time partway would leave the change half done, so
/// they get as long as a remote change is waited for rather than the usual
/// limit for one Git command.
pub(super) const CHANGE_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
pub(super) const CHANGED_WHILE_SAVING: &str =
    "The working copy changed while Repola was saving it. Nothing was changed; review it again.";
/// Summaries read per Git invocation: at most 16 MiB, half the capture limit.
const SUMMARIES_PER_READ: usize = 256;
/// Listed items per response: half of what one SSH protocol frame carries.
pub(super) const MAX_LISTED_BYTES: usize = 8 * 1024 * 1024;
/// Content hashed per Git process, well within the per-command time limit even
/// when every byte is written to the object store.
const HASH_CHUNK_BYTES: u64 = 1024 * 1024 * 1024;
/// Up to this many paths, Git is asked about each one; beyond it, reading the
/// whole index or tree once is faster than matching every entry against
/// every path.
const PATHSPEC_LIMIT: usize = 512;
const MAX_RECOVERY_ID_BYTES: usize = 128;
/// Paths per Git invocation stay well inside the Windows command-line limit.
/// No single path may exceed it, since a path is never split across chunks;
/// UTF-8 is never shorter than the UTF-16 Windows counts, so one that fits in
/// bytes fits there too.
const ARGUMENT_BUDGET_BYTES: usize = 16 * 1024;
/// A restore preview's patch, once encoded, stays within half of one SSH
/// protocol frame.
const MAX_PREVIEW_PATCH_BYTES: usize = 8 * 1024 * 1024;
/// Either side of a restore preview larger than this is not diffed.
const MAX_PREVIEW_SIDE_BYTES: u64 = 8 * 1024 * 1024;

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
    /// That folder's permission bits, so a restore brings it back as it was.
    /// Unix only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_permissions: Option<u32>,
    /// How many of the path's parent folders exist, counted from the top. A
    /// restore removes only the folders below these, so folders that existed
    /// before, even empty ones, stay.
    #[serde(default)]
    pub parents: usize,
    /// The permission bits of those folders, from the top, so a restore that
    /// recreates one does not widen access to it. Unix only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parent_permissions: Vec<u32>,
    /// A parent of the path is a file or a link. Writing the path would
    /// replace it, and it is not saved, so nothing is written there.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub blocked: bool,
    /// How many paths the index holds beneath the path, as if it were a folder.
    /// Writing the path's own entry would drop them.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub index_beneath: usize,
    /// The index holds one of the path's parents as an entry of its own, as
    /// if it were a file; this is how many components that parent has.
    /// Writing the path's entry would drop it. The index can hold at most one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexed_parent: Option<usize>,
}

fn is_zero(count: &usize) -> bool {
    *count == 0
}

impl PathState {
    fn has_content(&self) -> bool {
        !self.index.is_empty() || self.worktree.is_some() || self.directory
    }

    /// Whether restoring `saved` over this state changes anything: its index
    /// entries, its file, or the folder that stood at the path and its
    /// permissions. A folder there now is never removed by a restore.
    fn differs_from(&self, saved: &PathState) -> bool {
        self.index != saved.index
            || self.worktree != saved.worktree
            || saved.directory && !self.directory
            || self.folder_permissions_differ(saved)
    }

    /// The saved folder is still there, with other permissions.
    fn folder_permissions_differ(&self, saved: &PathState) -> bool {
        saved.directory && self.directory && saved.folder_permissions != self.folder_permissions
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
    /// The file's permission bits, so a restore does not widen access to a
    /// private file. Unix only; links have none of their own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<u32>,
    /// A link made as a link to a folder. Windows records links to files and
    /// to folders differently, so a restore makes the same kind whatever its
    /// target is by then.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub directory_link: bool,
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
        if bytes.len() > ARGUMENT_BUDGET_BYTES {
            return Err(format!(
                "{} is too long a path for Repola to save. Rename it, or change it with Git instead.",
                path.display
            ));
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
            if !fits_in_one_name(component) {
                return Err(format!(
                    "{} has a name too long for this file system, so Repola will not change it. Rename it with Git first.",
                    path.display
                ));
            }
            if !stored_as_named(component) {
                return Err(format!(
                    "{} cannot be stored under that name on Windows, so Repola will not change it. Rename it with Git on a system that allows the name.",
                    path.display
                ));
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
/// the trailing dots and spaces Windows ignores and the characters HFS+
/// ignores, and its short-name alias.
fn names_git_directory(component: &[u8]) -> bool {
    let visible: String = String::from_utf8_lossy(component)
        .chars()
        .filter(|&character| !ignored_by_hfs(character))
        .collect();
    let trimmed = visible.trim_end_matches(['.', ' ']);
    trimmed.eq_ignore_ascii_case(".git") || trimmed.eq_ignore_ascii_case("git~1")
}

/// Invisible formatting characters HFS+ leaves out when it compares names.
fn ignored_by_hfs(character: char) -> bool {
    matches!(
        character,
        '\u{200c}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{206a}'..='\u{206f}' | '\u{feff}'
    )
}

/// The forms under which a file system that ignores case may find a name, so
/// names sharing either are taken as one. File systems differ: APFS and
/// Linux's case-insensitive folders apply Unicode case folding, under which
/// `ß` is `ss`; NTFS, exFAT, and ZFS uppercase one character at a time, under
/// which `ı` is `i`; HFS+ also ignores some invisible characters.
fn case_forms(name: &str) -> [String; 2] {
    let visible = || name.chars().filter(|&character| !ignored_by_hfs(character));
    let folded = visible().nfd().default_case_fold().nfd().collect();
    let uppercased = visible()
        .nfd()
        .map(|character| {
            let mut upper = character.to_uppercase();
            match (upper.next(), upper.next()) {
                (Some(upper), None) => upper,
                _ => character,
            }
        })
        .collect();
    [folded, uppercased]
}

#[cfg(windows)]
fn has_platform_separator(component: &[u8]) -> bool {
    component.iter().any(|byte| matches!(byte, b'\\' | b':'))
}

#[cfg(not(windows))]
fn has_platform_separator(_component: &[u8]) -> bool {
    false
}

/// File systems store at most 255 units in one name: UTF-16 units on Windows
/// and macOS, bytes elsewhere.
fn fits_in_one_name(component: &[u8]) -> bool {
    const MAX_NAME_UNITS: usize = 255;
    if cfg!(any(windows, target_os = "macos")) {
        String::from_utf8_lossy(component).encode_utf16().count() <= MAX_NAME_UNITS
    } else {
        component.len() <= MAX_NAME_UNITS
    }
}

/// Whether Git reaches paths of any length here. Unless `core.longpaths` is
/// on, Git for Windows reaches no file of `MAX_PATH` UTF-16 units or more,
/// counted from the drive, and creates no folder of `MAX_PATH - 12` or more.
pub(super) fn long_paths(worktree: &Path) -> Result<bool, String> {
    Ok(!cfg!(windows) || config_bool(worktree, "core.longpaths", false)?)
}

const MAX_PATH: usize = 260;

/// Whether Git can reach the file at `path` in `worktree`.
pub(super) fn git_can_reach(worktree: &Path, path: &[u8], long_paths: bool) -> bool {
    long_paths || windows_path_units(worktree, path) < MAX_PATH
}

/// Whether Git can write the file at `path` in `worktree`, creating the
/// folders on the way that are missing below the first `existing_parents`.
pub(super) fn git_can_write(
    worktree: &Path,
    path: &[u8],
    existing_parents: usize,
    long_paths: bool,
) -> bool {
    let folders = path.iter().filter(|&&byte| byte == b'/').count();
    let deepest = path.iter().rposition(|&byte| byte == b'/').unwrap_or(0);
    git_can_reach(worktree, path, long_paths)
        && (long_paths
            || existing_parents >= folders
            || windows_path_units(worktree, &path[..deepest]) < MAX_PATH - 12)
}

/// The length of `path` inside `worktree` as Windows counts it, from the drive.
fn windows_path_units(worktree: &Path, path: &[u8]) -> usize {
    worktree
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .count()
        + 1
        + String::from_utf8_lossy(path).encode_utf16().count()
}

/// Whether this platform stores a path component under exactly that name.
#[cfg(windows)]
fn stored_as_named(component: &[u8]) -> bool {
    !invalid_on_windows(component)
}

#[cfg(not(windows))]
fn stored_as_named(_component: &[u8]) -> bool {
    true
}

/// Whether Windows cannot store a path component as itself: it drops a
/// trailing dot or space, forbids some characters, and reserves device names
/// such as `NUL` and `COM1`, with or without an extension.
#[cfg(any(windows, test))]
fn invalid_on_windows(component: &[u8]) -> bool {
    if component
        .last()
        .is_some_and(|byte| matches!(byte, b'.' | b' '))
    {
        return true;
    }
    let forbidden = |byte: &u8| {
        *byte < 0x20 || matches!(byte, b'<' | b'>' | b':' | b'"' | b'\\' | b'|' | b'?' | b'*')
    };
    if component.iter().any(forbidden) {
        return true;
    }
    let stem = component
        .split(|byte| *byte == b'.')
        .next()
        .unwrap_or(component);
    let end = stem
        .iter()
        .rposition(|byte| *byte != b' ')
        .map_or(0, |index| index + 1);
    let stem = stem[..end].to_ascii_uppercase();
    let numbered = |prefix: &[u8]| {
        stem.strip_prefix(prefix).is_some_and(|number| {
            matches!(number, [b'1'..=b'9'])
                // Superscript one, two, and three count as digits too.
                || matches!(number, [0xc2, 0xb9] | [0xc2, 0xb2] | [0xc2, 0xb3])
        })
    };
    matches!(
        stem.as_slice(),
        b"CON" | b"PRN" | b"AUX" | b"NUL" | b"CONIN$" | b"CONOUT$"
    ) || numbered(b"COM")
        || numbered(b"LPT")
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

/// Like `inspect_path`, but creates missing parent directories one at a time,
/// adding each to `created`, and refuses any parent that is not a real
/// directory, so a restore can never write through a symbolic link or outside
/// the working copy.
fn prepare_path(
    worktree: &Path,
    path: &WorktreePath,
    display: &str,
    created: &mut Vec<PathBuf>,
) -> Result<PathBuf, String> {
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
                created.push(current.clone());
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

/// Whether the working copy's file system treats names that differ only in
/// case as one: `.git` is also found as `.GIT`.
fn folds_case(worktree: &Path) -> bool {
    fs::symlink_metadata(worktree.join(".git")).is_ok()
        && fs::symlink_metadata(worktree.join(".GIT")).is_ok()
}

/// How the working copy's file system compares names.
struct NameFolding {
    case: bool,
    /// The composed and decomposed forms of a name, such as `é` as one
    /// character or as `e` and an accent, are one name.
    normalization: bool,
}

fn name_folding(worktree: &Path) -> Result<NameFolding, String> {
    Ok(NameFolding {
        // Git's setting can be stale, as in a repository copied from a
        // case-sensitive system, so the file system is asked as well.
        case: config_bool(worktree, "core.ignorecase", false)? || folds_case(worktree),
        // Every macOS file system, case-sensitive or not, compares names this
        // way.
        normalization: cfg!(target_os = "macos"),
    })
}

/// Paths that are one name on the file system, whether or not either is on
/// disk: observing both would save it twice and lose which spelling it had,
/// and writing both would leave only one.
fn refuse_name_collisions(
    paths: &[GitPath],
    validated: &[WorktreePath],
    folding: &NameFolding,
) -> Result<(), String> {
    if !folding.case && !folding.normalization {
        return Ok(());
    }
    let forms = |bytes: &[u8]| {
        let name = String::from_utf8_lossy(bytes);
        if folding.case {
            case_forms(&name).to_vec()
        } else {
            vec![name.nfd().collect()]
        }
    };
    let mut seen: HashMap<(usize, String), (&GitPath, &[u8])> = HashMap::new();
    for (path, valid) in paths.iter().zip(validated) {
        for form in forms(&valid.bytes).into_iter().enumerate() {
            match seen.get(&form) {
                Some((other, bytes)) if *bytes != valid.bytes.as_slice() => {
                    // Only letters from A to Z are one in either case on every
                    // file system that ignores case.
                    return Err(if bytes.eq_ignore_ascii_case(&valid.bytes) {
                        format!(
                            "{} and {} differ only in letter case, so they are one file on this file system and Repola cannot save them separately. Undo this change with Git instead.",
                            other.display, path.display
                        )
                    } else {
                        format!(
                            "{} and {} are spelled differently but this file system may take them as one name, so Repola cannot save them separately. Undo this change with Git instead.",
                            other.display, path.display
                        )
                    });
                }
                Some(_) => {}
                None => {
                    seen.insert(form, (path, &valid.bytes));
                }
            }
        }
    }
    Ok(())
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
    refuse_name_collisions(paths, &validated, &name_folding(worktree)?)?;
    let IndexListing {
        entries: mut index,
        beneath,
    } = index_entries(worktree, head, &validated)?;
    let above = indexed_parents(worktree, &validated)?;

    enum Source {
        File(OsString),
        /// A link's target, and whether it was made as a link to a folder.
        Link(Vec<u8>, bool),
    }
    struct Observed {
        index: Vec<IndexEntry>,
        entry: Option<(WorktreeEntryKind, u64, Option<u32>, Source)>,
        directory: bool,
        folder_permissions: Option<u32>,
        chain: ParentChain,
    }
    // Two Git paths that are one entry on disk, as case or Unicode
    // normalization can make them, would be saved as two copies of one file.
    let mut entries_on_disk: HashMap<OsString, &GitPath> = HashMap::new();
    let mut folder_listings = HashMap::new();
    let mut observed = Vec::with_capacity(paths.len());
    for (path, valid) in paths.iter().zip(&validated) {
        let chain = parent_chain(worktree, valid, &path.display, &mut folder_listings)?;
        let blocked = chain.blocked;
        let mut item = Observed {
            index: index.remove(&valid.bytes).unwrap_or_default(),
            entry: None,
            directory: false,
            folder_permissions: None,
            chain,
        };
        let location = match inspect_path(worktree, valid)? {
            Some(location) if !blocked => location,
            _ => {
                observed.push(item);
                continue;
            }
        };
        let metadata = match fs::symlink_metadata(&location) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                observed.push(item);
                continue;
            }
            Err(error) => return Err(format!("{} could not be inspected: {error}", path.display)),
        };
        refuse_alias(&location, &path.display, &mut folder_listings)?;
        for identity in entry_identity(&location, &metadata, &mut folder_listings)? {
            if let Some(other) = entries_on_disk.insert(identity, path) {
                return Err(format!(
                    "{} and {} are spelled differently but are one file on this file system, so Repola cannot save them separately. Undo this change with Git instead.",
                    other.display, path.display
                ));
            }
        }
        let file_type = metadata.file_type();
        if file_type.is_dir() {
            item.directory = true;
            item.folder_permissions = permission_bits(&metadata);
            observed.push(item);
            continue;
        }
        // Windows refuses to remove or replace a read-only file, so nothing
        // is saved or changed until the attribute is cleared.
        #[cfg(windows)]
        if metadata.permissions().readonly() {
            return Err(format!(
                "{} is read-only. Clear its read-only attribute, then try again.",
                path.display
            ));
        }
        item.entry = Some(if file_type.is_symlink() {
            let target = fs::read_link(&location)
                .map_err(|error| format!("{} could not be read: {error}", path.display))?;
            let target = link_target_bytes(target, &path.display)?;
            let size = target.len() as u64;
            (
                WorktreeEntryKind::Symlink,
                size,
                None,
                Source::Link(target, is_directory_link(&metadata)),
            )
        } else if file_type.is_file() {
            let kind = if is_executable(&metadata, &item.index) {
                WorktreeEntryKind::Executable
            } else {
                WorktreeEntryKind::File
            };
            (
                kind,
                metadata.len(),
                permission_bits(&metadata),
                Source::File(valid.relative().into_os_string()),
            )
        } else {
            return Err(format!(
                "{} is neither a file nor a directory in the working copy, so Repola cannot save it.",
                path.display
            ));
        });
        observed.push(item);
    }

    let files: Vec<(OsString, u64)> = observed
        .iter()
        .filter_map(|item| match &item.entry {
            Some((_, size, _, Source::File(path))) => Some((path.clone(), *size)),
            _ => None,
        })
        .collect();
    let mut file_oids = hash_files(worktree, files, store)?.into_iter();
    let mut states = Vec::with_capacity(observed.len());
    for ((path, valid), item) in paths.iter().zip(&validated).zip(observed) {
        let worktree_entry = match item.entry {
            None => None,
            Some((kind, size, permissions, source)) => {
                let directory_link = matches!(source, Source::Link(_, true));
                let oid = match source {
                    Source::File(_) => file_oids
                        .next()
                        .ok_or_else(|| "Git did not hash every working-tree file.".to_string())?,
                    Source::Link(target, _) => hash_bytes(worktree, &target, store)?,
                };
                Some(WorktreeEntry {
                    kind,
                    oid,
                    size,
                    permissions,
                    directory_link,
                })
            }
        };
        states.push(PathState {
            path: path.clone(),
            index: item.index,
            worktree: worktree_entry,
            directory: item.directory,
            folder_permissions: item.folder_permissions,
            parents: item.chain.existing,
            parent_permissions: item.chain.permissions,
            blocked: item.chain.blocked,
            index_beneath: beneath.get(&valid.bytes).copied().unwrap_or(0),
            indexed_parent: parent_paths(&valid.bytes)
                .find(|parent| above.contains(*parent))
                .map(|parent| parent.split(|byte| *byte == b'/').count()),
        });
    }
    Ok(states)
}

/// Every proper parent of `path`: `a` and `a/b` for `a/b/c`.
fn parent_paths(path: &[u8]) -> impl Iterator<Item = &[u8]> {
    path.iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'/')
        .map(move |(index, _)| &path[..index])
}

/// The parents of `paths` that the index holds as entries of their own, at
/// any stage. Such a parent has nothing beneath it in the index, so listing
/// everything under each path's top folder includes it; the listing reads no
/// objects and goes to a file, however much lies beneath.
fn indexed_parents(worktree: &Path, paths: &[WorktreePath]) -> Result<HashSet<Vec<u8>>, String> {
    let parents: HashSet<&[u8]> = paths
        .iter()
        .flat_map(|path| parent_paths(&path.bytes))
        .collect();
    let tops: HashSet<&[u8]> = parents
        .iter()
        .map(|parent| leading_components(parent, 1))
        .collect();
    let mut indexed = HashSet::new();
    let pathspecs = tops
        .into_iter()
        .map(|top| os_string_from_path_bytes(top.to_vec()).map(literal_pathspec))
        .collect::<Result<Vec<_>, _>>()?;
    for chunk in argument_chunks(pathspecs) {
        let mut args = ["ls-files", "-z", "--"].map(OsString::from).to_vec();
        args.extend(chunk);
        let file = git_to_temporary_file(worktree, args, "read the index entries")?;
        for_each_record(file, |path| {
            if parents.contains(path) {
                indexed.insert(path.to_vec());
            }
            Ok(())
        })?;
    }
    Ok(indexed)
}

/// How many of `path`'s parent folders exist as real folders, counted from
/// the top, and whether the first that does not is a file or a link rather
/// than missing.
fn parent_chain(
    worktree: &Path,
    path: &WorktreePath,
    display: &str,
    folder_listings: &mut HashMap<PathBuf, FolderListing>,
) -> Result<ParentChain, String> {
    let parents = &path.components[..path.components.len().saturating_sub(1)];
    let mut chain = ParentChain {
        existing: 0,
        blocked: false,
        permissions: Vec::new(),
    };
    let mut current = worktree.to_path_buf();
    for component in parents {
        current.push(component);
        let found = fs::symlink_metadata(&current);
        if found.is_ok() {
            refuse_alias(&current, display, folder_listings)?;
        }
        match found {
            Ok(metadata) if metadata.file_type().is_dir() => {
                chain.existing += 1;
                chain.permissions.extend(permission_bits(&metadata));
            }
            Ok(_) => {
                chain.blocked = true;
                return Ok(chain);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(chain),
            Err(error) => {
                return Err(format!(
                    "{} could not be inspected: {error}",
                    current.display()
                ))
            }
        }
    }
    Ok(chain)
}

/// The folders above a path that exist as real folders, from the top.
struct ParentChain {
    existing: usize,
    /// The first one that does not is a file or a link rather than missing.
    blocked: bool,
    permissions: Vec<u32>,
}

/// A folder's entries, read once.
struct FolderListing {
    names: HashSet<OsString>,
    /// Each name with its inode, to find the names of a link.
    #[cfg(unix)]
    inodes: Vec<(OsString, u64)>,
    /// Every name's case forms, worked out when a name is first not found as
    /// spelled.
    forms: OnceCell<HashSet<(usize, String)>>,
}

impl FolderListing {
    fn read(folder: &Path) -> Result<Self, String> {
        let mut listing = Self {
            names: HashSet::new(),
            #[cfg(unix)]
            inodes: Vec::new(),
            forms: OnceCell::new(),
        };
        for entry in fs::read_dir(folder)
            .map_err(|error| format!("{} could not be read: {error}", folder.display()))?
        {
            let entry = entry.map_err(|error| error.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirEntryExt;
                listing.inodes.push((entry.file_name(), entry.ino()));
            }
            listing.names.insert(entry.file_name());
        }
        Ok(listing)
    }

    /// Whether the folder stores `name` as spelled or under a spelling a file
    /// system may take as the same name.
    fn stores(&self, name: &OsStr) -> bool {
        if self.names.contains(name) {
            return true;
        }
        let forms = self.forms.get_or_init(|| {
            self.names
                .iter()
                .flat_map(|stored| {
                    case_forms(&stored.to_string_lossy())
                        .into_iter()
                        .enumerate()
                })
                .collect()
        });
        case_forms(&name.to_string_lossy())
            .into_iter()
            .enumerate()
            .any(|form| forms.contains(&form))
    }
}

fn folder_listing<'a>(
    folder: &Path,
    listings: &'a mut HashMap<PathBuf, FolderListing>,
) -> Result<&'a FolderListing, String> {
    Ok(match listings.entry(folder.to_path_buf()) {
        Entry::Occupied(listing) => listing.into_mut(),
        Entry::Vacant(listing) => listing.insert(FolderListing::read(folder)?),
    })
}

/// Refuses a path that reaches an entry through a name its folder does not
/// store. A file system can find an entry under such an alias, as Windows
/// finds one under the short name it makes up for a long name, and `GIT~2`
/// can be the `.git` folder itself.
fn refuse_alias(
    location: &Path,
    display: &str,
    listings: &mut HashMap<PathBuf, FolderListing>,
) -> Result<(), String> {
    let (Some(folder), Some(name)) = (location.parent(), location.file_name()) else {
        return Ok(());
    };
    if folder_listing(folder, listings)?.stores(name) {
        Ok(())
    } else {
        Err(format!(
            "{display} reaches {} through a name its folder does not store, such as a short name Windows makes up, so Repola will not change it. Rename it with Git first.",
            location.display()
        ))
    }
}

/// What names one entry on disk, so two paths sharing any of these are one
/// entry under two spellings. On Unix it is the real path, which on macOS is
/// spelled as the folder stores the name, and, for a folder or a file with one
/// link, its device and inode, which also covers file systems whose real paths
/// keep the spelling asked for. Hard links are separate entries by design.
#[cfg(unix)]
fn entry_identity(
    location: &Path,
    metadata: &fs::Metadata,
    folder_listings: &mut HashMap<PathBuf, FolderListing>,
) -> Result<Vec<OsString>, String> {
    use std::os::unix::fs::MetadataExt;
    let mut identities = Vec::new();
    let mut real_path = |real: PathBuf| {
        let mut identity = OsString::from("path:");
        identity.push(real);
        identities.push(identity);
    };
    if !metadata.file_type().is_symlink() {
        real_path(
            dunce::canonicalize(location).map_err(|error| {
                format!("{} could not be inspected: {error}", location.display())
            })?,
        );
    } else if let (Some(parent), Some(name)) = (location.parent(), location.file_name()) {
        // Resolving a link would follow it, so its real path is its folder's
        // and the name stored there: the one asked for if it is stored as is,
        // or else each name stored for the link's own inode.
        let folder = dunce::canonicalize(parent)
            .map_err(|error| format!("{} could not be inspected: {error}", parent.display()))?;
        let listing = folder_listing(parent, folder_listings)?;
        if listing.names.contains(name) {
            real_path(folder.join(name));
        } else {
            for (stored, _) in listing
                .inodes
                .iter()
                .filter(|(_, inode)| *inode == metadata.ino())
            {
                real_path(folder.join(stored));
            }
        }
    }
    if metadata.is_dir() || metadata.nlink() == 1 {
        identities.push(OsString::from(format!(
            "inode:{}:{}",
            metadata.dev(),
            metadata.ino()
        )));
    }
    Ok(identities)
}

/// What names one entry on disk, so two paths with the same identity are one
/// entry under two spellings. On Windows it is the folder's real path and the
/// name the folder stores, whatever spelling found it and whatever
/// `core.ignorecase` says.
#[cfg(windows)]
fn entry_identity(
    location: &Path,
    _metadata: &fs::Metadata,
    folder_listings: &mut HashMap<PathBuf, FolderListing>,
) -> Result<Vec<OsString>, String> {
    let (Some(parent), Some(name)) = (location.parent(), location.file_name()) else {
        return Ok(Vec::new());
    };
    let folder = dunce::canonicalize(parent)
        .map_err(|error| format!("{} could not be inspected: {error}", parent.display()))?;
    let listing = folder_listing(parent, folder_listings)?;
    if listing.names.contains(name) {
        return Ok(vec![folder.join(name).into_os_string()]);
    }
    // The file system found the name under another spelling, so the folder
    // stores it under a name sharing one of its forms. Any that does counts.
    let wanted = case_forms(&name.to_string_lossy());
    Ok(listing
        .names
        .iter()
        .filter(|stored| {
            let forms = case_forms(&stored.to_string_lossy());
            forms[0] == wanted[0] || forms[1] == wanted[1]
        })
        .map(|stored| folder.join(stored).into_os_string())
        .collect())
}

#[cfg(windows)]
fn is_directory_link(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::FileTypeExt;
    metadata.file_type().is_symlink_dir()
}

/// Unix links have no kind of their own.
#[cfg(unix)]
fn is_directory_link(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn permission_bits(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(metadata.permissions().mode() & 0o7777)
}

#[cfg(windows)]
fn permission_bits(_metadata: &fs::Metadata) -> Option<u32> {
    None
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

/// The index entries of some paths, and which of those paths have entries
/// beneath them.
struct IndexListing {
    entries: HashMap<Vec<u8>, Vec<IndexEntry>>,
    beneath: HashMap<Vec<u8>, usize>,
}

fn index_entries(
    worktree: &Path,
    head: Option<&str>,
    paths: &[WorktreePath],
) -> Result<IndexListing, String> {
    let base = match head {
        Some(head) => head.to_string(),
        None => empty_tree(worktree)?,
    };
    let wanted: HashSet<&[u8]> = paths.iter().map(|path| path.bytes.as_slice()).collect();
    let mut entries: HashMap<Vec<u8>, Vec<IndexEntry>> = HashMap::new();
    let mut beneath: HashMap<Vec<u8>, usize> = HashMap::new();
    // Conflicted paths list one entry per stage; each path counts once.
    let mut counted: HashSet<Vec<u8>> = HashSet::new();
    let mut record = |record: &[u8], intent_to_add: &HashSet<Vec<u8>>| -> Result<(), String> {
        let (fields, path) = split_once(record, b'\t')
            .ok_or_else(|| "Git returned a malformed index entry.".to_string())?;
        // Even an entry that is wanted itself can lie beneath another.
        if counted.insert(path.to_vec()) {
            for parent in parent_paths(path).filter(|parent| wanted.contains(parent)) {
                *beneath.entry(parent.to_vec()).or_default() += 1;
            }
        }
        if !wanted.contains(path) {
            return Ok(());
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
        Ok(())
    };
    // `-v` prefixes each entry with a tag that carries its flags: `S` for
    // skip-worktree, and lowercase for assume-unchanged.
    let listing = ["ls-files", "--stage", "-v", "-z"].map(OsString::from);
    if paths.len() > PATHSPEC_LIMIT {
        let intent_to_add = intent_to_add_paths(worktree, &base, &[])?;
        let mut args = listing.to_vec();
        args.push(OsString::from("--"));
        let file = git_to_temporary_file(worktree, args, "read the index entries")?;
        for_each_record(file, |item| record(item, &intent_to_add))?;
    } else {
        for chunk in argument_chunks(literal_pathspecs(paths)?) {
            let intent_to_add = intent_to_add_paths(worktree, &base, &chunk)?;
            let mut args = listing.to_vec();
            args.push(OsString::from("--"));
            args.extend(chunk);
            let output = git_stdout(worktree, args, "read the index entries")?;
            for item in output
                .split(|byte| *byte == 0)
                .filter(|item| !item.is_empty())
            {
                record(item, &intent_to_add)?;
            }
        }
    }
    Ok(IndexListing { entries, beneath })
}

/// What the last commit holds at a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HeadEntry {
    pub mode: String,
    /// `blob`, `tree`, or `commit` (a submodule).
    pub kind: String,
    pub oid: String,
}

/// HEAD's entries at `paths`; a path HEAD lacks is absent from the map.
pub(super) fn head_entries(
    worktree: &Path,
    head: &str,
    paths: &[GitPath],
) -> Result<HashMap<Vec<u8>, HeadEntry>, String> {
    let validated = paths
        .iter()
        .map(WorktreePath::new)
        .collect::<Result<Vec<_>, _>>()?;
    let wanted: HashSet<&[u8]> = validated.iter().map(|path| path.bytes.as_slice()).collect();
    let mut entries = HashMap::new();
    let mut record = |record: &[u8]| -> Result<(), String> {
        let (fields, path) = split_once(record, b'\t')
            .ok_or_else(|| "Git returned a malformed tree entry.".to_string())?;
        if !wanted.contains(path) {
            return Ok(());
        }
        let fields = String::from_utf8_lossy(fields);
        let mut fields = fields.split(' ');
        let (Some(mode), Some(kind), Some(oid), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err("Git returned a malformed tree entry.".into());
        };
        entries.insert(
            path.to_vec(),
            HeadEntry {
                mode: mode.to_string(),
                kind: kind.to_string(),
                oid: oid.to_string(),
            },
        );
        Ok(())
    };
    if validated.len() > PATHSPEC_LIMIT {
        let file = git_to_temporary_file(
            worktree,
            ["ls-tree", "-r", "-t", "-z", "--full-tree", head],
            "read the last commit",
        )?;
        for_each_record(file, record)?;
    } else {
        for chunk in argument_chunks(literal_pathspecs(&validated)?) {
            let mut args = ["ls-tree", "-z", "--full-tree", head, "--"]
                .map(OsString::from)
                .to_vec();
            args.extend(chunk);
            let output = git_stdout(worktree, args, "read the last commit")?;
            for item in output
                .split(|byte| *byte == 0)
                .filter(|item| !item.is_empty())
            {
                record(item)?;
            }
        }
    }
    Ok(entries)
}

fn literal_pathspecs(paths: &[WorktreePath]) -> Result<Vec<OsString>, String> {
    paths
        .iter()
        .map(|path| os_string_from_path_bytes(path.bytes.clone()).map(literal_pathspec))
        .collect()
}

/// Calls `each` for every NUL-terminated record in `file`.
fn for_each_record(
    file: fs::File,
    mut each: impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<(), String> {
    use std::io::BufRead;
    let mut reader = BufReader::new(file);
    let mut item = Vec::new();
    loop {
        item.clear();
        let read = reader
            .read_until(0, &mut item)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Ok(());
        }
        if item.last() == Some(&0) {
            item.pop();
        }
        if !item.is_empty() {
            each(&item)?;
        }
    }
}

/// The paths among `pathspecs` (every index entry when there are none) whose
/// index entry was recorded with `git add --intent-to-add`, which `ls-files`
/// does not report.
fn intent_to_add_paths(
    worktree: &Path,
    base: &str,
    pathspecs: &[OsString],
) -> Result<HashSet<Vec<u8>>, String> {
    // Git compares an intent-to-add entry as an empty file in one mode and as
    // absent in the other, so its status differs between them; every other
    // entry compares the same way in both.
    let listed = |visibility: &str| -> Result<HashMap<Vec<u8>, Vec<u8>>, String> {
        let mut args = [
            "diff-index",
            "--cached",
            "--name-status",
            "--no-renames",
            "-z",
            visibility,
            base,
            "--",
        ]
        .map(OsString::from)
        .to_vec();
        args.extend(pathspecs.iter().cloned());
        let file = git_to_temporary_file(worktree, args, "read the intent-to-add entries")?;
        let mut statuses = HashMap::new();
        let mut status: Option<Vec<u8>> = None;
        for_each_record(file, |item| {
            match status.take() {
                None => status = Some(item.to_vec()),
                Some(code) => {
                    statuses.insert(item.to_vec(), code);
                }
            }
            Ok(())
        })?;
        if status.is_some() {
            return Err("Git returned a malformed index comparison.".into());
        }
        Ok(statuses)
    };
    let visible = listed("--ita-visible-in-index")?;
    let hidden = listed("--ita-invisible-in-index")?;
    Ok(visible
        .keys()
        .chain(hidden.keys())
        .filter(|path| visible.get(*path) != hidden.get(*path))
        .cloned()
        .collect())
}

/// The empty tree's object ID in this repository's hash format, which Git
/// knows without storing it.
fn empty_tree(worktree: &Path) -> Result<String, String> {
    let output = command::git_at_with_input(
        worktree,
        HooksOff::new()?.args(["hash-object", "-t", "tree", "--stdin"]),
        b"",
    )
    .map_err(|error| error.to_string())?;
    let oid = successful_stdout(output, "name the empty tree")?;
    Ok(String::from_utf8_lossy(&oid).trim().to_string())
}

/// Hashes `files` (paths with their sizes), a bounded amount of content per
/// Git process so none runs into the per-command time limit.
fn hash_files(
    worktree: &Path,
    files: Vec<(OsString, u64)>,
    store: bool,
) -> Result<Vec<String>, String> {
    let mut oids = Vec::with_capacity(files.len());
    let mut chunks: Vec<Vec<OsString>> = Vec::new();
    let mut bytes = 0;
    for (file, size) in files {
        match chunks.last_mut() {
            Some(chunk) if bytes + size <= HASH_CHUNK_BYTES => {
                bytes += size;
                chunk.push(file);
            }
            _ => {
                bytes = size;
                chunks.push(vec![file]);
            }
        }
    }
    for chunk in chunks.into_iter().flat_map(argument_chunks) {
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
    let output = command::git_at_with_input(worktree, HooksOff::new()?.args(args), bytes)
        .map_err(|error| error.to_string())?;
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

/// A recovery point whose content and tree are stored, but which no reference
/// names yet.
pub(super) struct PreparedPoint {
    tree: String,
    stamp: String,
    record: Summary,
}

/// Stores the tree a recovery point of `states` needs. Their content must
/// already be stored (see `store_contents`); writing the tree checks it is.
pub(super) fn prepare_recovery_point(
    worktree: &Path,
    kind: RecoveryPointKind,
    summary: String,
    head: Option<&str>,
    states: &[PathState],
) -> Result<PreparedPoint, String> {
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
    let serialize =
        |record: &Summary| serde_json::to_vec(record).map_err(|error| error.to_string());
    let mut summary_json = serialize(&record)?;
    for state in states.iter().take(SUMMARY_SAMPLE_PATHS) {
        record.paths.push(state.path.clone());
        let sampled = serialize(&record)?;
        if sampled.len() > MAX_SUMMARY_BYTES {
            record.paths.pop();
            break;
        }
        summary_json = sampled;
    }
    if summary_json.len() > MAX_SUMMARY_BYTES {
        return Err(
            "The description of this change is too long to save as a recovery point, so nothing was changed."
                .into(),
        );
    }
    let manifest = Manifest {
        version: MANIFEST_VERSION,
        paths: states.to_vec(),
    };
    let summary_oid = hash_bytes(worktree, &summary_json, true)?;
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
    let hooks_off = HooksOff::new()?;
    let output = command::git_at_with_input_and_env(
        worktree,
        hooks_off.args(["update-index", "-z", "--index-info"]),
        &input,
        environment,
    )
    .map_err(|error| error.to_string())?;
    successful_stdout(output, "assemble the recovery point")?;
    let output = command::git_at_with_env(worktree, hooks_off.args(["write-tree"]), environment)
        .map_err(|error| error.to_string())?;
    let tree = String::from_utf8_lossy(&successful_stdout(output, "write the recovery point")?)
        .trim()
        .to_string();
    if !is_object_id(&tree) {
        return Err("Git returned a malformed recovery-point tree.".into());
    }
    Ok(PreparedPoint {
        tree,
        stamp,
        record,
    })
}

/// Names a prepared recovery point. The reference is created only if it does
/// not exist yet, so no recovery point can ever be overwritten.
pub(super) fn create_recovery_point(
    worktree: &Path,
    prepared: PreparedPoint,
) -> Result<RecoveryPoint, String> {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let id = format!(
        "{RECOVERY_REF_NAMESPACE}/{}-{}",
        prepared.stamp,
        &suffix[..12]
    );
    create_reference(worktree, &id, &prepared.tree)?;
    Ok(point_from_summary(id, prepared.tree, prepared.record))
}

/// Writes the working-tree content `states` observed to the object store,
/// refusing if any of it changed since, and checks that every stored object
/// has the size observed on disk.
pub(super) fn store_contents(worktree: &Path, states: &[PathState]) -> Result<(), String> {
    let changed = || CHANGED_WHILE_SAVING.to_string();
    let mut files = Vec::new();
    let mut expected = Vec::new();
    for state in states {
        let Some(entry) = &state.worktree else {
            continue;
        };
        let valid = WorktreePath::new(&state.path)?;
        if entry.kind == WorktreeEntryKind::Symlink {
            let location = inspect_path(worktree, &valid)?.ok_or_else(changed)?;
            let target = fs::read_link(&location).map_err(|_| changed())?;
            let target = link_target_bytes(target, &state.path.display)?;
            if hash_bytes(worktree, &target, true)? != entry.oid {
                return Err(changed());
            }
        } else {
            files.push((valid.relative().into_os_string(), entry.size));
            expected.push(entry.oid.as_str());
        }
    }
    let stored = hash_files(worktree, files, true)?;
    if stored
        .iter()
        .map(String::as_str)
        .ne(expected.iter().copied())
    {
        return Err(changed());
    }
    let objects: Vec<(&str, u64)> = states
        .iter()
        .filter_map(|state| state.worktree.as_ref())
        .map(|entry| (entry.oid.as_str(), entry.size))
        .collect();
    if !objects_have_sizes(worktree, &objects)? {
        return Err("Git did not store the saved content intact, so nothing was changed.".into());
    }
    Ok(())
}

/// Whether every one of `objects` is stored with the size given beside it.
/// Git reports sizes without reading the content.
fn objects_have_sizes(worktree: &Path, objects: &[(&str, u64)]) -> Result<bool, String> {
    for chunk in objects.chunks(SUMMARIES_PER_READ) {
        let input: String = chunk.iter().map(|(oid, _)| format!("{oid}\n")).collect();
        let output = command::git_at_with_input(
            worktree,
            HooksOff::new()?.args(["cat-file", "--batch-check=%(objectname) %(objectsize)"]),
            input.as_bytes(),
        )
        .map_err(|error| error.to_string())?;
        let listed = successful_stdout(output, "check the saved content")?;
        let sizes: Vec<String> = String::from_utf8_lossy(&listed)
            .lines()
            .map(str::to_string)
            .collect();
        let intact = sizes.len() == chunk.len()
            && sizes
                .iter()
                .zip(chunk)
                .all(|(line, (oid, size))| *line == format!("{oid} {size}"));
        if !intact {
            return Ok(false);
        }
    }
    Ok(true)
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

/// Leading options for every Git command Repola runs to save, discard,
/// restore, or delete. Hooks and fsmonitor programs are user code that could
/// change the working copy between Repola's last look and its change, so none
/// runs; the hooks folder is empty.
pub(super) struct HooksOff {
    _folder: tempfile::TempDir,
    options: [OsString; 4],
}

impl HooksOff {
    pub(super) fn new() -> Result<Self, String> {
        let folder = tempfile::tempdir()
            .map_err(|error| format!("Could not create an empty hooks folder: {error}"))?;
        let mut hooks = OsString::from("core.hooksPath=");
        hooks.push(folder.path());
        Ok(Self {
            options: [
                "-c".into(),
                hooks,
                "-c".into(),
                "core.fsmonitor=false".into(),
            ],
            _folder: folder,
        })
    }

    pub(super) fn args<I, S>(&self, args: I) -> Vec<OsString>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.options
            .iter()
            .cloned()
            .chain(args.into_iter().map(|arg| arg.as_ref().to_os_string()))
            .collect()
    }
}

pub(super) fn push_index_info(input: &mut Vec<u8>, mode: &str, oid: &str, path: &[u8]) {
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
        // A sample's names are shown as their tokens spell them, so a crafted
        // summary cannot list one file under another's name.
        paths: summary
            .paths
            .iter()
            .filter_map(|path| decode_path_token_bytes(&path.token).ok())
            .map(|bytes| git_path(&bytes))
            .collect(),
        stored_bytes: summary.stored_bytes,
    }
}

/// Lists the repository's recovery points, newest first, as many as fit in
/// one response; the rest are counted. Points saved from every worktree of the
/// repository share one list.
pub fn list_recovery_points(request: WorkingCopyRequest) -> Result<RecoveryPointList, String> {
    let snapshot = working_copy_snapshot(request)?;
    let mut points = list(Path::new(&snapshot.worktree_path))?;
    let mut budget = MAX_LISTED_BYTES;
    let omitted = keep_within(&mut points, &mut budget)?;
    Ok(RecoveryPointList { points, omitted })
}

/// Keeps the leading `items` that fit in `budget` bytes once encoded, takes
/// their size from it, and returns how many were left out.
pub(super) fn keep_within<T: Serialize>(
    items: &mut Vec<T>,
    budget: &mut usize,
) -> Result<u64, String> {
    let mut fits = 0;
    for item in items.iter() {
        let size = serde_json::to_vec(item)
            .map_err(|error| error.to_string())?
            .len();
        if size > *budget {
            break;
        }
        *budget -= size;
        fits += 1;
    }
    Ok(items.split_off(fits).len() as u64)
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
        HooksOff::new()?.args([
            "cat-file",
            "--batch-check=%(objectname) %(objecttype) %(objectsize)",
        ]),
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
        let output = command::git_at_with_input(
            worktree,
            HooksOff::new()?.args(["cat-file", "--batch"]),
            input.as_bytes(),
        )
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
        HooksOff::new()?.args([
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            reference.id.as_str(),
        ]),
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
        // Reviews show a path's name and restores act on its token, so the
        // two must agree, and a token has one spelling so no path is saved
        // twice under two.
        let path = decode_path_token_bytes(&state.path.token)?;
        if state.path != git_path(&path) || !seen.insert(state.path.token.as_str()) {
            return Err(damaged());
        }
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
    // Previews and restores rely on the sizes recorded for saved files.
    let saved_files: Vec<(&str, u64)> = states
        .iter()
        .filter_map(|state| state.worktree.as_ref())
        .map(|entry| (entry.oid.as_str(), entry.size))
        .collect();
    if !objects_have_sizes(worktree, &saved_files)? {
        return Err(damaged());
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
    refuse_unsafe_restore(worktree, &saved, &current)?;
    let mut entries: Vec<RecoveryRestoreEntry> = saved
        .iter()
        .zip(&current)
        .map(|(saved, current)| RecoveryRestoreEntry {
            path: saved.path.clone(),
            worktree: restore_effect(saved, current),
            index_changes: saved.index != current.index,
        })
        .collect();
    // Only as many entries as one response carries; the rest are counted.
    let mut budget = MAX_LISTED_BYTES;
    let omitted = keep_within(&mut entries, &mut budget)?;
    Ok(RecoveryRestorePlan {
        fingerprint: fingerprint(
            snapshot.head.as_deref(),
            snapshot.operation,
            &current,
            &request.point,
        )?,
        point,
        entries,
        omitted,
    })
}

/// What one discard or restore removes before it writes anything.
#[derive(Default)]
pub(super) struct Removals<'a> {
    /// Files removed from disk.
    pub files: HashSet<&'a [u8]>,
    /// Paths whose index entries are removed.
    pub entries: HashSet<&'a [u8]>,
}

impl Removals<'_> {
    /// Whether the folder at `path` is gone, or empty, by the time a file is
    /// written in its place: everything in it is removed first.
    pub(super) fn empties_folder(&self, worktree: &Path, path: &GitPath) -> Result<bool, String> {
        let valid = WorktreePath::new(path)?;
        let Some(location) = inspect_path(worktree, &valid)? else {
            return Ok(true);
        };
        let bytes = decode_path_token_bytes(&path.token)?;
        let mut removed = HashSet::new();
        for file in self.files.iter().filter(|removed| {
            removed.len() > bytes.len()
                && removed.starts_with(&bytes)
                && removed[bytes.len()] == b'/'
        }) {
            let mut location = worktree.to_path_buf();
            for component in file.split(|byte| *byte == b'/') {
                location.push(os_string_from_path_bytes(component.to_vec())?);
            }
            removed.insert(location);
        }
        Ok(folder_vanishes(&location, &removed)?.0)
    }

    /// Whether writing `path` would replace something not removed first: a
    /// file or link in the way of one of its folders on disk, or index entries
    /// its own entry would displace.
    pub(super) fn crossed_by(
        &self,
        path: &[u8],
        state: &PathState,
        writes_disk: bool,
        writes_index: bool,
    ) -> bool {
        let removed_beneath = self
            .entries
            .iter()
            .filter(|removed| {
                removed.len() > path.len()
                    && removed.starts_with(path)
                    && removed[path.len()] == b'/'
            })
            .count();
        writes_disk
            && state.blocked
            && !self
                .files
                .contains(leading_components(path, state.parents + 1))
            || writes_index
                && (state.indexed_parent.is_some_and(|components| {
                    !self.entries.contains(leading_components(path, components))
                }) || removed_beneath != state.index_beneath)
    }
}

/// The first `count` components of a slash-separated path: `a/b` for `a/b/c`
/// and 2.
pub(super) fn leading_components(path: &[u8], count: usize) -> &[u8] {
    let end = path
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'/')
        .nth(count.saturating_sub(1))
        .map_or(path.len(), |(index, _)| index);
    &path[..end]
}

pub(super) fn file_folder_conflict(display: &str) -> String {
    format!(
        "{display} is a file in one place and a folder in another (in the last commit, the staging area, or on disk), so Repola will not change it. Sort it out with Git first."
    )
}

/// What restoring `saved` over `current` writes: the file on disk, including
/// the empty stand-in a deleted intent-to-add entry needs while it is
/// recorded, and the index entries.
fn restore_writes(saved: &PathState, current: &PathState) -> (bool, bool) {
    let writes_disk = saved.worktree.is_some() && saved.worktree != current.worktree
        || saved.directory && !current.directory
        || saved.worktree.is_none()
            && saved.index != current.index
            && saved.index.iter().any(|entry| entry.intent_to_add);
    let writes_index = !saved.index.is_empty() && saved.index != current.index;
    (writes_disk, writes_index)
}

/// A restore never writes where it would replace something it has not
/// saved: a folder with something in it, or a file or index entries in the
/// way that the restore does not itself remove first.
fn refuse_unsafe_restore(
    worktree: &Path,
    saved: &[PathState],
    current: &[PathState],
) -> Result<(), String> {
    let paths = saved
        .iter()
        .map(|state| decode_path_token_bytes(&state.path.token))
        .collect::<Result<Vec<_>, _>>()?;
    let long_paths = long_paths(worktree)?;
    let mut removals = Removals::default();
    for ((saved, current), path) in saved.iter().zip(current).zip(&paths) {
        // Intent-to-add entries go back through `git add -N`, which must find
        // the file where Git can reach it.
        let adds_intent =
            saved.index != current.index && saved.index.iter().any(|entry| entry.intent_to_add);
        if adds_intent && !git_can_reach(worktree, path, long_paths) {
            return Err(format!(
                "{} is too long a path for Git to reach on Windows. Turn on core.longpaths, or move the working copy to a shorter folder, then review the restore again.",
                saved.path.display
            ));
        }
        if saved.worktree.is_none() && current.worktree.is_some() {
            removals.files.insert(path);
        }
        if saved.index.is_empty() && !current.index.is_empty() {
            removals.entries.insert(path);
        }
    }
    for ((saved, current), path) in saved.iter().zip(current).zip(&paths) {
        if !current.differs_from(saved) {
            continue;
        }
        let (writes_disk, writes_index) = restore_writes(saved, current);
        if writes_disk && current.directory && !removals.empties_folder(worktree, &saved.path)? {
            return Err(format!(
                "{} is now a directory in the working copy, so Repola will not replace it. Move it aside and review the restore again.",
                saved.path.display
            ));
        }
        if removals.crossed_by(path, current, writes_disk, writes_index) {
            return Err(file_folder_conflict(&saved.path.display));
        }
    }
    Ok(())
}

fn restore_effect(saved: &PathState, current: &PathState) -> RestoreEffect {
    if saved.directory && !current.directory {
        return RestoreEffect::CreateFolder;
    }
    if current.folder_permissions_differ(saved) {
        return RestoreEffect::FolderPermissions;
    }
    match (saved.worktree.as_ref(), current.worktree.as_ref()) {
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
    let head = snapshot.head.as_deref();
    let current = observe_paths(&worktree, head, &paths, false)?;
    if fingerprint(head, snapshot.operation, &current, &request.point)? != request.fingerprint {
        return Err(
            "The working copy changed after this restore was reviewed. Review it again.".into(),
        );
    }
    refuse_unsafe_restore(&worktree, &saved, &current)?;
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
    let prepared = if replaced_states.iter().any(PathState::has_content) {
        store_contents(&worktree, &replaced_states)?;
        Some(prepare_recovery_point(
            &worktree,
            RecoveryPointKind::Restore,
            format!("Replaced while restoring “{}”", point.summary),
            head,
            &replaced_states,
        )?)
    } else {
        None
    };
    // The last look before writing, at HEAD and the in-progress operation as
    // well as the paths: only creating the reference and the restore itself
    // follow.
    let latest = working_copy_snapshot(WorkingCopyRequest {
        repository_path: snapshot.repository_path.clone(),
        worktree_path: snapshot.worktree_path.clone(),
    })?;
    let latest_paths = observe_paths(&worktree, latest.head.as_deref(), &paths, false)?;
    if fingerprint(
        latest.head.as_deref(),
        latest.operation,
        &latest_paths,
        &request.point,
    )? != request.fingerprint
    {
        return Err(CHANGED_WHILE_SAVING.into());
    }
    let replaced = prepared
        .map(|prepared| create_recovery_point(&worktree, prepared))
        .transpose()?;

    apply_restore(&worktree, &point, &changed).map_err(|error| match &replaced {
        Some(replaced) => format!(
            "{error}\n\nThe content this restore was replacing is saved in recovery point {}.",
            replaced.id
        ),
        None => error,
    })?;
    Ok(RecoveryRestoreResult { replaced })
}

fn apply_restore(
    worktree: &Path,
    point: &RecoveryPoint,
    changed: &[(&PathState, &PathState)],
) -> Result<(), String> {
    // Everything in the way goes first: removed files, then empty folders
    // where files go back, before any file or index entry is written.
    let mut folders = Vec::new();
    for (saved, current) in changed {
        if saved.worktree.is_none() && current.worktree.is_some() {
            restore_worktree_entry(worktree, saved, &mut folders)?;
        }
    }
    for (saved, current) in changed {
        if restore_writes(saved, current).0 && current.directory {
            remove_empty_directory(worktree, &saved.path)?;
        }
    }
    for (saved, current) in changed {
        if saved.directory && !current.directory {
            restore_folder(worktree, saved, &mut folders)?;
        } else if current.folder_permissions_differ(saved) {
            let location = inspect_path(worktree, &WorktreePath::new(&saved.path)?)?;
            if let (Some(location), Some(mode)) = (location, saved.folder_permissions) {
                folders.push((location, mode));
            }
        }
    }
    for (saved, current) in changed {
        if saved.worktree.is_some() && saved.worktree != current.worktree {
            restore_worktree_entry(worktree, saved, &mut folders)?;
        }
    }
    // Recreated folders get their permissions once everything inside them is
    // written, deepest first, so even a folder that is not writable is filled.
    folders.sort_by_key(|(folder, _)| std::cmp::Reverse(folder.components().count()));
    for (folder, mode) in folders {
        set_folder_permissions(&folder, mode)
            .map_err(|error| format!("{} could not be restored: {error}", folder.display()))?;
    }
    let absent = "0".repeat(point.oid.len());
    // Every changed path's entries are removed before any saved entry is
    // added back, so none of them displaces another.
    let mut removals = Vec::new();
    let mut input = Vec::new();
    let mut intents = Vec::new();
    let mut placeholders = Vec::new();
    for (saved, current) in changed {
        if saved.index == current.index {
            continue;
        }
        let path = decode_path_token_bytes(&saved.path.token)?;
        // A zero mode removes every stage of the path.
        push_index_info(&mut removals, "0", &absent, &path);
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
    removals.extend(input);
    let hooks_off = HooksOff::new()?;
    if !removals.is_empty() {
        let output = command::git_at_with_input_timeout(
            worktree,
            hooks_off.args(["update-index", "-z", "--index-info"]),
            &removals,
            CHANGE_TIMEOUT,
        )
        .map_err(|error| error.to_string())?;
        successful_stdout(output, "restore the saved index entries")?;
    }
    if !intents.is_empty() {
        restore_intents(worktree, &intents, &placeholders)?;
    }
    restore_index_flags(worktree, changed)?;
    // Entries written without stat data would otherwise look modified to
    // commands that do not refresh the index first.
    let output = command::git_at_timeout(
        worktree,
        hooks_off.args(["update-index", "-q", "--unmerged", "--refresh"]),
        CHANGE_TIMEOUT,
    )
    .map_err(|error| error.to_string())?;
    successful_stdout(output, "refresh the index").map(drop)
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
    let mut directories = Vec::new();
    let recorded = placeholders
        .iter()
        .try_for_each(|path| {
            let target = prepare_path(
                worktree,
                &WorktreePath::new(path)?,
                &path.display,
                &mut directories,
            )?;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map_err(|error| format!("{} could not be restored: {error}", path.display))?;
            created.push(*path);
            Ok(())
        })
        .and_then(|()| {
            let output = command::git_at_with_input_timeout(
                worktree,
                HooksOff::new()?.args([
                    "add",
                    "--force",
                    "--intent-to-add",
                    "--pathspec-from-file=-",
                    "--pathspec-file-nul",
                ]),
                intents,
                CHANGE_TIMEOUT,
            )
            .map_err(|error| error.to_string())?;
            successful_stdout(output, "restore the intent-to-add entries").map(drop)
        });
    // Only the stand-ins and the folders made for them go; folders that were
    // already there stay, even when empty. Innermost first, and a folder that
    // something else has since filled stays too.
    for path in created {
        remove_worktree_file(worktree, &WorktreePath::new(path)?, path)?;
    }
    for directory in directories.iter().rev() {
        let _ = fs::remove_dir(directory);
    }
    recorded
}

fn config_bool(worktree: &Path, key: &str, default: bool) -> Result<bool, String> {
    let output = command::git_at(
        worktree,
        HooksOff::new()?.args(["config", "--type=bool", "--get", key]),
    )
    .map_err(|error| error.to_string())?;
    match output.status.code() {
        Some(0) => Ok(String::from_utf8_lossy(&output.stdout).trim() == "true"),
        Some(1) => Ok(default),
        _ => Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
    }
}

/// Recreates the folder that stood at `saved`, adding it and any parents it
/// needs, with the permissions they had, to `folders`.
fn restore_folder(
    worktree: &Path,
    saved: &PathState,
    folders: &mut Vec<(PathBuf, u32)>,
) -> Result<(), String> {
    let valid = WorktreePath::new(&saved.path)?;
    let mut created = Vec::new();
    let target = prepare_path(worktree, &valid, &saved.path.display, &mut created)?;
    let first = valid.components.len() - 1 - created.len();
    for (depth, folder) in (first..).zip(created) {
        if let Some(mode) = saved.parent_permissions.get(depth) {
            folders.push((folder, *mode));
        }
    }
    fs::create_dir(&target)
        .map_err(|error| format!("{} could not be restored: {error}", saved.path.display))?;
    if let Some(mode) = saved.folder_permissions {
        folders.push((target, mode));
    }
    Ok(())
}

/// Writes `saved` back, adding the folders it recreates, with the
/// permissions they had, to `folders`.
fn restore_worktree_entry(
    worktree: &Path,
    saved: &PathState,
    folders: &mut Vec<(PathBuf, u32)>,
) -> Result<(), String> {
    let path = &saved.path;
    let valid = WorktreePath::new(path)?;
    let Some(entry) = &saved.worktree else {
        // Folders that existed when the content was saved stay, even empty.
        if remove_worktree_file(worktree, &valid, path)? {
            remove_empty_parents(worktree, &valid, saved.parents);
        }
        return Ok(());
    };
    let mut created = Vec::new();
    let target = prepare_path(worktree, &valid, &path.display, &mut created)?;
    // The recreated folders are the deepest of the path's parents.
    let first = valid.components.len() - 1 - created.len();
    for (depth, folder) in (first..).zip(created) {
        if let Some(mode) = saved.parent_permissions.get(depth) {
            folders.push((folder, *mode));
        }
    }
    if fs::symlink_metadata(&target).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        return Err(format!(
            "{} is now a directory, so Repola will not replace it.",
            path.display
        ));
    }
    if entry.kind == WorktreeEntryKind::Symlink {
        // Only a real link on disk is saved as one, so it comes back as one
        // whatever `core.symlinks` says.
        let output = command::git_at_timeout(
            worktree,
            HooksOff::new()?.args(["cat-file", "blob", &entry.oid]),
            CHANGE_TIMEOUT,
        )
        .map_err(|error| error.to_string())?;
        let link = successful_stdout(output, "read the saved link")?;
        replace_with_symlink(&target, &link, entry.directory_link, &path.display)
    } else {
        replace_with_blob(worktree, entry, &target, &path.display)
    }
}

/// Removes the folder at `path` once removals have emptied it, refusing if it
/// still holds anything. Removing its last file may already have removed it.
pub(super) fn remove_empty_directory(worktree: &Path, path: &GitPath) -> Result<(), String> {
    let valid = WorktreePath::new(path)?;
    let Some(location) = inspect_path(worktree, &valid)? else {
        return Ok(());
    };
    match fs::symlink_metadata(&location) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Ok(metadata) if metadata.file_type().is_dir() => {}
        _ => {
            return Err(format!(
                "{} is no longer an empty folder, so Repola will not replace it.",
                path.display
            ))
        }
    }
    if fs::remove_dir(&location).is_err() {
        return Err(format!(
            "{} is no longer an empty folder, so Repola will not replace it.",
            path.display
        ));
    }
    Ok(())
}

/// Whether everything in `folder` goes once `removed` are removed, and
/// whether any of them lies in it. Removing a folder's last file removes the
/// folder too, so a subfolder goes only when it holds a removed file and
/// everything else in it goes as well; an empty one stays.
fn folder_vanishes(folder: &Path, removed: &HashSet<PathBuf>) -> Result<(bool, bool), String> {
    let mut everything = true;
    let mut any = false;
    let entries = fs::read_dir(folder)
        .map_err(|error| format!("{} could not be read: {error}", folder.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_dir() {
            let (inside, holds_removed) = folder_vanishes(&entry.path(), removed)?;
            everything &= inside && holds_removed;
            any |= holds_removed;
        } else if removed.contains(&entry.path()) {
            any = true;
        } else {
            everything = false;
        }
    }
    Ok((everything, any))
}

/// Removes the file or symbolic link at `path`, never a directory and never
/// anything reached through a symbolic link.
pub(super) fn remove_worktree_entry(worktree: &Path, path: &GitPath) -> Result<(), String> {
    let valid = WorktreePath::new(path)?;
    if remove_worktree_file(worktree, &valid, path)? {
        remove_empty_parents(worktree, &valid, 0);
    }
    Ok(())
}

/// Removes the file or link at `valid`, reporting whether there was one.
fn remove_worktree_file(
    worktree: &Path,
    valid: &WorktreePath,
    path: &GitPath,
) -> Result<bool, String> {
    let Some(target) = inspect_path(worktree, valid)? else {
        return Ok(false);
    };
    match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.file_type().is_dir() => Err(format!(
            "{} is a directory, so Repola will not remove it.",
            path.display
        )),
        Ok(metadata) => {
            remove_file_or_link(&target, &metadata)
                .map_err(|error| format!("{} could not be removed: {error}", path.display))?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("{} could not be inspected: {error}", path.display)),
    }
}

/// Removes the directories a removed file leaves empty, innermost first and
/// never the worktree root, the way `git clean -d` does. `remove_dir` only
/// removes empty directories, every directory is re-checked to be a real one
/// on the way down, and the walk stops at the first directory that is not
/// empty or cannot be removed, so no content and nothing reached through a
/// link is ever deleted.
fn remove_empty_parents(worktree: &Path, path: &WorktreePath, keep: usize) {
    let parents = &path.components[..path.components.len().saturating_sub(1)];
    for depth in (keep + 1..=parents.len()).rev() {
        let Ok(Some(directory)) = real_directory(worktree, &parents[..depth]) else {
            return;
        };
        if fs::remove_dir(&directory).is_err() {
            return;
        }
    }
}

/// Moves `source` over `target`, which may be a file or a link to either.
fn replace(source: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    if let Ok(metadata) = fs::symlink_metadata(target) {
        // Windows will not rename over a link to a folder; it is saved before
        // a restore replaces it.
        if metadata.file_type().is_symlink() {
            remove_file_or_link(target, &metadata)?;
        }
    }
    fs::rename(source, target)
}

/// Removes a file or a link, whether it links to a file or a folder.
#[cfg(windows)]
fn remove_file_or_link(target: &Path, metadata: &fs::Metadata) -> std::io::Result<()> {
    use std::os::windows::fs::FileTypeExt;
    if metadata.file_type().is_symlink_dir() {
        return fs::remove_dir(target);
    }
    fs::remove_file(target)
}

/// Removes a file or a link; Unix removes either the same way.
#[cfg(not(windows))]
fn remove_file_or_link(target: &Path, _metadata: &fs::Metadata) -> std::io::Result<()> {
    fs::remove_file(target)
}

fn sibling_temporary(target: &Path) -> Result<PathBuf, String> {
    let parent = target
        .parent()
        .ok_or_else(|| "The restore target has no parent directory.".to_string())?;
    Ok(parent.join(format!(".repola-restore-{}", uuid::Uuid::new_v4().simple())))
}

fn replace_with_blob(
    worktree: &Path,
    entry: &WorktreeEntry,
    target: &Path,
    display: &str,
) -> Result<(), String> {
    let temporary = sibling_temporary(target)?;
    let written = write_blob(worktree, entry, &temporary)
        .and_then(|()| replace(&temporary, target).map_err(|error| error.to_string()));
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(format!("{display} could not be restored: {error}"));
    }
    Ok(())
}

fn write_blob(worktree: &Path, entry: &WorktreeEntry, destination: &Path) -> Result<(), String> {
    let file = write_object(worktree, &entry.oid, destination)?;
    set_permissions(&file, entry).map_err(|error| error.to_string())
}

/// Writes the blob `oid` to a new file at `destination`.
fn write_object(worktree: &Path, oid: &str, destination: &Path) -> Result<fs::File, String> {
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| error.to_string())?;
    let sink = file.try_clone().map_err(|error| error.to_string())?;
    // A restore may be writing it, so it is not cut short by the usual limit.
    let output = command::git_at_to_file_timeout(
        worktree,
        HooksOff::new()?.args(["cat-file", "blob", oid]),
        sink,
        CHANGE_TIMEOUT,
    )
    .map_err(|error| error.to_string())?;
    successful_stdout(output, "read the saved file")?;
    file.sync_all().map_err(|error| error.to_string())?;
    Ok(file)
}

#[cfg(unix)]
fn set_permissions(file: &fs::File, entry: &WorktreeEntry) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = file.metadata()?.permissions();
    let mode = match entry.permissions {
        Some(saved) => saved,
        // Grant execute wherever read is granted, as Git does on checkout.
        None if entry.kind == WorktreeEntryKind::Executable => {
            permissions.mode() | ((permissions.mode() & 0o444) >> 2)
        }
        None => return Ok(()),
    };
    permissions.set_mode(mode);
    file.set_permissions(permissions)
}

#[cfg(unix)]
fn set_folder_permissions(folder: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(folder, fs::Permissions::from_mode(mode))
}

#[cfg(windows)]
fn set_folder_permissions(_folder: &Path, _mode: u32) -> std::io::Result<()> {
    // Windows records no permission bits to restore.
    Ok(())
}

#[cfg(windows)]
fn set_permissions(_file: &fs::File, _entry: &WorktreeEntry) -> std::io::Result<()> {
    // Windows has no permission bits to restore.
    Ok(())
}

fn replace_with_symlink(
    target: &Path,
    link: &[u8],
    directory: bool,
    display: &str,
) -> Result<(), String> {
    let temporary = sibling_temporary(target)?;
    let created = create_symlink(link, &temporary, directory)
        .and_then(|()| replace(&temporary, target))
        .map_err(|error| error.to_string());
    if let Err(error) = created {
        let _ = fs::remove_file(&temporary);
        return Err(format!("{display} could not be restored: {error}"));
    }
    Ok(())
}

/// Unix links have no kind, so `_directory` does not matter.
#[cfg(unix)]
fn create_symlink(link: &[u8], path: &Path, _directory: bool) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(link), path)
}

/// Makes the kind of link that was saved, a link to a folder or to a file.
#[cfg(windows)]
fn create_symlink(link: &[u8], path: &Path, directory: bool) -> std::io::Result<()> {
    let link = std::str::from_utf8(link)
        .map_err(|_| std::io::Error::other("the saved link target is not valid Unicode"))?
        .replace('/', "\\");
    if directory {
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
    let output = command::git_at_with_input_timeout(
        worktree,
        HooksOff::new()?.args(["update-index", "-z", flag, "--stdin"]),
        &input,
        CHANGE_TIMEOUT,
    )
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
            write_object(worktree, &entry.oid, &copy)?;
            copy.into_os_string()
        }
    };
    // Written to a file: a patch can be several times the size of its sides.
    let diff = |extra: &str| -> Result<fs::File, String> {
        let mut file = tempfile::tempfile().map_err(|error| error.to_string())?;
        let sink = file.try_clone().map_err(|error| error.to_string())?;
        let output = command::git_at_to_file(
            worktree,
            HooksOff::new()?.args([
                OsString::from("diff"),
                OsString::from("--no-index"),
                OsString::from("--no-color"),
                OsString::from("--no-ext-diff"),
                OsString::from("--no-textconv"),
                OsString::from(extra),
                OsString::from("--"),
                current.clone(),
                saved_side.clone(),
            ]),
            sink,
        )
        .map_err(|error| error.to_string())?;
        if !(output.status.success() || output.status.code() == Some(1)) {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        file.rewind().map_err(|error| error.to_string())?;
        Ok(file)
    };
    let mut numstat = Vec::new();
    diff("--numstat")?
        .read_to_end(&mut numstat)
        .map_err(|error| error.to_string())?;
    let binary = numstat
        .split(|byte| *byte == b'\n')
        .any(|line| line.starts_with(b"-\t-\t"));
    let file = diff("--patch")?;
    if file.metadata().map_err(|error| error.to_string())?.len() > MAX_PREVIEW_PATCH_BYTES as u64 {
        return Ok(too_large);
    }
    let mut patch = Vec::new();
    BufReader::new(file)
        .read_to_end(&mut patch)
        .map_err(|error| error.to_string())?;
    let patch = relabel_patch(&String::from_utf8_lossy(&patch), &state.path.display);
    // Bytes that are not text grow when encoded for the response.
    if serde_json::to_vec(&patch)
        .map_err(|error| error.to_string())?
        .len()
        > MAX_PREVIEW_PATCH_BYTES
    {
        return Ok(too_large);
    }
    Ok(RecoveryFileDiff {
        patch,
        binary,
        truncated: false,
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
pub fn delete_recovery_points(request: DeleteRecoveryPointsRequest) -> Result<(), String> {
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
    let output = command::git_at_with_input(
        worktree,
        HooksOff::new()?.args(["update-ref", "--stdin", "-z"]),
        &input,
    )
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Nothing was deleted: a selected recovery point changed or no longer exists. Refresh and review the selection again.\n\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
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
    let output = command::git_at(worktree, HooksOff::new()?.args(args))
        .map_err(|error| error.to_string())?;
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
    let output = command::git_at_to_file(worktree, HooksOff::new()?.args(args), sink)
        .map_err(|error| error.to_string())?;
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
                permissions: None,
                directory_link: false,
            }),
            directory: false,
            folder_permissions: None,
            parents: 0,
            parent_permissions: Vec::new(),
            blocked: false,
            index_beneath: 0,
            indexed_parent: None,
        }
    }

    /// Saves states whose content the test already stored.
    fn store_recovery_point(
        path: &Path,
        kind: RecoveryPointKind,
        summary: String,
        head: Option<&str>,
        states: &[PathState],
    ) -> Result<RecoveryPoint, String> {
        let prepared = prepare_recovery_point(path, kind, summary, head, states)?;
        create_recovery_point(path, prepared)
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
                    permissions: None,
                    directory_link: false,
                }),
                directory: false,
                folder_permissions: None,
                parents: 0,
                parent_permissions: Vec::new(),
                blocked: false,
                index_beneath: 0,
                indexed_parent: None,
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
                permissions: None,
                directory_link: false,
            }),
            directory: false,
            folder_permissions: None,
            parents: 0,
            parent_permissions: Vec::new(),
            blocked: false,
            index_beneath: 0,
            indexed_parent: None,
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
    fn a_manifest_naming_one_file_under_another_is_refused() {
        let (_directory, path) = repository();
        fs::write(path.join(".env"), b"secret\n").expect("write");
        let saved = hash_bytes(&path, b"replacement\n", true).expect("saved");
        let disguised = |path: GitPath| PathState {
            path,
            index: Vec::new(),
            worktree: Some(WorktreeEntry {
                kind: WorktreeEntryKind::File,
                oid: saved.clone(),
                size: 12,
                permissions: None,
                directory_link: false,
            }),
            directory: false,
            folder_permissions: None,
            parents: 0,
            parent_permissions: Vec::new(),
            blocked: false,
            index_beneath: 0,
            indexed_parent: None,
        };
        // A harmless name over `.env`'s token, and `.env`'s token spelled in
        // capitals.
        for (index, crafted) in [
            GitPath {
                display: "harmless.txt".into(),
                token: git_path(b".env").token,
            },
            GitPath {
                display: ".env".into(),
                token: git_path(b".env").token.to_uppercase(),
            },
        ]
        .into_iter()
        .enumerate()
        {
            let point = craft(
                &path,
                &format!("disguised-{index}"),
                &manifest_for(&[disguised(crafted)]),
                &[("100644", saved.clone(), b"worktree/.env".to_vec())],
            );
            let error = plan(&path, &point).expect_err("disguised");
            assert!(
                error.contains("damaged") || error.contains("malformed"),
                "{error}"
            );
        }
        assert_eq!(fs::read(path.join(".env")).expect("read"), b"secret\n");
    }

    #[test]
    fn a_manifest_understating_a_saved_file_is_refused() {
        let (_directory, path) = repository();
        let large = hash_bytes(&path, &vec![b'x'; 64 * 1024], true).expect("large");
        let state = PathState {
            path: git_path(b"a.txt"),
            index: Vec::new(),
            worktree: Some(WorktreeEntry {
                kind: WorktreeEntryKind::File,
                oid: large.clone(),
                size: 6,
                permissions: None,
                directory_link: false,
            }),
            directory: false,
            folder_permissions: None,
            parents: 0,
            parent_permissions: Vec::new(),
            blocked: false,
            index_beneath: 0,
            indexed_parent: None,
        };
        let point = craft(
            &path,
            "understated",
            &manifest_for(&[state]),
            &[("100644", large, b"worktree/a.txt".to_vec())],
        );
        let error = plan(&path, &point).expect_err("damaged");
        assert!(error.contains("damaged"), "{error}");
    }

    #[test]
    fn a_listed_sample_is_named_as_its_token_spells_it() {
        let summary = Summary {
            version: MANIFEST_VERSION,
            kind: RecoveryPointKind::DiscardFile,
            summary: "Crafted".into(),
            created_at: "2026-01-01T00:00:00.000Z".into(),
            worktree_path: "/repo".into(),
            head: None,
            path_count: 1,
            paths: vec![GitPath {
                display: "harmless.txt".into(),
                token: git_path(b".env").token,
            }],
            stored_bytes: 0,
        };
        let point = point_from_summary("id".into(), "oid".into(), summary);
        assert_eq!(point.paths, [git_path(b".env")]);
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
        // Refused when reviewed, before anything is written.
        let error = plan(&path, &point).expect_err("parent is a file");
        assert!(
            error.contains("a file in one place and a folder"),
            "{error}"
        );
        let error = restore(&path, &point).expect_err("parent is a file");
        assert!(
            error.contains("a file in one place and a folder"),
            "{error}"
        );
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
            assert!(
                error.contains("a file in one place and a folder"),
                "{error}"
            );
            assert_eq!(fs::read_dir(outside.path()).expect("outside").count(), 0);
        }
        drop(directory);
    }

    // Creating links needs privileges Windows runners do not grant.
    #[cfg(unix)]
    #[test]
    fn saved_links_come_back_as_links_whatever_core_symlinks_says() {
        let (_directory, path) = repository();
        // Only a real link on disk is saved as one, so `core.symlinks`, which
        // decides how Git checks out tracked links, does not apply.
        for setting in ["false", "true"] {
            git(&path, &["config", "core.symlinks", setting]);
            let link = saved_file(&path, "alias", WorktreeEntryKind::Symlink, b"base.txt");
            restore(&path, &reference(&store(&path, &[link]))).expect("restore");
            assert_eq!(
                fs::read_link(path.join("alias")).expect("link"),
                Path::new("base.txt"),
                "core.symlinks={setting}"
            );
            fs::remove_file(path.join("alias")).expect("remove");
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
        assert_eq!(
            list_recovery_points(request(&path))
                .expect("list")
                .points
                .len(),
            2
        );
        assert!(delete(vec![reference(&first), reference(&first)]).is_err());
        delete(vec![reference(&first), reference(&second)]).expect("delete");
        assert!(list_recovery_points(request(&path))
            .expect("list")
            .points
            .is_empty());
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
        let listed = list_recovery_points(request(&path)).expect("list").points;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path_count, SUMMARY_SAMPLE_PATHS as u64);
    }

    #[test]
    fn a_summary_too_large_to_list_is_never_stored() {
        let (_directory, path) = repository();
        store_recovery_point(
            &path,
            RecoveryPointKind::DiscardFile,
            "x".repeat(MAX_SUMMARY_BYTES),
            None,
            &[saved_file(&path, "a.txt", WorktreeEntryKind::File, b"a\n")],
        )
        .expect_err("oversized summary");
        assert!(git(&path, &["for-each-ref", RECOVERY_REF_NAMESPACE]).is_empty());
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
        let points = list_recovery_points(request(&path)).expect("list").points;
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
            .points
            .into_iter()
            .map(|point| point.id)
            .collect();
        listed.sort();
        copies.push(first.id);
        copies.sort();
        assert_eq!(listed, copies);
    }

    #[test]
    fn listing_sends_the_newest_points_one_response_carries_and_counts_the_rest() {
        let (_directory, path) = repository();
        // Summaries near their limit, so a few hundred points fill a response.
        let large = store_recovery_point(
            &path,
            RecoveryPointKind::DiscardFile,
            "x".repeat(MAX_SUMMARY_BYTES - 1024),
            None,
            &[saved_file(&path, "a.txt", WorktreeEntryKind::File, b"a\n")],
        )
        .expect("store");
        let copies = 2 * MAX_LISTED_BYTES / MAX_SUMMARY_BYTES;
        let updates: String = (0..copies)
            .map(|index| {
                format!(
                    "create {RECOVERY_REF_NAMESPACE}/point-{index:04} {}\n",
                    large.oid
                )
            })
            .collect();
        let output =
            command::git_at_with_input(&path, ["update-ref", "--stdin"], updates.as_bytes())
                .expect("update-ref");
        assert!(output.status.success());

        let list = list_recovery_points(request(&path)).expect("list");
        assert!(list.omitted > 0);
        assert_eq!(list.points.len() as u64 + list.omitted, copies as u64 + 1);
        let sent: usize = list
            .points
            .iter()
            .map(|point| serde_json::to_vec(point).expect("serialize").len())
            .sum();
        assert!(sent <= MAX_LISTED_BYTES);
        // Every copy has the same creation time, so newest first falls back to
        // the reference name, descending.
        let mut newest: Vec<String> = (0..copies)
            .map(|index| format!("{RECOVERY_REF_NAMESPACE}/point-{index:04}"))
            .chain([large.id])
            .collect();
        newest.sort_by(|left, right| right.cmp(left));
        newest.truncate(list.points.len());
        let listed: Vec<String> = list.points.into_iter().map(|point| point.id).collect();
        assert_eq!(listed, newest);
    }

    #[test]
    fn a_path_too_long_for_one_command_line_is_refused() {
        let (_directory, path) = repository();
        let fits = "d/".repeat(ARGUMENT_BUDGET_BYTES / 2 - 1) + "f";
        assert_eq!(fits.len(), ARGUMENT_BUDGET_BYTES - 1);
        observe_paths(&path, None, &[git_path(fits.as_bytes())], false).expect("observe");
        let long = fits + "/x";
        let error =
            observe_paths(&path, None, &[git_path(long.as_bytes())], false).expect_err("too long");
        assert!(error.contains("too long a path"), "{error}");
    }

    #[test]
    fn content_is_saved_only_as_it_was_observed() {
        let (_directory, path) = repository();
        fs::write(path.join("a.txt"), b"observed\n").expect("write");
        let observed = observe_paths(&path, None, &[git_path(b"a.txt")], false).expect("observe");
        fs::write(path.join("a.txt"), b"edited after\n").expect("edit");
        assert_eq!(
            store_contents(&path, &observed).expect_err("changed"),
            CHANGED_WHILE_SAVING
        );

        // Stored content must also have the size observed on disk.
        fs::write(path.join("a.txt"), b"observed\n").expect("write");
        let mut wrong = observed.clone();
        if let Some(entry) = wrong[0].worktree.as_mut() {
            entry.size += 1;
        }
        let error = store_contents(&path, &wrong).expect_err("size");
        assert!(error.contains("intact"), "{error}");
        store_contents(&path, &observed).expect("store");

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("observed", path.join("link")).expect("link");
            let observed =
                observe_paths(&path, None, &[git_path(b"link")], false).expect("observe");
            fs::remove_file(path.join("link")).expect("remove");
            std::os::unix::fs::symlink("retargeted", path.join("link")).expect("link");
            assert_eq!(
                store_contents(&path, &observed).expect_err("changed"),
                CHANGED_WHILE_SAVING
            );
        }
    }

    #[test]
    fn a_preview_too_large_once_encoded_is_left_out() {
        let (_directory, path) = repository();
        // Bytes that are not text each become a three-byte character.
        let bytes = [&[0xff_u8; 63][..], b"\n"].concat().repeat(48 * 1024);
        let state = saved_file(&path, "noise.txt", WorktreeEntryKind::File, &bytes);
        let point = reference(&store(&path, &[state]));
        let diff = recovery_file_diff(RecoveryFileDiffRequest {
            repository_path: request(&path).repository_path,
            worktree_path: request(&path).worktree_path,
            point,
            path: git_path(b"noise.txt"),
        })
        .expect("preview");
        assert!(diff.truncated);
        assert!(diff.patch.is_empty());
    }

    #[test]
    fn lists_keep_what_one_budget_carries_and_count_the_rest() {
        // Each string encodes to its length plus two quotes.
        let mut budget = 25;
        let mut first = vec!["aaaaaaaa".to_string(); 3];
        assert_eq!(keep_within(&mut first, &mut budget).expect("first"), 1);
        assert_eq!(first.len(), 2);
        assert_eq!(budget, 5);
        let mut second = vec!["bbb".to_string(), "c".to_string()];
        assert_eq!(keep_within(&mut second, &mut budget).expect("second"), 1);
        assert_eq!(second, ["bbb"]);
    }

    #[test]
    fn two_spellings_of_one_file_on_disk_are_refused() {
        let (_directory, path) = repository();
        // Names that differ only in case may be refused by name alone first.
        let refused_as_one_name = |error: &str| {
            error.contains("spelled differently") || error.contains("differ only in letter case")
        };
        // Even where Git is told names are case-sensitive, the file system
        // decides which spellings are one file.
        git(&path, &["config", "core.ignorecase", "false"]);
        fs::write(path.join("caf\u{e9}.txt"), b"one file\n").expect("write");
        for (stored, other) in [
            ("caf\u{e9}.txt", "CAF\u{c9}.TXT"),
            ("caf\u{e9}.txt", "cafe\u{301}.txt"),
        ] {
            if !path.join(other).exists() {
                // This file system tells the spellings apart.
                continue;
            }
            let paths = [git_path(stored.as_bytes()), git_path(other.as_bytes())];
            let error = observe_paths(&path, None, &paths, false).expect_err("one file");
            assert!(refused_as_one_name(&error), "{error}");
        }
        // Hard links are separate names on purpose and stay allowed, and a
        // file having one does not hide another spelling of its own name.
        fs::hard_link(path.join("caf\u{e9}.txt"), path.join("linked.txt")).expect("link");
        let paths = [
            git_path("caf\u{e9}.txt".as_bytes()),
            git_path(b"linked.txt"),
        ];
        observe_paths(&path, None, &paths, false).expect("hard links");
        for other in ["CAF\u{c9}.TXT", "cafe\u{301}.txt"] {
            if path.join(other).exists() {
                let paths = [
                    git_path("caf\u{e9}.txt".as_bytes()),
                    git_path(other.as_bytes()),
                ];
                let error = observe_paths(&path, None, &paths, false).expect_err("one file");
                assert!(refused_as_one_name(&error), "{error}");
            }
        }
        // A link is found without following it, even with a hard link of its
        // own.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("target", path.join("caf\u{e9}-link")).expect("symlink");
            fs::hard_link(path.join("caf\u{e9}-link"), path.join("other-link")).expect("link");
            assert!(fs::symlink_metadata(path.join("other-link"))
                .expect("hard link")
                .file_type()
                .is_symlink());
            for other in ["CAF\u{c9}-LINK", "cafe\u{301}-link"] {
                if fs::symlink_metadata(path.join(other)).is_ok() {
                    let paths = [
                        git_path("caf\u{e9}-link".as_bytes()),
                        git_path(other.as_bytes()),
                    ];
                    let error = observe_paths(&path, None, &paths, false).expect_err("one link");
                    assert!(refused_as_one_name(&error), "{error}");
                }
            }
            let paths = [
                git_path("caf\u{e9}-link".as_bytes()),
                git_path(b"other-link"),
            ];
            observe_paths(&path, None, &paths, false).expect("hard-linked links");
        }
    }

    #[test]
    fn paths_differing_only_in_case_are_refused_where_the_file_system_folds_case() {
        let (_directory, path) = repository();
        // A setting copied from a case-sensitive system says nothing about
        // this file system.
        git(&path, &["config", "core.ignorecase", "false"]);
        // Neither spelling is on disk, so only their names can tell.
        let paths = [git_path(b"Gone.txt"), git_path(b"gone.txt")];
        let observed = observe_paths(&path, None, &paths, false);
        if path.join(".GIT").exists() {
            let error = observed.expect_err("one name on this file system");
            assert!(error.contains("differ only in letter case"), "{error}");
        } else {
            observed.expect("two names on this file system");
        }
    }

    #[test]
    fn names_one_file_system_takes_as_one_share_a_case_form() {
        let one = |left: &str, right: &str| {
            let (left, right) = (case_forms(left), case_forms(right));
            left[0] == right[0] || left[1] == right[1]
        };
        for (left, right) in [
            // One name on APFS and in Linux's case-insensitive folders.
            ("\u{3c3}", "\u{3c2}"),
            ("\u{df}", "ss"),
            ("\u{1e9e}", "\u{df}"),
            ("\u{17f}", "s"),
            ("\u{fb00}", "ff"),
            ("\u{1f80}", "\u{1f88}"),
            ("\u{10400}", "\u{10428}"),
            ("\u{130}", "i\u{307}"),
            ("\u{e9}", "E\u{301}"),
            // One name where each character is uppercased, as on NTFS.
            ("\u{131}", "i"),
            ("\u{cd}", "\u{131}\u{301}"),
            ("\u{3c2}", "\u{3a3}"),
            // One name on HFS+.
            ("a\u{200c}b", "ab"),
            ("\u{feff}x", "X"),
        ] {
            assert!(one(left, right), "{left} and {right}");
        }
        for (left, right) in [("a", "b"), ("\u{df}", "s"), ("\u{130}", "i"), ("1", "2")] {
            assert!(!one(left, right), "{left} and {right}");
        }
    }

    #[test]
    fn names_any_file_system_ignoring_case_takes_as_one_are_refused() {
        let (_directory, path) = repository();
        git(&path, &["config", "core.ignorecase", "true"]);
        // Neither spelling is on disk, so only their names can tell.
        for (left, right) in [("\u{3c3}.txt", "\u{3c2}.txt"), ("stra\u{df}e", "STRASSE")] {
            let paths = [git_path(left.as_bytes()), git_path(right.as_bytes())];
            let error = observe_paths(&path, None, &paths, false).expect_err("one name");
            assert!(error.contains("may take them as one name"), "{error}");
        }
    }

    #[test]
    fn a_spelling_the_file_system_finds_is_the_entry_it_stores() {
        let (_directory, path) = repository();
        fs::write(path.join("\u{3c3}.txt"), b"one file\n").expect("write");
        let other = path.join("\u{3c2}.txt");
        let Ok(metadata) = fs::symlink_metadata(&other) else {
            // This file system tells the spellings apart.
            return;
        };
        let mut listings = HashMap::new();
        let stored = entry_identity(
            &path.join("\u{3c3}.txt"),
            &fs::symlink_metadata(path.join("\u{3c3}.txt")).expect("stored"),
            &mut listings,
        )
        .expect("stored identity");
        let found = entry_identity(&other, &metadata, &mut listings).expect("identity");
        assert!(found.iter().any(|identity| stored.contains(identity)));
    }

    #[test]
    fn a_name_its_folder_does_not_store_is_refused() {
        let (_directory, path) = repository();
        fs::create_dir(path.join("longfoldername")).expect("folder");
        fs::write(path.join("longfoldername/file.txt"), b"one\n").expect("write");
        fs::write(path.join("longfilename.txt"), b"two\n").expect("write");
        let listing = FolderListing::read(&path).expect("listing");
        assert!(listing.stores(OsStr::new("longfilename.txt")));
        assert!(listing.stores(OsStr::new("LONGFILENAME.TXT")));
        assert!(!listing.stores(OsStr::new("LONGFI~1.TXT")));
        // Where the file system finds entries under the short names Windows
        // makes up, a path through one is refused.
        for alias in ["LONGFO~1/file.txt", "LONGFI~1.TXT"] {
            if path.join(alias).exists() {
                let error = observe_paths(&path, None, &[git_path(alias.as_bytes())], false)
                    .expect_err("an alias");
                assert!(error.contains("does not store"), "{error}");
            }
        }
        let stored = [
            git_path(b"longfoldername/file.txt"),
            git_path(b"longfilename.txt"),
        ];
        observe_paths(&path, None, &stored, false).expect("stored names");
    }

    // Only macOS file systems compare names this way.
    #[cfg(target_os = "macos")]
    #[test]
    fn composed_and_decomposed_names_are_one_name_even_when_neither_is_on_disk() {
        let (_directory, path) = repository();
        let paths = [
            git_path("caf\u{e9}.txt".as_bytes()),
            git_path("cafe\u{301}.txt".as_bytes()),
        ];
        let error = observe_paths(&path, None, &paths, false).expect_err("one name");
        assert!(error.contains("spelled differently"), "{error}");
    }

    #[test]
    fn names_and_paths_too_long_to_write_are_recognized() {
        // 255 two-byte characters are 255 UTF-16 units but 510 bytes.
        let accented = "\u{e9}".repeat(255);
        assert!(fits_in_one_name("a".repeat(255).as_bytes()));
        assert!(!fits_in_one_name("a".repeat(256).as_bytes()));
        assert_eq!(
            fits_in_one_name(accented.as_bytes()),
            cfg!(any(windows, target_os = "macos"))
        );
        let worktree = Path::new("C:\\repo");
        let fits = "a".repeat(259 - 8);
        assert_eq!(windows_path_units(worktree, fits.as_bytes()), 259);
        assert!(git_can_write(worktree, fits.as_bytes(), 0, false));
        let long = format!("{fits}b");
        assert!(!git_can_reach(worktree, long.as_bytes(), false));
        assert!(!git_can_write(worktree, long.as_bytes(), 0, false));
        assert!(git_can_write(worktree, long.as_bytes(), 0, true));
        // Folders Git has to create stay under 248 units.
        let nested = format!("{}/{}", "a".repeat(247 - 8), "b".repeat(10));
        assert_eq!(windows_path_units(worktree, &nested.as_bytes()[..239]), 247);
        assert!(git_can_write(worktree, nested.as_bytes(), 0, false));
        let deeper = format!("{}/{}", "a".repeat(248 - 8), "b".repeat(10));
        assert!(git_can_reach(worktree, deeper.as_bytes(), false));
        assert!(!git_can_write(worktree, deeper.as_bytes(), 0, false));
        assert!(git_can_write(worktree, deeper.as_bytes(), 1, false));
        assert!(git_can_write(worktree, deeper.as_bytes(), 0, true));
    }

    #[test]
    fn names_windows_cannot_store_as_themselves_are_recognized() {
        for name in [
            "file.",
            "file ",
            "nul",
            "NUL.txt",
            "con .log",
            "Aux",
            "com1",
            "LPT9.dat",
            "com\u{b9}",
            "conin$",
            "a<b",
            "what?",
            "star*",
            "tab\tname",
        ] {
            assert!(invalid_on_windows(name.as_bytes()), "{name}");
        }
        for name in [
            "file",
            "nullable",
            "console.txt",
            "com10",
            "lpt",
            "auxiliary",
            "a.b",
        ] {
            assert!(!invalid_on_windows(name.as_bytes()), "{name}");
        }
    }

    #[test]
    fn git_directory_names_are_recognized_in_every_spelling() {
        for name in [
            ".git",
            ".GIT",
            ".Git. ",
            ".git...",
            "git~1",
            "GIT~1",
            ".g\u{200c}it",
            "\u{feff}.GIT",
        ] {
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
