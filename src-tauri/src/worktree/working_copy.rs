use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use base64::prelude::{Engine as _, BASE64_STANDARD};

use super::command;
use super::models::{
    ApplyPatchHunkRequest, CommitFileSelection, CommitHunkSelection, CommitPerson, CommitRequest,
    CommitResult, CommitSigning, CommitTrailer, ConflictFile, ConflictFileRequest,
    ConflictResolutionKind, DiscardAllRequest, DiscardFileRequest, DiscardScope, FileChange,
    FileChangeKind, FileDiff, FileDiffRequest, FileModeChange, GitPath, ImagePreview, PatchHunk,
    PatchHunkAction, RepositoryOperation, ResolveConflictRequest, ReviewedFileChange,
    SetFileStagingRequest, UndoCommitRequest, UndoCommitResult, WorkingCopyRequest,
    WorkingCopySnapshot,
};

const MAX_COMMIT_SUMMARY_BYTES: usize = 998;
const MAX_COMMIT_DESCRIPTION_BYTES: usize = 1024 * 1024;
const MAX_FILE_PATCH_BYTES: usize = 8 * 1024 * 1024;
const MAX_IMAGE_PREVIEW_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CONFLICT_FILE_BYTES: usize = 2 * 1024 * 1024;

pub fn working_copy_snapshot(request: WorkingCopyRequest) -> Result<WorkingCopySnapshot, String> {
    let repository = canonical_directory(&request.repository_path, "repository")?;
    let worktree = canonical_directory(&request.worktree_path, "worktree")?;
    let common_dir = git_text(
        &worktree,
        ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let expected_common = git_text(
        &repository,
        ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    if Path::new(&common_dir) != Path::new(&expected_common) {
        return Err("The selected worktree does not belong to the selected repository.".into());
    }

    // Ignored files are intentionally not requested; the parser still accepts
    // `!` records so the `ignored` guards below stay correct if they ever appear.
    let output = command::git_at(
        &worktree,
        [
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ],
    )
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let mut snapshot = parse_status(&output.stdout)?;
    snapshot.repository_path = repository.to_string_lossy().into_owned();
    snapshot.worktree_path = worktree.to_string_lossy().into_owned();
    snapshot.remote = resolve_remote(&worktree, snapshot.branch.as_deref());
    snapshot.upstream_head = snapshot.upstream.as_deref().and_then(|upstream| {
        command::git_at(
            &worktree,
            [
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                upstream,
            ],
        )
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
    });
    snapshot.operation = detect_operation(&worktree);
    Ok(snapshot)
}

pub fn set_file_staging(request: SetFileStagingRequest) -> Result<WorkingCopySnapshot, String> {
    if request.paths.is_empty() {
        return Err("Select at least one changed path.".into());
    }
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path.clone(),
        worktree_path: request.worktree_path.clone(),
    })?;
    let current: BTreeMap<&str, &FileChange> = snapshot
        .changes
        .iter()
        .map(|change| (change.path.token.as_str(), change))
        .collect();
    let mut tokens = BTreeSet::new();
    for requested in &request.paths {
        let change = current.get(requested.token.as_str()).ok_or_else(|| {
            format!(
                "{} changed after it was selected. Refresh and try again.",
                requested.display
            )
        })?;
        if change.ignored {
            return Err(format!(
                "{} is ignored and cannot be staged implicitly.",
                change.path.display
            ));
        }
        tokens.insert(change.path.token.clone());
        if let Some(previous) = &change.previous_path {
            tokens.insert(previous.token.clone());
        }
    }

    let worktree = PathBuf::from(&snapshot.worktree_path);
    let mut args = Vec::<OsString>::new();
    if request.staged {
        args.extend([OsString::from("add"), OsString::from("--")]);
    } else if snapshot.head.is_some() {
        args.extend([
            OsString::from("restore"),
            OsString::from("--staged"),
            OsString::from("--"),
        ]);
    } else {
        args.extend([
            OsString::from("rm"),
            OsString::from("--cached"),
            OsString::from("-r"),
            OsString::from("--"),
        ]);
    }
    for token in tokens {
        args.push(path_from_token(&token)?);
    }
    let output = command::git_at(&worktree, &args).map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    working_copy_snapshot(WorkingCopyRequest {
        repository_path: snapshot.repository_path,
        worktree_path: snapshot.worktree_path,
    })
}

pub fn file_diff(request: FileDiffRequest) -> Result<FileDiff, String> {
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path,
        worktree_path: request.worktree_path,
    })?;
    let change = snapshot
        .changes
        .iter()
        .find(|change| change.path.token == request.path.token)
        .ok_or_else(|| {
            format!(
                "{} is no longer present in the working-copy changes.",
                request.path.display
            )
        })?;
    if change.ignored {
        return Err("Ignored paths are not opened implicitly.".into());
    }

    let worktree = PathBuf::from(&snapshot.worktree_path);
    let paths = diff_paths(change)?;
    let (patch_output, numstat_output) = if change.untracked {
        let null_device = null_device();
        let mut patch_args = vec![
            OsString::from("diff"),
            OsString::from("--no-index"),
            OsString::from("--no-color"),
            OsString::from("--no-ext-diff"),
            OsString::from("--"),
            null_device.clone(),
        ];
        patch_args.extend(paths.iter().cloned());
        let mut numstat_args = vec![
            OsString::from("diff"),
            OsString::from("--no-index"),
            OsString::from("--numstat"),
            OsString::from("--"),
            null_device,
        ];
        numstat_args.extend(paths.iter().cloned());
        (
            git_diff_output(&worktree, &patch_args, true)?,
            git_diff_output(&worktree, &numstat_args, true)?,
        )
    } else {
        let base = match snapshot.head.as_deref() {
            Some(head) => head.to_string(),
            None => empty_tree(&worktree)?,
        };
        let mut patch_args = vec![
            OsString::from("diff"),
            OsString::from("--no-color"),
            OsString::from("--no-ext-diff"),
            OsString::from("--find-renames"),
            OsString::from(&base),
            OsString::from("--"),
        ];
        patch_args.extend(paths.iter().cloned());
        let mut numstat_args = vec![
            OsString::from("diff"),
            OsString::from("--numstat"),
            OsString::from(&base),
            OsString::from("--"),
        ];
        numstat_args.extend(paths.iter().cloned());
        (
            git_diff_output(&worktree, &patch_args, false)?,
            git_diff_output(&worktree, &numstat_args, false)?,
        )
    };

    let binary = numstat_output
        .split(|byte| *byte == b'\n')
        .any(|line| line.starts_with(b"-\t-\t"));
    let submodule = change.submodule
        || patch_output
            .windows(b"Subproject commit ".len())
            .any(|window| window == b"Subproject commit ");
    let image = if binary && !submodule {
        worktree_image_preview(&worktree, change)?
    } else {
        None
    };
    let (patch, truncated) = truncate_file_patch(&patch_output);
    let hunks = if binary || truncated || change.conflicted {
        Vec::new()
    } else {
        split_patch_hunks(&patch_output)
    };
    let (staged_hunks, unstaged_hunks) = if binary || truncated || change.conflicted {
        (Vec::new(), Vec::new())
    } else {
        let (staged, unstaged) = partial_patch_outputs(
            &worktree,
            change,
            &paths,
            &patch_output,
            snapshot.head.as_deref(),
        )?;
        (split_patch_hunks(&staged), split_patch_hunks(&unstaged))
    };
    Ok(FileDiff {
        patch,
        truncated,
        binary,
        submodule,
        image,
        hunks,
        staged_hunks,
        unstaged_hunks,
    })
}

fn worktree_image_preview(
    worktree: &Path,
    change: &FileChange,
) -> Result<Option<ImagePreview>, String> {
    if change.kind == FileChangeKind::Deleted || change.path.token.is_empty() {
        return Ok(None);
    }
    let relative = PathBuf::from(path_from_token(&change.path.token)?);
    let candidate = worktree.join(relative);
    let metadata = match fs::symlink_metadata(&candidate) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("The image preview could not be inspected: {error}")),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_IMAGE_PREVIEW_BYTES
    {
        return Ok(None);
    }
    let canonical = dunce::canonicalize(&candidate)
        .map_err(|error| format!("The image preview path could not be resolved: {error}"))?;
    if !canonical.starts_with(worktree) {
        return Err("The image preview resolved outside the selected working copy.".into());
    }
    let bytes = fs::read(&canonical)
        .map_err(|error| format!("The image preview could not be read: {error}"))?;
    image_preview(&bytes, "Current working copy")
}

pub(super) fn image_preview(bytes: &[u8], label: &str) -> Result<Option<ImagePreview>, String> {
    if bytes.len() as u64 > MAX_IMAGE_PREVIEW_BYTES {
        return Ok(None);
    }
    let mime_type = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else if bytes.starts_with(b"BM") {
        "image/bmp"
    } else {
        return Ok(None);
    };
    Ok(Some(ImagePreview {
        mime_type: mime_type.into(),
        base64: BASE64_STANDARD.encode(bytes),
        byte_length: bytes.len() as u64,
        label: label.into(),
    }))
}

pub fn apply_patch_hunk(request: ApplyPatchHunkRequest) -> Result<WorkingCopySnapshot, String> {
    if request.expected_patch.len() > MAX_FILE_PATCH_BYTES {
        return Err("The selected patch hunk is too large to apply safely.".into());
    }
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path.clone(),
        worktree_path: request.worktree_path.clone(),
    })?;
    if snapshot.head != request.expected_head {
        return Err(
            "The working-copy HEAD changed after this hunk was selected. Refresh and try again."
                .into(),
        );
    }
    let change = snapshot
        .changes
        .iter()
        .find(|change| change.path.token == request.path.token)
        .ok_or_else(|| {
            "The selected changed path no longer exists. Refresh and try again.".to_string()
        })?;
    if change.conflicted
        || change.ignored
        || matches!(
            change.kind,
            FileChangeKind::Renamed | FileChangeKind::Copied
        )
    {
        return Err("This path does not support partial staging in its current state.".into());
    }
    let diff = file_diff(FileDiffRequest {
        repository_path: snapshot.repository_path.clone(),
        worktree_path: snapshot.worktree_path.clone(),
        path: request.path,
    })?;
    if diff.binary || diff.truncated {
        return Err("Binary or truncated diffs cannot be staged by hunk.".into());
    }
    let candidates = match request.action {
        PatchHunkAction::Stage => &diff.unstaged_hunks,
        PatchHunkAction::Unstage => &diff.staged_hunks,
        PatchHunkAction::Discard => &diff.unstaged_hunks,
    };
    let hunk = candidates
        .iter()
        .find(|hunk| hunk.index == request.hunk_index)
        .ok_or_else(|| "The selected hunk no longer exists. Refresh and try again.".to_string())?;
    if hunk.patch != request.expected_patch {
        return Err(
            "The selected hunk changed after it was reviewed. Refresh and try again.".into(),
        );
    }
    let patch = if request.selected_line_indices.is_empty() {
        hunk.patch.clone()
    } else {
        partial_hunk_patch(&hunk.patch, &request.selected_line_indices)?
    };
    let mut args = vec!["apply"];
    if request.action != PatchHunkAction::Discard {
        args.push("--cached");
    }
    args.push("--whitespace=nowarn");
    if request.action != PatchHunkAction::Stage {
        args.push("--reverse");
    }
    let output =
        command::git_at_with_input(Path::new(&snapshot.worktree_path), args, patch.as_bytes())
            .map_err(|error| error.to_string())?;
    ensure_success(output, "apply the selected patch hunk")?;
    working_copy_snapshot(WorkingCopyRequest {
        repository_path: snapshot.repository_path,
        worktree_path: snapshot.worktree_path,
    })
}

fn partial_hunk_patch(patch: &str, selected_line_indices: &[u32]) -> Result<String, String> {
    let header_offset = patch
        .find("@@ ")
        .ok_or_else(|| "The reviewed patch has no hunk header.".to_string())?;
    let hunk = &patch[header_offset..];
    let header_end = hunk
        .find('\n')
        .ok_or_else(|| "The reviewed hunk header is truncated.".to_string())?;
    let header = &hunk[..header_end];
    let (old_start, new_start, suffix) = parse_hunk_header(header)?;
    let selected: BTreeSet<u32> = selected_line_indices.iter().copied().collect();
    if selected.len() != selected_line_indices.len() {
        return Err("The selected patch lines contain duplicates.".into());
    }
    let mut body = String::new();
    let mut old_count = 0_u32;
    let mut new_count = 0_u32;
    let mut selected_changes = 0_usize;
    for (index, line) in hunk[header_end + 1..].split_inclusive('\n').enumerate() {
        let index =
            u32::try_from(index).map_err(|_| "The patch has too many lines.".to_string())?;
        match line.as_bytes().first().copied() {
            Some(b' ') => {
                body.push_str(line);
                old_count += 1;
                new_count += 1;
            }
            Some(b'+') if selected.contains(&index) => {
                body.push_str(line);
                new_count += 1;
                selected_changes += 1;
            }
            Some(b'+') => {}
            Some(b'-') if selected.contains(&index) => {
                body.push_str(line);
                old_count += 1;
                selected_changes += 1;
            }
            Some(b'-') => {
                body.push(' ');
                body.push_str(&line[1..]);
                old_count += 1;
                new_count += 1;
            }
            Some(b'\\') => {
                return Err("Lines next to a no-newline marker must be staged or discarded as a complete hunk.".into());
            }
            _ => return Err("The reviewed patch contains an unsupported line.".into()),
        }
    }
    if selected_changes == 0 {
        return Err("Select at least one added or deleted line.".into());
    }
    if selected.iter().any(|index| {
        hunk[header_end + 1..]
            .split_inclusive('\n')
            .nth(*index as usize)
            .and_then(|line| line.as_bytes().first())
            .is_none_or(|prefix| !matches!(prefix, b'+' | b'-'))
    }) {
        return Err("Only added or deleted lines can be selected.".into());
    }
    Ok(format!(
        "{}@@ -{},{} +{},{} @@{}\n{}",
        &patch[..header_offset],
        old_start,
        old_count,
        new_start,
        new_count,
        suffix,
        body
    ))
}

fn parse_hunk_header(header: &str) -> Result<(u32, u32, &str), String> {
    let rest = header
        .strip_prefix("@@ -")
        .ok_or_else(|| "The reviewed hunk header is invalid.".to_string())?;
    let (old_range, rest) = rest
        .split_once(" +")
        .ok_or_else(|| "The reviewed hunk header has no new range.".to_string())?;
    let (new_range, suffix) = rest
        .split_once(" @@")
        .ok_or_else(|| "The reviewed hunk header is incomplete.".to_string())?;
    Ok((range_start(old_range)?, range_start(new_range)?, suffix))
}

fn range_start(range: &str) -> Result<u32, String> {
    range
        .split(',')
        .next()
        .unwrap_or(range)
        .parse()
        .map_err(|_| "The reviewed hunk range is invalid.".into())
}

fn partial_patch_outputs(
    worktree: &Path,
    change: &FileChange,
    paths: &[OsString],
    full_patch: &[u8],
    head: Option<&str>,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    if change.untracked {
        return Ok((Vec::new(), full_patch.to_vec()));
    }
    let base = match head {
        Some(head) => head.to_string(),
        None => empty_tree(worktree)?,
    };
    let mut staged_args = vec![
        OsString::from("diff"),
        OsString::from("--cached"),
        OsString::from("--no-color"),
        OsString::from("--no-ext-diff"),
        OsString::from(base),
        OsString::from("--"),
    ];
    staged_args.extend(paths.iter().cloned());
    let mut unstaged_args = vec![
        OsString::from("diff"),
        OsString::from("--no-color"),
        OsString::from("--no-ext-diff"),
        OsString::from("--"),
    ];
    unstaged_args.extend(paths.iter().cloned());
    Ok((
        git_diff_output(worktree, &staged_args, false)?,
        git_diff_output(worktree, &unstaged_args, false)?,
    ))
}

fn split_patch_hunks(bytes: &[u8]) -> Vec<PatchHunk> {
    if bytes.len() > MAX_FILE_PATCH_BYTES {
        return Vec::new();
    }
    let Ok(patch) = String::from_utf8(bytes.to_vec()) else {
        return Vec::new();
    };
    let mut starts = Vec::new();
    let mut offset = 0;
    for line in patch.split_inclusive('\n') {
        if line.starts_with("@@ ") {
            starts.push(offset);
        }
        offset += line.len();
    }
    let Some(first) = starts.first().copied() else {
        return Vec::new();
    };
    let file_header = &patch[..first];
    starts
        .iter()
        .enumerate()
        .map(|(index, start)| {
            let end = starts.get(index + 1).copied().unwrap_or(patch.len());
            let body = &patch[*start..end];
            PatchHunk {
                index: index as u32,
                header: body.lines().next().unwrap_or("@@").to_string(),
                patch: format!("{file_header}{body}"),
            }
        })
        .collect()
}

pub fn conflict_file(request: ConflictFileRequest) -> Result<ConflictFile, String> {
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path,
        worktree_path: request.worktree_path,
    })?;
    let change = snapshot
        .changes
        .iter()
        .find(|change| change.path.token == request.path.token)
        .ok_or_else(|| {
            "The selected conflict no longer exists. Refresh and try again.".to_string()
        })?;
    if !change.conflicted {
        return Err("The selected path is no longer conflicted. Refresh and try again.".into());
    }
    let path = path_from_token(&change.path.token)?;
    let content = read_conflict_content(Path::new(&snapshot.worktree_path), &path)?;
    Ok(ConflictFile {
        byte_length: content.len() as u64,
        content,
    })
}

pub fn resolve_conflict(request: ResolveConflictRequest) -> Result<WorkingCopySnapshot, String> {
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path,
        worktree_path: request.worktree_path,
    })?;
    if snapshot.head != request.expected_head {
        return Err("The working-copy HEAD changed after this resolution was reviewed.".into());
    }
    let change = snapshot
        .changes
        .iter()
        .find(|change| change.path.token == request.path.token)
        .ok_or_else(|| {
            "The selected conflict no longer exists. Refresh and try again.".to_string()
        })?;
    if !change.conflicted {
        return Err("The selected path is no longer conflicted. Refresh and try again.".into());
    }
    let path = path_from_token(&change.path.token)?;
    let worktree = PathBuf::from(&snapshot.worktree_path);
    if matches!(
        request.kind,
        ConflictResolutionKind::Both | ConflictResolutionKind::Manual
    ) {
        let expected = request.expected_content.as_deref().ok_or_else(|| {
            "The conflict resolution is missing the reviewed file content. Refresh and try again."
                .to_string()
        })?;
        if read_conflict_content(&worktree, &path)? != expected {
            return Err("The conflicted file changed after this resolution was reviewed. Refresh and review it again.".into());
        }
    }
    if request.kind == ConflictResolutionKind::Remove {
        let output = command::git_at(
            &worktree,
            [OsString::from("rm"), OsString::from("--"), path],
        )
        .map_err(|error| error.to_string())?;
        ensure_success(output, "remove the conflicted path")?;
    } else {
        if let Some(side) = match request.kind {
            ConflictResolutionKind::Ours => Some("--ours"),
            ConflictResolutionKind::Theirs => Some("--theirs"),
            ConflictResolutionKind::Both => {
                let content = merge_both_sides(&worktree, &path)?;
                write_conflict_content(&worktree, &path, &content)?;
                None
            }
            ConflictResolutionKind::Manual => {
                let content = request.manual_content.as_deref().ok_or_else(|| {
                    "The manual resolution content is missing. Review the file and try again."
                        .to_string()
                })?;
                if content.len() > MAX_CONFLICT_FILE_BYTES || content.contains('\0') {
                    return Err("Manual conflict resolutions must be UTF-8 text no larger than 2 MiB and cannot contain NUL bytes.".into());
                }
                write_conflict_content(&worktree, &path, content.as_bytes())?;
                None
            }
            ConflictResolutionKind::MarkResolved => None,
            ConflictResolutionKind::Remove => unreachable!(),
        } {
            let output = command::git_at(
                &worktree,
                [
                    OsString::from("checkout"),
                    OsString::from(side),
                    OsString::from("--"),
                    path.clone(),
                ],
            )
            .map_err(|error| error.to_string())?;
            ensure_success(output, "choose the requested conflict side")?;
        }
        let output = command::git_at(
            &worktree,
            [OsString::from("add"), OsString::from("--"), path],
        )
        .map_err(|error| error.to_string())?;
        ensure_success(output, "mark the conflict resolved")?;
    }
    working_copy_snapshot(WorkingCopyRequest {
        repository_path: snapshot.repository_path,
        worktree_path: snapshot.worktree_path,
    })
}

fn read_conflict_content(worktree: &Path, path: &OsString) -> Result<String, String> {
    let candidate = worktree.join(path);
    let metadata = fs::symlink_metadata(&candidate)
        .map_err(|error| format!("The conflicted file could not be inspected: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(
            "This conflict is not a regular text file. Choose ours, theirs, or remove it.".into(),
        );
    }
    if metadata.len() > MAX_CONFLICT_FILE_BYTES as u64 {
        return Err(
            "This conflicted file is larger than the 2 MiB manual-resolution limit.".into(),
        );
    }
    let bytes = fs::read(candidate)
        .map_err(|error| format!("The conflicted file could not be read: {error}"))?;
    String::from_utf8(bytes)
        .map_err(|_| "This conflict is not UTF-8 text. Choose ours, theirs, or remove it.".into())
}

fn write_conflict_content(worktree: &Path, path: &OsString, content: &[u8]) -> Result<(), String> {
    let candidate = worktree.join(path);
    let metadata = fs::symlink_metadata(&candidate)
        .map_err(|error| format!("The conflicted file could not be inspected: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(
            "This conflict is not a regular text file. Choose ours, theirs, or remove it.".into(),
        );
    }
    fs::write(candidate, content)
        .map_err(|error| format!("The conflict resolution could not be written: {error}"))
}

fn merge_both_sides(worktree: &Path, path: &OsString) -> Result<Vec<u8>, String> {
    let output = command::successful_git_at(
        worktree,
        [
            OsString::from("ls-files"),
            OsString::from("--stage"),
            OsString::from("-z"),
            OsString::from("--"),
            path.clone(),
        ],
    )
    .map_err(|error| error.to_string())?;
    let mut stages = BTreeMap::<u8, String>::new();
    for record in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let header = record.split(|byte| *byte == b'\t').next().unwrap_or(record);
        let fields: Vec<_> = header.split(|byte| *byte == b' ').collect();
        if fields.len() != 3 {
            return Err("Git returned malformed conflict-stage metadata.".into());
        }
        let stage = String::from_utf8_lossy(fields[2])
            .parse::<u8>()
            .map_err(|_| "Git returned an invalid conflict stage.".to_string())?;
        stages.insert(stage, String::from_utf8_lossy(fields[1]).into_owned());
    }
    let ours = stages.get(&2).ok_or_else(|| {
        "Keep both is unavailable because this path has no ours version.".to_string()
    })?;
    let theirs = stages.get(&3).ok_or_else(|| {
        "Keep both is unavailable because this path has no theirs version.".to_string()
    })?;
    let mut base_file = tempfile::NamedTempFile::new()
        .map_err(|error| format!("Could not prepare the merge base: {error}"))?;
    let mut ours_file = tempfile::NamedTempFile::new()
        .map_err(|error| format!("Could not prepare the ours version: {error}"))?;
    let mut theirs_file = tempfile::NamedTempFile::new()
        .map_err(|error| format!("Could not prepare the theirs version: {error}"))?;
    if let Some(base) = stages.get(&1) {
        base_file
            .write_all(&blob(worktree, base)?)
            .map_err(|error| format!("Could not prepare the merge base: {error}"))?;
    }
    ours_file
        .write_all(&blob(worktree, ours)?)
        .map_err(|error| format!("Could not prepare the ours version: {error}"))?;
    theirs_file
        .write_all(&blob(worktree, theirs)?)
        .map_err(|error| format!("Could not prepare the theirs version: {error}"))?;
    let output = command::git_at(
        worktree,
        [
            OsString::from("merge-file"),
            OsString::from("--union"),
            OsString::from("-p"),
            ours_file.path().as_os_str().to_owned(),
            base_file.path().as_os_str().to_owned(),
            theirs_file.path().as_os_str().to_owned(),
        ],
    )
    .map_err(|error| error.to_string())?;
    if output.status.code() == Some(255) || output.status.code().is_none() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    if output.stdout.len() > MAX_CONFLICT_FILE_BYTES {
        return Err("The combined conflict resolution exceeds the 2 MiB text limit.".into());
    }
    Ok(output.stdout)
}

fn blob(worktree: &Path, oid: &str) -> Result<Vec<u8>, String> {
    let output = command::successful_git_at(worktree, ["cat-file", "blob", oid])
        .map_err(|error| error.to_string())?;
    if output.stdout.len() > MAX_CONFLICT_FILE_BYTES {
        return Err("A conflict stage exceeds the 2 MiB text limit.".into());
    }
    Ok(output.stdout)
}

pub fn discard_file(request: DiscardFileRequest) -> Result<WorkingCopySnapshot, String> {
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path,
        worktree_path: request.worktree_path,
    })?;
    if snapshot.head != request.expected_head {
        return Err(
            "The working-copy HEAD changed after this discard was reviewed. Refresh and try again."
                .into(),
        );
    }
    let change = snapshot
        .changes
        .iter()
        .find(|change| change.path.token == request.path.token)
        .ok_or_else(|| {
            "The selected change no longer exists. Refresh and try again.".to_string()
        })?;
    if change.index_status != request.expected_index_status
        || change.worktree_status != request.expected_worktree_status
    {
        return Err(
            "The selected change changed after this discard was reviewed. Refresh and try again."
                .into(),
        );
    }
    if change.conflicted {
        return Err("Resolve the conflict explicitly instead of discarding it.".into());
    }
    if change.ignored {
        return Err("Repola does not discard ignored files.".into());
    }

    let worktree = PathBuf::from(&snapshot.worktree_path);
    let current = path_from_token(&change.path.token)?;
    match request.scope {
        DiscardScope::Unstaged => {
            if !change.unstaged && !change.untracked {
                return Err("The selected path has no unstaged changes to discard.".into());
            }
            if change.untracked {
                clean_path(&worktree, current)?;
            } else {
                restore_worktree_path(&worktree, current)?;
            }
        }
        DiscardScope::All => {
            if change.untracked {
                clean_path(&worktree, current)?;
            } else if snapshot.head.is_none() {
                let output = command::git_at(
                    &worktree,
                    [
                        OsString::from("rm"),
                        OsString::from("-f"),
                        OsString::from("--"),
                        current,
                    ],
                )
                .map_err(|error| error.to_string())?;
                ensure_success(output, "discard the path from the unborn branch")?;
            } else {
                discard_tracked_path(&worktree, change, current)?;
            }
        }
    }
    working_copy_snapshot(WorkingCopyRequest {
        repository_path: snapshot.repository_path,
        worktree_path: snapshot.worktree_path,
    })
}

pub fn discard_all(request: DiscardAllRequest) -> Result<WorkingCopySnapshot, String> {
    if request.expected_changes.is_empty() {
        return Err("There are no reviewed changes to discard.".into());
    }
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path,
        worktree_path: request.worktree_path,
    })?;
    if snapshot.head != request.expected_head {
        return Err(
            "The working-copy HEAD changed after discard-all was reviewed. Refresh and try again."
                .into(),
        );
    }
    if snapshot.operation.is_some() {
        return Err(
            "Abort or finish the in-progress Git operation before discarding every change.".into(),
        );
    }
    let mut reviewed = request.expected_changes;
    reviewed.sort_by(|left, right| left.path_token.cmp(&right.path_token));
    if reviewed
        .windows(2)
        .any(|pair| pair[0].path_token == pair[1].path_token)
    {
        return Err("The reviewed discard-all selection contains a duplicate path.".into());
    }
    let mut current: Vec<ReviewedFileChange> = snapshot
        .changes
        .iter()
        .filter(|change| !change.ignored)
        .map(|change| ReviewedFileChange {
            path_token: change.path.token.clone(),
            previous_path_token: change.previous_path.as_ref().map(|path| path.token.clone()),
            index_status: change.index_status.clone(),
            worktree_status: change.worktree_status.clone(),
        })
        .collect();
    current.sort_by(|left, right| left.path_token.cmp(&right.path_token));
    if current != reviewed {
        return Err(
            "The working-copy changes no longer match the reviewed discard-all plan. Refresh and review them again."
                .into(),
        );
    }

    let worktree = Path::new(&snapshot.worktree_path);
    if snapshot.head.is_some() {
        let output = command::git_at(
            worktree,
            ["reset", "--hard", "--recurse-submodules", "HEAD"],
        )
        .map_err(|error| error.to_string())?;
        ensure_success(output, "restore every tracked path")?;
    } else {
        let output = command::git_at(
            worktree,
            ["rm", "--cached", "-r", "--ignore-unmatch", "--", "."],
        )
        .map_err(|error| error.to_string())?;
        ensure_success(output, "clear the unborn branch index")?;
    }
    let output = command::git_at(worktree, ["clean", "-ffd", "--", "."])
        .map_err(|error| error.to_string())?;
    ensure_success(output, "remove every reviewed untracked path")?;
    working_copy_snapshot(WorkingCopyRequest {
        repository_path: snapshot.repository_path,
        worktree_path: snapshot.worktree_path,
    })
}

fn discard_tracked_path(
    worktree: &Path,
    change: &FileChange,
    current: OsString,
) -> Result<(), String> {
    let previous = change
        .previous_path
        .as_ref()
        .map(|path| path_from_token(&path.token))
        .transpose()?;
    let mut reset_paths = vec![current.clone()];
    if change.kind == FileChangeKind::Renamed {
        if let Some(path) = previous.as_ref() {
            reset_paths.push(path.clone());
        }
    }
    let mut reset = vec![
        OsString::from("restore"),
        OsString::from("--staged"),
        OsString::from("--"),
    ];
    reset.extend(reset_paths);
    let output = command::git_at(worktree, reset).map_err(|error| error.to_string())?;
    ensure_success(output, "reset the selected path in the index")?;

    match change.kind {
        FileChangeKind::Added | FileChangeKind::Copied => clean_path(worktree, current),
        FileChangeKind::Renamed => {
            let previous = previous
                .ok_or_else(|| "The renamed path lost its original filename.".to_string())?;
            restore_worktree_path(worktree, previous)?;
            clean_path(worktree, current)
        }
        _ => restore_worktree_path(worktree, current),
    }
}

fn restore_worktree_path(worktree: &Path, path: OsString) -> Result<(), String> {
    let output = command::git_at(
        worktree,
        [
            OsString::from("restore"),
            OsString::from("--worktree"),
            OsString::from("--"),
            path,
        ],
    )
    .map_err(|error| error.to_string())?;
    ensure_success(output, "restore the selected working-tree path")
}

fn clean_path(worktree: &Path, path: OsString) -> Result<(), String> {
    let output = command::git_at(
        worktree,
        [
            OsString::from("clean"),
            OsString::from("-f"),
            OsString::from("--"),
            path,
        ],
    )
    .map_err(|error| error.to_string())?;
    ensure_success(output, "remove the selected untracked path")
}

fn ensure_success(output: std::process::Output, intent: &str) -> Result<(), String> {
    if output.status.success() {
        Ok(())
    } else {
        let diagnostic = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if diagnostic.is_empty() {
            format!("Git could not {intent}.")
        } else {
            diagnostic
        })
    }
}

fn diff_paths(change: &FileChange) -> Result<Vec<OsString>, String> {
    let mut paths = Vec::with_capacity(if change.previous_path.is_some() { 2 } else { 1 });
    if let Some(previous) = &change.previous_path {
        paths.push(path_from_token(&previous.token)?);
    }
    paths.push(path_from_token(&change.path.token)?);
    Ok(paths)
}

fn git_diff_output(
    path: &Path,
    args: &[OsString],
    differences_are_status_one: bool,
) -> Result<Vec<u8>, String> {
    let output = command::git_at(path, args).map_err(|error| error.to_string())?;
    if output.status.success() || (differences_are_status_one && output.status.code() == Some(1)) {
        Ok(output.stdout)
    } else {
        let diagnostic = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if diagnostic.is_empty() {
            "Git could not render the selected file diff.".into()
        } else {
            diagnostic
        })
    }
}

pub(super) fn empty_tree(path: &Path) -> Result<String, String> {
    let output = command::git_at_with_input(path, ["hash-object", "-t", "tree", "--stdin"], b"")
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(unix)]
fn null_device() -> OsString {
    OsString::from("/dev/null")
}

#[cfg(windows)]
fn null_device() -> OsString {
    OsString::from("NUL")
}

pub(super) fn truncate_file_patch(bytes: &[u8]) -> (String, bool) {
    if bytes.len() <= MAX_FILE_PATCH_BYTES {
        return (String::from_utf8_lossy(bytes).into_owned(), false);
    }
    let boundary = bytes[..MAX_FILE_PATCH_BYTES]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .unwrap_or(MAX_FILE_PATCH_BYTES);
    (
        String::from_utf8_lossy(&bytes[..boundary]).into_owned(),
        true,
    )
}

pub fn commit(request: CommitRequest) -> Result<CommitResult, String> {
    validate_commit_message(&request.summary, &request.description)?;
    validate_commit_metadata(&request)?;
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path.clone(),
        worktree_path: request.worktree_path.clone(),
    })?;
    if snapshot.operation.is_some() && request.amend {
        return Err("Amending is unavailable while another Git operation is in progress.".into());
    }
    if request.amend && snapshot.head.is_none() {
        return Err("There is no commit to amend.".into());
    }
    if snapshot.head != request.expected_head {
        return Err(
            "The working-copy HEAD changed after the commit was reviewed. Refresh and try again."
                .into(),
        );
    }
    if !request.amend && request.included_changes.is_empty() {
        return Err("Include at least one change before committing.".into());
    }
    validate_commit_selections(&snapshot, &request.included_changes)?;
    let mut message = request.summary.trim().as_bytes().to_vec();
    if !request.description.is_empty() {
        message.extend_from_slice(b"\n\n");
        message.extend_from_slice(request.description.as_bytes());
    }
    if !request.co_authors.is_empty() || !request.trailers.is_empty() {
        message.extend_from_slice(b"\n\n");
        for person in &request.co_authors {
            message.extend_from_slice(
                format!(
                    "Co-authored-by: {} <{}>\n",
                    person.name.trim(),
                    person.email.trim()
                )
                .as_bytes(),
            );
        }
        for trailer in &request.trailers {
            message.extend_from_slice(
                format!("{}: {}\n", trailer.key.trim(), trailer.value.trim()).as_bytes(),
            );
        }
    }
    if !message.ends_with(b"\n") {
        message.push(b'\n');
    }
    let worktree = PathBuf::from(&snapshot.worktree_path);
    let temporary_index = tempfile::tempdir()
        .map_err(|error| format!("Could not create a temporary Git index: {error}"))?;
    let index_path = temporary_index.path().join("index");
    initialize_commit_index(&worktree, snapshot.head.as_deref(), &index_path)?;
    populate_commit_index(&snapshot, &request.included_changes, &worktree, &index_path)?;
    if !request.amend
        && commit_index_matches_base(&worktree, &index_path, snapshot.head.as_deref())?
    {
        return Err("The included changes do not produce a commit.".into());
    }

    let mut args = vec![OsString::from("commit")];
    if request.amend {
        args.push(OsString::from("--amend"));
    }
    args.push(OsString::from("--file=-"));
    if let Some(author) = &request.author {
        args.push(OsString::from(format!(
            "--author={} <{}>",
            author.name.trim(),
            author.email.trim()
        )));
    }
    match request.signing {
        CommitSigning::Default => {}
        CommitSigning::Sign => args.push(OsString::from("--gpg-sign")),
        CommitSigning::DoNotSign => args.push(OsString::from("--no-gpg-sign")),
    }
    let output = command::git_at_with_input_and_env(
        &worktree,
        args,
        &message,
        [("GIT_INDEX_FILE", index_path.as_os_str())],
    )
    .map_err(|error| error.to_string())?;
    let hook_output = combined_output(&output.stdout, &output.stderr);
    if !output.status.success() {
        return Err(if hook_output.is_empty() {
            "Git rejected the commit without diagnostic output.".into()
        } else {
            hook_output
        });
    }
    let commit = git_text(&worktree, ["rev-parse", "HEAD"])?;
    let reset = command::git_at(&worktree, ["reset", "--mixed", "--quiet", "HEAD"])
        .map_err(|error| error.to_string())?;
    ensure_success(reset, "reset the Git index after committing")?;
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: snapshot.repository_path,
        worktree_path: snapshot.worktree_path,
    })?;
    Ok(CommitResult {
        commit,
        snapshot,
        hook_output,
    })
}

fn validate_commit_selections(
    snapshot: &WorkingCopySnapshot,
    selections: &[CommitFileSelection],
) -> Result<(), String> {
    let current: BTreeMap<&str, &FileChange> = snapshot
        .changes
        .iter()
        .map(|change| (change.path.token.as_str(), change))
        .collect();
    let mut selected_tokens = BTreeSet::new();

    for selection in selections {
        if !selected_tokens.insert(selection.path.token.as_str()) {
            return Err(format!(
                "{} was included more than once.",
                selection.path.display
            ));
        }
        let change = current
            .get(selection.path.token.as_str())
            .ok_or_else(|| format!("{} changed after it was reviewed.", selection.path.display))?;
        if change.ignored {
            return Err(format!(
                "{} is ignored and cannot be included implicitly.",
                change.path.display
            ));
        }
        if change.conflicted {
            return Err(format!(
                "Resolve the conflict in {} before committing.",
                change.path.display
            ));
        }
        if change.previous_path != selection.previous_path
            || change.index_status != selection.expected_index_status
            || change.worktree_status != selection.expected_worktree_status
        {
            return Err(format!(
                "{} changed after it was reviewed. Refresh and try again.",
                change.path.display
            ));
        }
        if selection.include_all && !selection.hunks.is_empty() {
            return Err(format!(
                "{} has both whole-file and partial commit selections.",
                change.path.display
            ));
        }
        if !selection.include_all && selection.hunks.is_empty() {
            return Err(format!(
                "{} has no selected lines to commit.",
                change.path.display
            ));
        }
    }
    Ok(())
}

fn initialize_commit_index(
    worktree: &Path,
    head: Option<&str>,
    index_path: &Path,
) -> Result<(), String> {
    let args = match head {
        Some(head) => vec![OsString::from("read-tree"), OsString::from(head)],
        None => vec![OsString::from("read-tree"), OsString::from("--empty")],
    };
    let output =
        command::git_at_with_env(worktree, args, [("GIT_INDEX_FILE", index_path.as_os_str())])
            .map_err(|error| error.to_string())?;
    ensure_success(output, "initialize the commit index")
}

fn populate_commit_index(
    snapshot: &WorkingCopySnapshot,
    selections: &[CommitFileSelection],
    worktree: &Path,
    index_path: &Path,
) -> Result<(), String> {
    let mut whole_paths = BTreeSet::<OsString>::new();
    for selection in selections.iter().filter(|selection| selection.include_all) {
        whole_paths.insert(path_from_token(&selection.path.token)?);
        if let Some(previous) = &selection.previous_path {
            whole_paths.insert(path_from_token(&previous.token)?);
        }
    }
    if !whole_paths.is_empty() {
        let mut args = vec![
            OsString::from("add"),
            OsString::from("-A"),
            OsString::from("--"),
        ];
        args.extend(whole_paths);
        let output =
            command::git_at_with_env(worktree, args, [("GIT_INDEX_FILE", index_path.as_os_str())])
                .map_err(|error| error.to_string())?;
        ensure_success(output, "include the selected files in the commit")?;
    }

    for selection in selections.iter().filter(|selection| !selection.include_all) {
        let change = snapshot
            .changes
            .iter()
            .find(|change| change.path.token == selection.path.token)
            .ok_or_else(|| {
                "A selected change disappeared during commit preparation.".to_string()
            })?;
        if matches!(
            change.kind,
            FileChangeKind::Renamed | FileChangeKind::Copied
        ) || change.submodule
        {
            return Err(format!(
                "{} must be included as a whole file.",
                change.path.display
            ));
        }
        let diff = file_diff(FileDiffRequest {
            repository_path: snapshot.repository_path.clone(),
            worktree_path: snapshot.worktree_path.clone(),
            path: selection.path.clone(),
        })?;
        if diff.binary || diff.truncated {
            return Err(format!(
                "{} must be included as a whole file.",
                change.path.display
            ));
        }
        apply_commit_hunks(worktree, index_path, &diff.hunks, &selection.hunks)?;
    }
    Ok(())
}

fn apply_commit_hunks(
    worktree: &Path,
    index_path: &Path,
    current_hunks: &[PatchHunk],
    selections: &[CommitHunkSelection],
) -> Result<(), String> {
    let mut selected_patches = BTreeSet::new();
    for selection in selections {
        if selection.expected_patch.len() > MAX_FILE_PATCH_BYTES {
            return Err("A selected patch hunk is too large to commit safely.".into());
        }
        if !selected_patches.insert(selection.expected_patch.as_str()) {
            return Err("A patch hunk was selected more than once.".into());
        }
        let hunk = current_hunks
            .iter()
            .find(|hunk| hunk.patch == selection.expected_patch)
            .ok_or_else(|| {
                "A selected patch hunk changed after it was reviewed. Refresh and try again."
                    .to_string()
            })?;
        if selection.selected_line_indices.is_empty() {
            return Err("A partial commit hunk has no selected lines.".into());
        }
        let patch = partial_hunk_patch(&hunk.patch, &selection.selected_line_indices)?;
        let output = command::git_at_with_input_and_env(
            worktree,
            ["apply", "--cached", "--whitespace=nowarn"],
            patch.as_bytes(),
            [("GIT_INDEX_FILE", index_path.as_os_str())],
        )
        .map_err(|error| error.to_string())?;
        ensure_success(output, "include the selected lines in the commit")?;
    }
    Ok(())
}

fn commit_index_matches_base(
    worktree: &Path,
    index_path: &Path,
    head: Option<&str>,
) -> Result<bool, String> {
    let base = match head {
        Some(head) => head.to_string(),
        None => empty_tree(worktree)?,
    };
    let output = command::git_at_with_env(
        worktree,
        ["diff", "--cached", "--quiet", "--exit-code", &base],
        [("GIT_INDEX_FILE", index_path.as_os_str())],
    )
    .map_err(|error| error.to_string())?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
    }
}

fn validate_commit_metadata(request: &CommitRequest) -> Result<(), String> {
    if request.co_authors.len() > 20 {
        return Err("A commit can include at most 20 co-authors.".into());
    }
    if request.trailers.len() > 50 {
        return Err("A commit can include at most 50 additional trailers.".into());
    }
    if let Some(author) = &request.author {
        validate_commit_person(author, "author")?;
    }
    for person in &request.co_authors {
        validate_commit_person(person, "co-author")?;
    }
    for trailer in &request.trailers {
        validate_commit_trailer(trailer)?;
    }
    Ok(())
}

fn validate_commit_person(person: &CommitPerson, label: &str) -> Result<(), String> {
    let name = person.name.trim();
    let email = person.email.trim();
    if name.is_empty()
        || name.len() > 200
        || name.contains(['\0', '\n', '\r', '<', '>'])
        || name.chars().any(char::is_control)
    {
        return Err(format!("The {label} name is invalid."));
    }
    if email.is_empty()
        || email.len() > 320
        || !email.contains('@')
        || email.contains(['\0', '\n', '\r', '<', '>'])
        || email.chars().any(char::is_whitespace)
    {
        return Err(format!("The {label} email address is invalid."));
    }
    Ok(())
}

fn validate_commit_trailer(trailer: &CommitTrailer) -> Result<(), String> {
    let key = trailer.key.trim();
    let value = trailer.value.trim();
    if key.is_empty()
        || key.len() > 100
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("Trailer keys may contain only letters, numbers, and hyphens.".into());
    }
    if key.eq_ignore_ascii_case("co-authored-by") {
        return Err(
            "Add co-authors through the co-author field instead of a custom trailer.".into(),
        );
    }
    if value.is_empty()
        || value.len() > 998
        || value.contains(['\0', '\n', '\r'])
        || value.chars().any(char::is_control)
    {
        return Err(format!("The {key} trailer value is invalid."));
    }
    Ok(())
}

pub fn undo_commit(request: UndoCommitRequest) -> Result<UndoCommitResult, String> {
    let before = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path.clone(),
        worktree_path: request.worktree_path.clone(),
    })?;
    if before.head.as_deref() != Some(request.expected_head.as_str()) {
        return Err(
            "The latest commit changed after this undo was reviewed. Refresh and try again.".into(),
        );
    }
    if before.branch.is_none() {
        return Err("Check out a branch before undoing a commit.".into());
    }
    if before.operation.is_some() {
        return Err(
            "Finish or abort the in-progress Git operation before undoing a commit.".into(),
        );
    }
    let contains = format!("--contains={}", request.expected_head);
    let published = command::successful_git_at(
        Path::new(&before.worktree_path),
        [
            "for-each-ref",
            "--format=%(refname)",
            &contains,
            "refs/remotes",
        ],
    )
    .map_err(|error| error.to_string())?;
    if !published.stdout.is_empty() {
        return Err("The latest commit is present on a remote branch and cannot be safely undone. Revert it instead.".into());
    }

    let metadata = command::successful_git_at(
        Path::new(&before.worktree_path),
        ["show", "-s", "--format=%s%x00%b", &request.expected_head],
    )
    .map_err(|error| error.to_string())?;
    let mut fields = metadata.stdout.splitn(2, |byte| *byte == 0);
    let summary = String::from_utf8_lossy(fields.next().unwrap_or_default())
        .trim_end()
        .to_string();
    let description = String::from_utf8_lossy(fields.next().unwrap_or_default())
        .trim_end()
        .to_string();

    let parent = command::git_at(
        Path::new(&before.worktree_path),
        ["rev-parse", "--verify", "--quiet", "HEAD^"],
    )
    .map_err(|error| error.to_string())?;
    let output = if parent.status.success() {
        let parent = String::from_utf8_lossy(&parent.stdout).trim().to_string();
        command::git_at(
            Path::new(&before.worktree_path),
            ["reset", "--soft", &parent],
        )
    } else {
        // Deleting the symbolic branch ref is the root-commit equivalent of a
        // soft reset: the index and working tree remain untouched and staged.
        command::git_at(
            Path::new(&before.worktree_path),
            ["update-ref", "-d", "HEAD", &request.expected_head],
        )
    }
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        let diagnostic = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if diagnostic.is_empty() {
            "Git could not undo the latest commit and returned no diagnostic output.".into()
        } else {
            diagnostic
        });
    }
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: before.repository_path,
        worktree_path: before.worktree_path,
    })?;
    Ok(UndoCommitResult {
        commit: request.expected_head,
        summary,
        description,
        snapshot,
    })
}

fn validate_commit_message(summary: &str, description: &str) -> Result<(), String> {
    if summary.trim().is_empty() {
        return Err("A commit summary is required.".into());
    }
    if summary.contains(['\n', '\r', '\0']) {
        return Err("The commit summary must be one line.".into());
    }
    if description.contains('\0') {
        return Err("The commit description contains an unsupported NUL character.".into());
    }
    if summary.len() > MAX_COMMIT_SUMMARY_BYTES {
        return Err(format!(
            "The commit summary exceeds {MAX_COMMIT_SUMMARY_BYTES} bytes."
        ));
    }
    if description.len() > MAX_COMMIT_DESCRIPTION_BYTES {
        return Err(format!(
            "The commit description exceeds {MAX_COMMIT_DESCRIPTION_BYTES} bytes."
        ));
    }
    Ok(())
}

fn combined_output(stdout: &[u8], stderr: &[u8]) -> String {
    [stdout, stderr]
        .into_iter()
        .map(|bytes| String::from_utf8_lossy(bytes).trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn path_from_token(token: &str) -> Result<OsString, String> {
    let bytes = decode_hex(token)?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(OsString::from_vec(bytes))
    }
    #[cfg(windows)]
    {
        String::from_utf8(bytes)
            .map(OsString::from)
            .map_err(|_| "Git returned a path that Windows could not decode as UTF-8.".into())
    }
}

fn decode_hex(token: &str) -> Result<Vec<u8>, String> {
    if !token.len().is_multiple_of(2) {
        return Err("A selected path token was malformed.".into());
    }
    token
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let high = hex_digit(pair[0])?;
            let low = hex_digit(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_digit(value: u8) -> Result<u8, String> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err("A selected path token was malformed.".into()),
    }
}

fn canonical_directory(value: &str, label: &str) -> Result<PathBuf, String> {
    let requested = PathBuf::from(value);
    let canonical = dunce::canonicalize(&requested)
        .map_err(|error| format!("The {label} path could not be opened: {error}"))?;
    if !canonical.is_dir() {
        return Err(format!("The {label} path is not a directory."));
    }
    Ok(canonical)
}

fn git_text<const N: usize>(path: &Path, args: [&str; N]) -> Result<String, String> {
    let output = command::successful_git_at(path, args).map_err(|error| error.to_string())?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn resolve_remote(worktree: &Path, branch: Option<&str>) -> Option<String> {
    if let Some(branch) = branch {
        let reference = format!("refs/heads/{branch}");
        if let Ok(output) = command::git_at(
            worktree,
            [
                "for-each-ref",
                "--count=1",
                "--format=%(upstream:remotename)",
                reference.as_str(),
            ],
        ) {
            let remote = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if output.status.success() && !remote.is_empty() {
                return Some(remote);
            }
        }
    }
    command::git_at(worktree, ["remote", "get-url", "origin"])
        .ok()
        .filter(|output| output.status.success())
        .map(|_| "origin".to_string())
}

fn parse_status(bytes: &[u8]) -> Result<WorkingCopySnapshot, String> {
    let mut snapshot = WorkingCopySnapshot {
        repository_path: String::new(),
        worktree_path: String::new(),
        head: None,
        branch: None,
        upstream: None,
        upstream_head: None,
        remote: None,
        ahead: 0,
        behind: 0,
        changes: Vec::new(),
        operation: None,
    };
    let fields: Vec<&[u8]> = bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect();
    let mut index = 0;
    while index < fields.len() {
        let field = fields[index];
        if field.starts_with(b"# ") {
            parse_header(field, &mut snapshot)?;
        } else if field.starts_with(b"1 ") {
            snapshot.changes.push(parse_ordinary(field)?);
        } else if field.starts_with(b"2 ") {
            let previous = fields
                .get(index + 1)
                .ok_or_else(|| "Rename record was missing its original path.".to_string())?;
            snapshot.changes.push(parse_rename(field, previous)?);
            index += 1;
        } else if field.starts_with(b"u ") {
            snapshot.changes.push(parse_unmerged(field)?);
        } else if let Some(path) = field.strip_prefix(b"? ") {
            snapshot
                .changes
                .push(simple_change(path, FileChangeKind::Untracked, '?', '?'));
        } else if let Some(path) = field.strip_prefix(b"! ") {
            snapshot
                .changes
                .push(simple_change(path, FileChangeKind::Ignored, '!', '!'));
        } else {
            return Err(format!(
                "Git returned an unsupported porcelain-v2 record: {}",
                String::from_utf8_lossy(field)
            ));
        }
        index += 1;
    }
    snapshot.changes.sort_by(|left, right| {
        left.path
            .display
            .to_lowercase()
            .cmp(&right.path.display.to_lowercase())
    });
    Ok(snapshot)
}

fn parse_header(field: &[u8], snapshot: &mut WorkingCopySnapshot) -> Result<(), String> {
    if let Some(value) = field.strip_prefix(b"# branch.oid ") {
        let value = String::from_utf8_lossy(value);
        snapshot.head = (value != "(initial)").then(|| value.into_owned());
    } else if let Some(value) = field.strip_prefix(b"# branch.head ") {
        let value = String::from_utf8_lossy(value);
        snapshot.branch = (value != "(detached)").then(|| value.into_owned());
    } else if let Some(value) = field.strip_prefix(b"# branch.upstream ") {
        snapshot.upstream = Some(String::from_utf8_lossy(value).into_owned());
    } else if let Some(value) = field.strip_prefix(b"# branch.ab +") {
        let value = String::from_utf8(value.to_vec())
            .map_err(|_| "Git returned non-UTF-8 ahead/behind metadata.".to_string())?;
        let (ahead, behind) = value
            .split_once(" -")
            .ok_or_else(|| "Git returned malformed ahead/behind metadata.".to_string())?;
        snapshot.ahead = ahead
            .parse()
            .map_err(|_| "Git returned an invalid ahead count.".to_string())?;
        snapshot.behind = behind
            .parse()
            .map_err(|_| "Git returned an invalid behind count.".to_string())?;
    }
    Ok(())
}

fn parse_ordinary(field: &[u8]) -> Result<FileChange, String> {
    let parts = split_prefix(field, 8, "ordinary")?;
    let (index_status, worktree_status) = parse_xy(parts[1], "ordinary")?;
    Ok(change(
        parts[8],
        None,
        kind_for(index_status, worktree_status),
        index_status,
        worktree_status,
        parts[2] != b"N...",
        Some((parts[3], parts[4], parts[5])),
    ))
}

fn parse_rename(field: &[u8], previous: &[u8]) -> Result<FileChange, String> {
    let parts = split_prefix(field, 9, "rename/copy")?;
    let (index_status, worktree_status) = parse_xy(parts[1], "rename/copy")?;
    let score = parts[8].first().copied().unwrap_or(b'R');
    let kind = if score == b'C' {
        FileChangeKind::Copied
    } else {
        FileChangeKind::Renamed
    };
    Ok(change(
        parts[9],
        Some(previous),
        kind,
        index_status,
        worktree_status,
        parts[2] != b"N...",
        Some((parts[3], parts[4], parts[5])),
    ))
}

fn parse_unmerged(field: &[u8]) -> Result<FileChange, String> {
    let parts = split_prefix(field, 10, "unmerged")?;
    let (index_status, worktree_status) = parse_xy(parts[1], "unmerged")?;
    Ok(change(
        parts[10],
        None,
        FileChangeKind::Unmerged,
        index_status,
        worktree_status,
        parts[2] != b"N...",
        None,
    ))
}

fn split_prefix<'a>(
    field: &'a [u8],
    path_index: usize,
    label: &str,
) -> Result<Vec<&'a [u8]>, String> {
    let parts: Vec<&[u8]> = field.splitn(path_index + 1, |byte| *byte == b' ').collect();
    if parts.len() != path_index + 1 {
        return Err(format!("Git returned a malformed {label} status record."));
    }
    Ok(parts)
}

fn parse_xy(value: &[u8], label: &str) -> Result<(char, char), String> {
    if value.len() != 2 || !value.is_ascii() {
        return Err(format!("Git returned invalid {label} XY status."));
    }
    Ok((value[0] as char, value[1] as char))
}

fn simple_change(path: &[u8], kind: FileChangeKind, index: char, worktree: char) -> FileChange {
    change(path, None, kind, index, worktree, false, None)
}

fn change(
    path: &[u8],
    previous: Option<&[u8]>,
    kind: FileChangeKind,
    index_status: char,
    worktree_status: char,
    submodule: bool,
    modes: Option<(&[u8], &[u8], &[u8])>,
) -> FileChange {
    let path = git_path(path);
    let (head_mode, index_mode, worktree_mode, mode_change) = modes
        .map(|(head, index, worktree)| {
            let head = String::from_utf8_lossy(head).into_owned();
            let index = String::from_utf8_lossy(index).into_owned();
            let worktree = String::from_utf8_lossy(worktree).into_owned();
            let changed = if index_status != '.' {
                classify_mode_change(&head, &index)
            } else if worktree_status != '.' {
                classify_mode_change(&index, &worktree)
            } else {
                None
            };
            (Some(head), Some(index), Some(worktree), changed)
        })
        .unwrap_or((None, None, None, None));
    FileChange {
        id: path.token.clone(),
        path,
        previous_path: previous.map(git_path),
        kind,
        index_status: index_status.to_string(),
        worktree_status: worktree_status.to_string(),
        staged: !matches!(index_status, '.' | '?' | '!'),
        unstaged: !matches!(worktree_status, '.' | '!'),
        conflicted: kind == FileChangeKind::Unmerged,
        untracked: kind == FileChangeKind::Untracked,
        ignored: kind == FileChangeKind::Ignored,
        submodule,
        head_mode,
        index_mode,
        worktree_mode,
        mode_change,
    }
}

fn classify_mode_change(from: &str, to: &str) -> Option<FileModeChange> {
    if from == to || from == "000000" || to == "000000" {
        return None;
    }
    if from == "120000" || to == "120000" {
        return Some(FileModeChange::Symlink);
    }
    if matches!((from, to), ("100644", "100755") | ("100755", "100644")) {
        return Some(FileModeChange::ExecutableBit);
    }
    Some(FileModeChange::Other)
}

fn kind_for(index: char, worktree: char) -> FileChangeKind {
    let status = if worktree != '.' { worktree } else { index };
    match status {
        'A' => FileChangeKind::Added,
        'M' => FileChangeKind::Modified,
        'D' => FileChangeKind::Deleted,
        'R' => FileChangeKind::Renamed,
        'C' => FileChangeKind::Copied,
        'T' => FileChangeKind::TypeChanged,
        _ => FileChangeKind::Unknown,
    }
}

pub(super) fn git_path(bytes: &[u8]) -> GitPath {
    GitPath {
        display: String::from_utf8_lossy(bytes).into_owned(),
        token: hex(bytes),
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(DIGITS[(byte >> 4) as usize] as char);
        encoded.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn detect_operation(worktree: &Path) -> Option<RepositoryOperation> {
    let git_dir = git_text(
        worktree,
        ["rev-parse", "--path-format=absolute", "--git-dir"],
    )
    .ok()?;
    let git_dir = PathBuf::from(git_dir);
    if git_dir.join("MERGE_HEAD").is_file() {
        Some(RepositoryOperation::Merge)
    } else if git_dir.join("rebase-merge").is_dir() || git_dir.join("rebase-apply").is_dir() {
        Some(RepositoryOperation::Rebase)
    } else if git_dir.join("CHERRY_PICK_HEAD").is_file() {
        Some(RepositoryOperation::CherryPick)
    } else if git_dir.join("REVERT_HEAD").is_file() {
        Some(RepositoryOperation::Revert)
    } else if git_dir.join("BISECT_LOG").is_file() {
        Some(RepositoryOperation::Bisect)
    } else if git_dir.join("sequencer").is_dir() {
        Some(RepositoryOperation::Sequencer)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("temp repository");
        command::successful_git_at(directory.path(), ["init"]).expect("initialize repository");
        command::successful_git_at(directory.path(), ["config", "user.name", "Repola Test"])
            .expect("configure name");
        command::successful_git_at(
            directory.path(),
            ["config", "user.email", "repola@example.invalid"],
        )
        .expect("configure email");
        directory
    }

    fn request(path: &Path) -> WorkingCopyRequest {
        WorkingCopyRequest {
            repository_path: path.to_string_lossy().into_owned(),
            worktree_path: path.to_string_lossy().into_owned(),
        }
    }

    fn include_whole(change: &FileChange) -> CommitFileSelection {
        CommitFileSelection {
            path: change.path.clone(),
            previous_path: change.previous_path.clone(),
            expected_index_status: change.index_status.clone(),
            expected_worktree_status: change.worktree_status.clone(),
            include_all: true,
            hunks: Vec::new(),
        }
    }

    fn conflicted_repository() -> (tempfile::TempDir, WorkingCopySnapshot, GitPath) {
        let repository = repository();
        std::fs::write(repository.path().join("conflict.txt"), "base\n").expect("base");
        command::successful_git_at(repository.path(), ["add", "--", "conflict.txt"])
            .expect("stage base");
        command::successful_git_at(repository.path(), ["commit", "-m", "base"])
            .expect("commit base");
        let original =
            git_text(repository.path(), ["branch", "--show-current"]).expect("current branch");
        command::successful_git_at(repository.path(), ["branch", "feature"]).expect("branch");
        std::fs::write(repository.path().join("conflict.txt"), "ours\n").expect("ours");
        command::successful_git_at(repository.path(), ["commit", "-am", "ours"])
            .expect("commit ours");
        command::successful_git_at(repository.path(), ["switch", "feature"])
            .expect("switch feature");
        std::fs::write(repository.path().join("conflict.txt"), "theirs\n").expect("theirs");
        command::successful_git_at(repository.path(), ["commit", "-am", "theirs"])
            .expect("commit theirs");
        command::successful_git_at(repository.path(), ["switch", &original])
            .expect("switch original");
        let merge = command::git_at(repository.path(), ["merge", "feature"]).expect("run merge");
        assert!(!merge.status.success());
        let conflicted = working_copy_snapshot(request(repository.path())).expect("conflict");
        let path = conflicted
            .changes
            .iter()
            .find(|change| change.conflicted)
            .expect("conflicted path")
            .path
            .clone();
        (repository, conflicted, path)
    }

    #[test]
    fn parses_porcelain_v2_branch_and_every_record_shape() {
        let bytes = b"# branch.oid abc123\x00# branch.head feature\x00# branch.upstream origin/feature\x00# branch.ab +3 -2\x001 M. N... 100644 100644 100644 abc def staged.txt\x001 M. N... 100644 100755 100755 abc def exec.sh\x001 T. N... 100644 120000 120000 abc def link\x00? untracked.txt\x00! ignored.log\x002 R. N... 100644 100644 100644 abc def R100 renamed.txt\x00old.txt\x00u UU N... 100644 100644 100644 100644 a b c conflict.txt\x00";
        let snapshot = parse_status(bytes).expect("valid porcelain");
        assert_eq!(snapshot.branch.as_deref(), Some("feature"));
        assert_eq!(snapshot.upstream.as_deref(), Some("origin/feature"));
        assert_eq!((snapshot.ahead, snapshot.behind), (3, 2));
        assert_eq!(snapshot.changes.len(), 7);
        assert!(snapshot.changes.iter().any(|change| change.staged));
        assert!(snapshot.changes.iter().any(|change| change.untracked));
        assert!(snapshot.changes.iter().any(|change| change.ignored));
        let renamed = snapshot
            .changes
            .iter()
            .find(|change| change.kind == FileChangeKind::Renamed)
            .expect("rename");
        assert_eq!(
            renamed
                .previous_path
                .as_ref()
                .map(|path| path.display.as_str()),
            Some("old.txt")
        );
        assert!(snapshot.changes.iter().any(|change| change.conflicted));
        assert!(snapshot
            .changes
            .iter()
            .any(|change| change.mode_change == Some(FileModeChange::ExecutableBit)));
        assert!(snapshot
            .changes
            .iter()
            .any(|change| change.mode_change == Some(FileModeChange::Symlink)));
    }

    #[test]
    fn parses_tens_of_thousands_of_changes_without_a_display_bound() {
        let mut porcelain = Vec::with_capacity(600_000);
        for index in (0..25_000).rev() {
            porcelain
                .extend_from_slice(format!("? src/generated/file-{index:05}.txt\0").as_bytes());
        }
        let snapshot = parse_status(&porcelain).expect("large porcelain status");
        assert_eq!(snapshot.changes.len(), 25_000);
        assert_eq!(
            snapshot.changes[0].path.display,
            "src/generated/file-00000.txt"
        );
        assert_eq!(
            snapshot.changes[24_999].path.display,
            "src/generated/file-24999.txt"
        );
    }

    #[test]
    fn path_tokens_preserve_non_utf8_bytes() {
        let path = git_path(b"bad-\xff-name");
        assert_eq!(path.token, "6261642dff2d6e616d65");
        assert!(path.display.contains('\u{fffd}'));
    }

    #[test]
    fn renders_exact_untracked_and_binary_file_diffs() {
        let repository = repository();
        std::fs::write(repository.path().join("hello.txt"), "hello\n").expect("text file");
        let snapshot = working_copy_snapshot(request(repository.path())).expect("snapshot");
        let text_path = snapshot
            .changes
            .iter()
            .find(|change| change.path.display == "hello.txt")
            .expect("text change")
            .path
            .clone();
        let text = file_diff(FileDiffRequest {
            repository_path: snapshot.repository_path.clone(),
            worktree_path: snapshot.worktree_path.clone(),
            path: text_path.clone(),
        })
        .expect("text diff");
        assert!(text.patch.contains("+hello"));
        assert!(!text.binary);

        let staged = set_file_staging(SetFileStagingRequest {
            repository_path: snapshot.repository_path.clone(),
            worktree_path: snapshot.worktree_path.clone(),
            paths: vec![text_path],
            staged: true,
        })
        .expect("stage text");
        let included_changes = staged.changes.iter().map(include_whole).collect();
        commit(CommitRequest {
            repository_path: staged.repository_path.clone(),
            worktree_path: staged.worktree_path.clone(),
            expected_head: staged.head.clone(),
            included_changes,
            summary: "initial".into(),
            description: String::new(),
            amend: false,
            author: None,
            co_authors: Vec::new(),
            trailers: Vec::new(),
            signing: CommitSigning::Default,
        })
        .expect("commit text");
        std::fs::write(repository.path().join("hello.txt"), b"hello\0binary\n")
            .expect("binary edit");
        let binary_snapshot = working_copy_snapshot(request(repository.path())).expect("snapshot");
        let binary_path = binary_snapshot
            .changes
            .iter()
            .find(|change| change.path.display == "hello.txt")
            .expect("binary change")
            .path
            .clone();
        let binary = file_diff(FileDiffRequest {
            repository_path: binary_snapshot.repository_path,
            worktree_path: binary_snapshot.worktree_path,
            path: binary_path,
        })
        .expect("binary diff");
        assert!(binary.binary);

        std::fs::write(
            repository.path().join("pixel.png"),
            b"\x89PNG\r\n\x1a\n\0bounded-preview",
        )
        .expect("png");
        let image_snapshot = working_copy_snapshot(request(repository.path())).expect("snapshot");
        let image_path = image_snapshot
            .changes
            .iter()
            .find(|change| change.path.display == "pixel.png")
            .expect("image change")
            .path
            .clone();
        let image = file_diff(FileDiffRequest {
            repository_path: image_snapshot.repository_path,
            worktree_path: image_snapshot.worktree_path,
            path: image_path,
        })
        .expect("image diff");
        assert!(image.binary);
        let preview = image.image.expect("raster preview");
        assert_eq!(preview.mime_type, "image/png");
        assert_eq!(preview.byte_length, 24);
        assert!(!preview.base64.is_empty());
    }

    #[test]
    fn commits_only_included_files_without_leaking_the_real_index() {
        let repository = repository();
        std::fs::write(repository.path().join("included.txt"), "before included\n")
            .expect("included base");
        std::fs::write(repository.path().join("excluded.txt"), "before excluded\n")
            .expect("excluded base");
        command::successful_git_at(repository.path(), ["add", "--", "."]).expect("stage base");
        command::successful_git_at(repository.path(), ["commit", "-m", "base"])
            .expect("commit base");

        std::fs::write(repository.path().join("included.txt"), "after included\n")
            .expect("included edit");
        std::fs::write(repository.path().join("excluded.txt"), "after excluded\n")
            .expect("excluded edit");
        command::successful_git_at(repository.path(), ["add", "--", "excluded.txt"])
            .expect("stage excluded file outside Repola");
        let snapshot = working_copy_snapshot(request(repository.path())).expect("snapshot");
        let included = snapshot
            .changes
            .iter()
            .find(|change| change.path.display == "included.txt")
            .map(include_whole)
            .expect("included change");

        let result = commit(CommitRequest {
            repository_path: snapshot.repository_path.clone(),
            worktree_path: snapshot.worktree_path.clone(),
            expected_head: snapshot.head.clone(),
            included_changes: vec![included],
            summary: "include one file".into(),
            description: String::new(),
            amend: false,
            author: None,
            co_authors: Vec::new(),
            trailers: Vec::new(),
            signing: CommitSigning::DoNotSign,
        })
        .expect("commit included file");

        assert_eq!(
            git_text(repository.path(), ["show", "HEAD:included.txt"]).expect("included blob"),
            "after included"
        );
        assert_eq!(
            git_text(repository.path(), ["show", "HEAD:excluded.txt"]).expect("excluded blob"),
            "before excluded"
        );
        assert_eq!(
            std::fs::read_to_string(repository.path().join("excluded.txt"))
                .expect("excluded working file"),
            "after excluded\n"
        );
        let remaining = result
            .snapshot
            .changes
            .iter()
            .find(|change| change.path.display == "excluded.txt")
            .expect("excluded change remains");
        assert!(!remaining.staged);
        assert!(remaining.unstaged);
    }

    #[cfg(unix)]
    #[test]
    fn a_rejected_commit_preserves_the_real_index_exactly() {
        use std::os::unix::fs::PermissionsExt;

        let repository = repository();
        std::fs::write(repository.path().join("included.txt"), "base included\n")
            .expect("included base");
        std::fs::write(repository.path().join("staged.txt"), "base staged\n").expect("staged base");
        command::successful_git_at(repository.path(), ["add", "--", "."]).expect("stage base");
        command::successful_git_at(repository.path(), ["commit", "-m", "base"])
            .expect("commit base");
        std::fs::write(repository.path().join("included.txt"), "edited included\n")
            .expect("included edit");
        std::fs::write(repository.path().join("staged.txt"), "edited staged\n")
            .expect("staged edit");
        command::successful_git_at(repository.path(), ["add", "--", "staged.txt"])
            .expect("stage external change");

        let hooks = repository.path().join("test-hooks");
        std::fs::create_dir(&hooks).expect("hooks directory");
        let hook = hooks.join("pre-commit");
        std::fs::write(&hook, "#!/bin/sh\necho rejected-by-test >&2\nexit 1\n")
            .expect("pre-commit hook");
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))
            .expect("executable hook");
        command::successful_git_at(
            repository.path(),
            ["config", "core.hooksPath", "test-hooks"],
        )
        .expect("configure hooks");

        let snapshot = working_copy_snapshot(request(repository.path())).expect("snapshot");
        let included = snapshot
            .changes
            .iter()
            .find(|change| change.path.display == "included.txt")
            .map(include_whole)
            .expect("included change");
        let error = commit(CommitRequest {
            repository_path: snapshot.repository_path.clone(),
            worktree_path: snapshot.worktree_path.clone(),
            expected_head: snapshot.head.clone(),
            included_changes: vec![included],
            summary: "rejected commit".into(),
            description: String::new(),
            amend: false,
            author: None,
            co_authors: Vec::new(),
            trailers: Vec::new(),
            signing: CommitSigning::DoNotSign,
        })
        .expect_err("hook rejects commit");

        assert!(error.contains("rejected-by-test"));
        assert_eq!(
            git_text(repository.path(), ["diff", "--cached", "--name-only"]).expect("staged paths"),
            "staged.txt"
        );
        assert_eq!(
            git_text(repository.path(), ["rev-parse", "HEAD"]).expect("head after rejection"),
            snapshot.head.expect("reviewed head")
        );
    }

    #[test]
    fn commits_only_selected_lines_and_leaves_the_rest_in_the_worktree() {
        let repository = repository();
        std::fs::write(repository.path().join("lines.txt"), "one\ntwo\nthree\n")
            .expect("base file");
        command::successful_git_at(repository.path(), ["add", "--", "lines.txt"])
            .expect("stage base");
        command::successful_git_at(repository.path(), ["commit", "-m", "base"])
            .expect("commit base");
        std::fs::write(repository.path().join("lines.txt"), "ONE\ntwo\nTHREE\n")
            .expect("edit file");

        let snapshot = working_copy_snapshot(request(repository.path())).expect("snapshot");
        let change = snapshot
            .changes
            .iter()
            .find(|change| change.path.display == "lines.txt")
            .expect("line change");
        let diff = file_diff(FileDiffRequest {
            repository_path: snapshot.repository_path.clone(),
            worktree_path: snapshot.worktree_path.clone(),
            path: change.path.clone(),
        })
        .expect("file diff");
        let hunk = diff.hunks.first().expect("combined hunk");
        let hunk_offset = hunk.patch.find("@@ ").expect("hunk header");
        let selected_line_indices = hunk.patch[hunk_offset..]
            .lines()
            .skip(1)
            .enumerate()
            .filter_map(|(index, line)| matches!(line, "-one" | "+ONE").then_some(index as u32))
            .collect::<Vec<_>>();
        assert_eq!(selected_line_indices.len(), 2);

        let result = commit(CommitRequest {
            repository_path: snapshot.repository_path.clone(),
            worktree_path: snapshot.worktree_path.clone(),
            expected_head: snapshot.head.clone(),
            included_changes: vec![CommitFileSelection {
                path: change.path.clone(),
                previous_path: change.previous_path.clone(),
                expected_index_status: change.index_status.clone(),
                expected_worktree_status: change.worktree_status.clone(),
                include_all: false,
                hunks: vec![CommitHunkSelection {
                    expected_patch: hunk.patch.clone(),
                    selected_line_indices,
                }],
            }],
            summary: "include selected lines".into(),
            description: String::new(),
            amend: false,
            author: None,
            co_authors: Vec::new(),
            trailers: Vec::new(),
            signing: CommitSigning::DoNotSign,
        })
        .expect("partial commit");

        assert_eq!(
            git_text(repository.path(), ["show", "HEAD:lines.txt"]).expect("committed file"),
            "ONE\ntwo\nthree"
        );
        assert_eq!(
            std::fs::read_to_string(repository.path().join("lines.txt")).expect("working file"),
            "ONE\ntwo\nTHREE\n"
        );
        assert_eq!(result.snapshot.changes.len(), 1);
        assert_eq!(result.snapshot.changes[0].path.display, "lines.txt");
    }

    #[test]
    fn resolves_a_real_merge_conflict_with_a_reviewed_side() {
        let (repository, conflicted, path) = conflicted_repository();
        assert_eq!(conflicted.operation, Some(RepositoryOperation::Merge));
        let resolved = resolve_conflict(ResolveConflictRequest {
            repository_path: conflicted.repository_path,
            worktree_path: conflicted.worktree_path,
            path,
            kind: ConflictResolutionKind::Ours,
            expected_head: conflicted.head,
            expected_content: None,
            manual_content: None,
        })
        .expect("resolve ours");
        assert!(!resolved.changes.iter().any(|change| change.conflicted));
        assert_eq!(
            std::fs::read_to_string(repository.path().join("conflict.txt")).expect("content"),
            "ours\n"
        );
    }

    #[test]
    fn keeps_both_conflict_sides_and_accepts_reviewed_manual_text() {
        let (repository, conflicted, path) = conflicted_repository();
        let reviewed = conflict_file(ConflictFileRequest {
            repository_path: conflicted.repository_path.clone(),
            worktree_path: conflicted.worktree_path.clone(),
            path: path.clone(),
        })
        .expect("review conflict");
        let resolved = resolve_conflict(ResolveConflictRequest {
            repository_path: conflicted.repository_path,
            worktree_path: conflicted.worktree_path,
            path,
            kind: ConflictResolutionKind::Both,
            expected_head: conflicted.head,
            expected_content: Some(reviewed.content),
            manual_content: None,
        })
        .expect("keep both");
        assert!(!resolved.changes.iter().any(|change| change.conflicted));
        let content = std::fs::read_to_string(repository.path().join("conflict.txt"))
            .expect("combined content");
        assert!(content.contains("ours"));
        assert!(content.contains("theirs"));

        let (repository, conflicted, path) = conflicted_repository();
        let reviewed = conflict_file(ConflictFileRequest {
            repository_path: conflicted.repository_path.clone(),
            worktree_path: conflicted.worktree_path.clone(),
            path: path.clone(),
        })
        .expect("review conflict");
        let resolved = resolve_conflict(ResolveConflictRequest {
            repository_path: conflicted.repository_path,
            worktree_path: conflicted.worktree_path,
            path,
            kind: ConflictResolutionKind::Manual,
            expected_head: conflicted.head,
            expected_content: Some(reviewed.content),
            manual_content: Some("carefully resolved\n".into()),
        })
        .expect("manual resolution");
        assert!(!resolved.changes.iter().any(|change| change.conflicted));
        assert_eq!(
            std::fs::read_to_string(repository.path().join("conflict.txt"))
                .expect("manual content"),
            "carefully resolved\n"
        );
    }

    #[test]
    fn undoes_the_latest_local_commit_without_discarding_its_changes() {
        let repository = repository();
        std::fs::write(repository.path().join("file.txt"), "first\n").expect("write first");
        command::successful_git_at(repository.path(), ["add", "file.txt"]).expect("add first");
        command::successful_git_at(repository.path(), ["commit", "-m", "first"])
            .expect("commit first");
        std::fs::write(repository.path().join("file.txt"), "second\n").expect("write second");
        command::successful_git_at(repository.path(), ["add", "file.txt"]).expect("add second");
        command::successful_git_at(
            repository.path(),
            ["commit", "-m", "second", "-m", "details"],
        )
        .expect("commit second");
        let path = repository.path().to_string_lossy().into_owned();
        let before = working_copy_snapshot(WorkingCopyRequest {
            repository_path: path.clone(),
            worktree_path: path.clone(),
        })
        .expect("before");
        let undone = undo_commit(UndoCommitRequest {
            repository_path: path.clone(),
            worktree_path: path,
            expected_head: before.head.expect("head"),
        })
        .expect("undo");
        assert_eq!(undone.summary, "second");
        assert_eq!(undone.description, "details");
        assert!(undone.snapshot.changes.iter().any(|change| change.staged));
        assert_eq!(
            std::fs::read_to_string(repository.path().join("file.txt")).expect("contents"),
            "second\n"
        );
    }

    #[test]
    fn discards_reviewed_untracked_modified_and_renamed_paths_exactly() {
        let repository = repository();
        std::fs::write(repository.path().join("tracked.txt"), "original\n").expect("tracked");
        std::fs::write(repository.path().join("rename-me.txt"), "rename\n").expect("rename");
        command::successful_git_at(repository.path(), ["add", "."]).expect("add");
        command::successful_git_at(repository.path(), ["commit", "-m", "base"]).expect("commit");
        std::fs::write(repository.path().join("tracked.txt"), "changed\n").expect("modify");
        std::fs::write(repository.path().join("untracked.txt"), "temporary\n").expect("untracked");
        command::successful_git_at(repository.path(), ["mv", "rename-me.txt", "renamed.txt"])
            .expect("rename");

        let mut snapshot = working_copy_snapshot(request(repository.path())).expect("snapshot");
        for name in ["tracked.txt", "untracked.txt", "renamed.txt"] {
            let change = snapshot
                .changes
                .iter()
                .find(|change| change.path.display == name)
                .expect("change")
                .clone();
            snapshot = discard_file(DiscardFileRequest {
                repository_path: snapshot.repository_path.clone(),
                worktree_path: snapshot.worktree_path.clone(),
                path: change.path,
                scope: DiscardScope::All,
                expected_head: snapshot.head.clone(),
                expected_index_status: change.index_status,
                expected_worktree_status: change.worktree_status,
            })
            .expect("discard");
        }
        assert!(snapshot.changes.is_empty());
        assert_eq!(
            std::fs::read_to_string(repository.path().join("tracked.txt")).expect("restored"),
            "original\n"
        );
        assert!(repository.path().join("rename-me.txt").is_file());
        assert!(!repository.path().join("renamed.txt").exists());
        assert!(!repository.path().join("untracked.txt").exists());
    }

    #[test]
    fn discard_all_revalidates_every_path_before_restoring_the_repository() {
        let repository = repository();
        std::fs::write(repository.path().join("tracked.txt"), "original\n").expect("tracked");
        command::successful_git_at(repository.path(), ["add", "."]).expect("add");
        command::successful_git_at(repository.path(), ["commit", "-m", "base"]).expect("commit");
        std::fs::write(repository.path().join("tracked.txt"), "changed\n").expect("modify");
        std::fs::write(repository.path().join("untracked.txt"), "temporary\n").expect("untracked");
        let before = working_copy_snapshot(request(repository.path())).expect("snapshot");
        let reviewed = |snapshot: &WorkingCopySnapshot| {
            snapshot
                .changes
                .iter()
                .filter(|change| !change.ignored)
                .map(|change| ReviewedFileChange {
                    path_token: change.path.token.clone(),
                    previous_path_token: change
                        .previous_path
                        .as_ref()
                        .map(|path| path.token.clone()),
                    index_status: change.index_status.clone(),
                    worktree_status: change.worktree_status.clone(),
                })
                .collect()
        };
        let expected = reviewed(&before);
        std::fs::write(repository.path().join("later.txt"), "not reviewed\n").expect("later");
        let stale = discard_all(DiscardAllRequest {
            repository_path: before.repository_path.clone(),
            worktree_path: before.worktree_path.clone(),
            expected_head: before.head.clone(),
            expected_changes: expected,
        })
        .expect_err("stale review");
        assert!(stale.contains("no longer match"));

        let refreshed = working_copy_snapshot(request(repository.path())).expect("refresh");
        let result = discard_all(DiscardAllRequest {
            repository_path: refreshed.repository_path.clone(),
            worktree_path: refreshed.worktree_path.clone(),
            expected_head: refreshed.head.clone(),
            expected_changes: reviewed(&refreshed),
        })
        .expect("discard all");
        assert!(result.changes.is_empty());
        assert_eq!(
            std::fs::read_to_string(repository.path().join("tracked.txt")).expect("restored"),
            "original\n"
        );
        assert!(!repository.path().join("untracked.txt").exists());
        assert!(!repository.path().join("later.txt").exists());
    }

    #[test]
    fn stages_and_unstages_individual_reviewed_hunks() {
        let repository = repository();
        let original = (1..=24)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        std::fs::write(repository.path().join("file.txt"), &original).expect("original");
        command::successful_git_at(repository.path(), ["add", "file.txt"]).expect("add");
        command::successful_git_at(repository.path(), ["commit", "-m", "base"]).expect("commit");
        let changed = original
            .replace("line 2\n", "line two\n")
            .replace("line 22\n", "line twenty-two\n");
        std::fs::write(repository.path().join("file.txt"), changed).expect("changed");
        let snapshot = working_copy_snapshot(request(repository.path())).expect("snapshot");
        let change = snapshot.changes.first().expect("change");
        let diff = file_diff(FileDiffRequest {
            repository_path: snapshot.repository_path.clone(),
            worktree_path: snapshot.worktree_path.clone(),
            path: change.path.clone(),
        })
        .expect("diff");
        assert_eq!(diff.unstaged_hunks.len(), 2);
        let first = diff.unstaged_hunks[0].clone();
        let staged = apply_patch_hunk(ApplyPatchHunkRequest {
            repository_path: snapshot.repository_path.clone(),
            worktree_path: snapshot.worktree_path.clone(),
            path: change.path.clone(),
            action: PatchHunkAction::Stage,
            hunk_index: first.index,
            expected_patch: first.patch,
            expected_head: snapshot.head.clone(),
            selected_line_indices: Vec::new(),
        })
        .expect("stage hunk");
        let staged_change = staged.changes.first().expect("partially staged change");
        assert!(staged_change.staged && staged_change.unstaged);
        let staged_diff = file_diff(FileDiffRequest {
            repository_path: staged.repository_path.clone(),
            worktree_path: staged.worktree_path.clone(),
            path: staged_change.path.clone(),
        })
        .expect("staged diff");
        assert_eq!(staged_diff.staged_hunks.len(), 1);
        assert_eq!(staged_diff.unstaged_hunks.len(), 1);
        let first = staged_diff.staged_hunks[0].clone();
        let unstaged = apply_patch_hunk(ApplyPatchHunkRequest {
            repository_path: staged.repository_path,
            worktree_path: staged.worktree_path,
            path: staged_change.path.clone(),
            action: PatchHunkAction::Unstage,
            hunk_index: first.index,
            expected_patch: first.patch,
            expected_head: staged.head,
            selected_line_indices: Vec::new(),
        })
        .expect("unstage hunk");
        let change = unstaged.changes.first().expect("unstaged change");
        assert!(!change.staged && change.unstaged);
        let discard_diff = file_diff(FileDiffRequest {
            repository_path: unstaged.repository_path.clone(),
            worktree_path: unstaged.worktree_path.clone(),
            path: change.path.clone(),
        })
        .expect("discard diff");
        let first = discard_diff.unstaged_hunks[0].clone();
        apply_patch_hunk(ApplyPatchHunkRequest {
            repository_path: unstaged.repository_path,
            worktree_path: unstaged.worktree_path,
            path: change.path.clone(),
            action: PatchHunkAction::Discard,
            hunk_index: first.index,
            expected_patch: first.patch,
            expected_head: unstaged.head,
            selected_line_indices: Vec::new(),
        })
        .expect("discard hunk");
        let remaining =
            std::fs::read_to_string(repository.path().join("file.txt")).expect("remaining file");
        assert_ne!(
            remaining.contains("line two\n"),
            remaining.contains("line twenty-two\n")
        );
    }

    #[test]
    fn stages_and_discards_only_reviewed_lines_inside_one_hunk() {
        let repository = repository();
        std::fs::write(
            repository.path().join("file.txt"),
            "one\ntwo\nthree\nfour\n",
        )
        .expect("base");
        command::successful_git_at(repository.path(), ["add", "file.txt"]).expect("add");
        command::successful_git_at(repository.path(), ["commit", "-m", "base"]).expect("commit");
        std::fs::write(
            repository.path().join("file.txt"),
            "one\nTWO\nthree\nFOUR\n",
        )
        .expect("changed");
        let snapshot = working_copy_snapshot(request(repository.path())).expect("snapshot");
        let change = snapshot.changes.first().expect("change");
        let diff = file_diff(FileDiffRequest {
            repository_path: snapshot.repository_path.clone(),
            worktree_path: snapshot.worktree_path.clone(),
            path: change.path.clone(),
        })
        .expect("diff");
        assert_eq!(diff.unstaged_hunks.len(), 1);
        let hunk = diff.unstaged_hunks[0].clone();
        let selected: Vec<u32> = hunk_line_indices(&hunk.patch, &["-two", "+TWO"]);
        let staged = apply_patch_hunk(ApplyPatchHunkRequest {
            repository_path: snapshot.repository_path,
            worktree_path: snapshot.worktree_path,
            path: change.path.clone(),
            action: PatchHunkAction::Stage,
            hunk_index: hunk.index,
            expected_patch: hunk.patch,
            expected_head: snapshot.head,
            selected_line_indices: selected,
        })
        .expect("stage selected lines");
        let index = git_text(repository.path(), ["show", ":file.txt"]).expect("index");
        assert_eq!(index, "one\nTWO\nthree\nfour");
        let change = staged.changes.first().expect("partially staged");
        assert!(change.staged && change.unstaged);

        let diff = file_diff(FileDiffRequest {
            repository_path: staged.repository_path.clone(),
            worktree_path: staged.worktree_path.clone(),
            path: change.path.clone(),
        })
        .expect("remaining diff");
        let hunk = diff.unstaged_hunks[0].clone();
        let selected = hunk_line_indices(&hunk.patch, &["-four", "+FOUR"]);
        let discarded = apply_patch_hunk(ApplyPatchHunkRequest {
            repository_path: staged.repository_path,
            worktree_path: staged.worktree_path,
            path: change.path.clone(),
            action: PatchHunkAction::Discard,
            hunk_index: hunk.index,
            expected_patch: hunk.patch,
            expected_head: staged.head,
            selected_line_indices: selected,
        })
        .expect("discard selected lines");
        assert!(discarded
            .changes
            .first()
            .is_some_and(|change| change.staged && !change.unstaged));
        assert_eq!(
            std::fs::read_to_string(repository.path().join("file.txt")).expect("working tree"),
            "one\nTWO\nthree\nfour\n"
        );
    }

    fn hunk_line_indices(patch: &str, lines: &[&str]) -> Vec<u32> {
        let hunk = &patch[patch.find("@@ ").expect("hunk")..];
        let body = &hunk[hunk.find('\n').expect("header newline") + 1..];
        body.split_inclusive('\n')
            .enumerate()
            .filter(|(_, line)| lines.iter().any(|expected| line.trim_end() == *expected))
            .map(|(index, _)| index as u32)
            .collect()
    }

    #[test]
    fn stages_unstages_and_commits_with_exact_path_tokens() {
        let repository = repository();
        std::fs::write(repository.path().join("hello.txt"), "hello\n").expect("write file");
        let snapshot = working_copy_snapshot(request(repository.path())).expect("snapshot");
        let path = snapshot
            .changes
            .iter()
            .find(|change| change.untracked)
            .expect("untracked file")
            .path
            .clone();

        let staged = set_file_staging(SetFileStagingRequest {
            repository_path: snapshot.repository_path.clone(),
            worktree_path: snapshot.worktree_path.clone(),
            paths: vec![path.clone()],
            staged: true,
        })
        .expect("stage file");
        assert!(staged.changes.iter().any(|change| change.staged));

        let unstaged = set_file_staging(SetFileStagingRequest {
            repository_path: staged.repository_path.clone(),
            worktree_path: staged.worktree_path.clone(),
            paths: vec![path.clone()],
            staged: false,
        })
        .expect("unstage unborn file");
        assert!(unstaged.changes.iter().any(|change| change.untracked));

        let staged = set_file_staging(SetFileStagingRequest {
            repository_path: unstaged.repository_path.clone(),
            worktree_path: unstaged.worktree_path.clone(),
            paths: vec![path],
            staged: true,
        })
        .expect("restage file");
        let included_changes = staged.changes.iter().map(include_whole).collect();
        let result = commit(CommitRequest {
            repository_path: staged.repository_path.clone(),
            worktree_path: staged.worktree_path.clone(),
            expected_head: staged.head.clone(),
            included_changes,
            summary: "add hello".into(),
            description: "A body with\nmultiple lines.".into(),
            amend: false,
            author: Some(CommitPerson {
                name: "Alternate Author".into(),
                email: "alternate@example.invalid".into(),
            }),
            co_authors: vec![CommitPerson {
                name: "Pair Programmer".into(),
                email: "pair@example.invalid".into(),
            }],
            trailers: vec![CommitTrailer {
                key: "Reviewed-by".into(),
                value: "Careful Reviewer".into(),
            }],
            signing: CommitSigning::DoNotSign,
        })
        .expect("commit");
        assert_eq!(result.commit.len(), 40);
        assert!(result.snapshot.changes.is_empty());
        let message = git_text(repository.path(), ["log", "-1", "--format=%B"]).expect("message");
        assert_eq!(
            message,
            "add hello\n\nA body with\nmultiple lines.\n\nCo-authored-by: Pair Programmer <pair@example.invalid>\nReviewed-by: Careful Reviewer"
        );
        let author =
            git_text(repository.path(), ["log", "-1", "--format=%an <%ae>"]).expect("author");
        assert_eq!(author, "Alternate Author <alternate@example.invalid>");
    }
}
