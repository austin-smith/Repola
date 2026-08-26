use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::command;
use super::models::{
    CommitChangedFile, CommitFileDiffRequest, CommitFilesRequest, CommitSignature, CommitSummary,
    FileChangeKind, FileDiff, HistoryPage, HistoryRequest, ReflogEntry, ReflogRequest,
};
use super::working_copy::{
    empty_tree, git_path, image_preview, path_from_token, truncate_file_patch,
};

const MAX_HISTORY_PAGE: u16 = 100;
const MAX_REFLOG_PAGE: u16 = 500;

pub fn reflog(request: ReflogRequest) -> Result<Vec<ReflogEntry>, String> {
    if request.limit == 0 || request.limit > MAX_REFLOG_PAGE {
        return Err(format!(
            "Reflog page size must be between 1 and {MAX_REFLOG_PAGE}."
        ));
    }
    let worktree = canonical_directory(&request.worktree_path)?;
    validate_repository(&request.repository_path, &worktree)?;
    let max_count = format!("--max-count={}", request.limit);
    let output = command::git_at(
        &worktree,
        [
            "reflog",
            "show",
            "--date=iso-strict",
            "--format=%H%x00%gD%x00%gs%x00%cI",
            &max_count,
            "HEAD",
        ],
    )
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if detail.contains("does not have any commits yet") || detail.contains("unknown revision") {
            return Ok(Vec::new());
        }
        return Err(detail);
    }
    parse_reflog(&output.stdout)
}

fn parse_reflog(bytes: &[u8]) -> Result<Vec<ReflogEntry>, String> {
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            let fields: Vec<_> = line.split(|byte| *byte == 0).collect();
            if fields.len() != 4 {
                return Err("Git returned malformed reflog metadata.".to_string());
            }
            Ok(ReflogEntry {
                oid: String::from_utf8_lossy(fields[0]).into_owned(),
                selector: String::from_utf8_lossy(fields[1]).into_owned(),
                subject: String::from_utf8_lossy(fields[2]).into_owned(),
                committed_at: String::from_utf8_lossy(fields[3]).into_owned(),
            })
        })
        .collect()
}

pub fn history(request: HistoryRequest) -> Result<HistoryPage, String> {
    if request.limit == 0 || request.limit > MAX_HISTORY_PAGE {
        return Err(format!(
            "History page size must be between 1 and {MAX_HISTORY_PAGE}."
        ));
    }
    let worktree = canonical_directory(&request.worktree_path)?;
    validate_repository(&request.repository_path, &worktree)?;
    if let Some(cursor) = &request.cursor {
        validate_oid(cursor)?;
    }
    let query = request
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty());
    if let Some(query) = query {
        if query.len() > 200 || query.chars().any(char::is_control) {
            return Err("The history search must be 200 characters or fewer and cannot contain control characters.".into());
        }
    }
    let comparison_base = request
        .comparison_base
        .as_deref()
        .map(str::trim)
        .filter(|reference| !reference.is_empty())
        .map(|reference| resolve_commit(&worktree, reference, "comparison branch"))
        .transpose()?;
    let mut args = vec![
        "log".to_string(),
        "--no-show-signature".to_string(),
        "--date=iso-strict".to_string(),
        "--format=%H%x00%P%x00%an%x00%ae%x00%aI%x00%cI%x00%G?%x00%s%x00%b%x00".to_string(),
        "-z".to_string(),
        format!("--max-count={}", request.limit),
    ];
    if let Some(query) = query {
        args.push("--regexp-ignore-case".into());
        args.push("--fixed-strings".into());
        args.push(format!("--grep={query}"));
    }
    if request.cursor.is_some() {
        args.push("--skip=1".to_string());
    }
    args.push("--end-of-options".into());
    args.push(request.cursor.unwrap_or_else(|| "HEAD".into()));
    if let Some(base) = comparison_base {
        args.push(format!("^{base}"));
    }
    let output = command::git_at(&worktree, &args).map_err(|error| error.to_string())?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if detail.contains("does not have any commits yet") || detail.contains("unknown revision") {
            return Ok(HistoryPage {
                commits: Vec::new(),
                next_cursor: None,
            });
        }
        return Err(detail);
    }
    let commits = parse_log(&output.stdout)?;
    let next_cursor = (commits.len() == usize::from(request.limit))
        .then(|| commits.last().map(|commit| commit.oid.clone()))
        .flatten();
    Ok(HistoryPage {
        commits,
        next_cursor,
    })
}

fn resolve_commit(worktree: &Path, reference: &str, label: &str) -> Result<String, String> {
    if reference.len() > 1024
        || reference.is_empty()
        || reference.starts_with('-')
        || reference.chars().any(char::is_control)
    {
        return Err(format!("The selected {label} is invalid."));
    }
    let revision = format!("{reference}^{{commit}}");
    let output = command::git_at(
        worktree,
        [
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &revision,
        ],
    )
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "The selected {label} no longer exists. Refresh branches and try again."
        ));
    }
    let oid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    validate_oid(&oid)?;
    Ok(oid)
}

pub fn commit_files(request: CommitFilesRequest) -> Result<Vec<CommitChangedFile>, String> {
    let worktree = canonical_directory(&request.worktree_path)?;
    validate_repository(&request.repository_path, &worktree)?;
    let base = commit_base(&worktree, &request.commit)?;
    let output = command::successful_git_at(
        &worktree,
        [
            "diff",
            "--name-status",
            "-z",
            "--find-renames",
            &base,
            &request.commit,
            "--",
        ],
    )
    .map_err(|error| error.to_string())?;
    parse_changed_files(&output.stdout)
}

pub fn commit_file_diff(request: CommitFileDiffRequest) -> Result<FileDiff, String> {
    let files = commit_files(CommitFilesRequest {
        repository_path: request.repository_path.clone(),
        worktree_path: request.worktree_path.clone(),
        commit: request.commit.clone(),
    })?;
    let file = files
        .iter()
        .find(|file| file.path.token == request.path.token)
        .ok_or_else(|| {
            "The selected file is not part of this commit. Refresh and try again.".to_string()
        })?;
    let worktree = canonical_directory(&request.worktree_path)?;
    let base = commit_base(&worktree, &request.commit)?;
    let mut paths = Vec::new();
    if let Some(previous) = file.previous_path.as_ref() {
        paths.push(path_from_token(&previous.token)?);
    }
    paths.push(path_from_token(&file.path.token)?);
    let mut patch_args = vec![
        OsString::from("diff"),
        OsString::from("--no-color"),
        OsString::from("--no-ext-diff"),
        OsString::from("--find-renames"),
        OsString::from(&base),
        OsString::from(&request.commit),
        OsString::from("--"),
    ];
    patch_args.extend(paths.iter().cloned());
    let mut numstat_args = vec![
        OsString::from("diff"),
        OsString::from("--numstat"),
        OsString::from(&base),
        OsString::from(&request.commit),
        OsString::from("--"),
    ];
    numstat_args.extend(paths);
    let patch =
        command::successful_git_at(&worktree, patch_args).map_err(|error| error.to_string())?;
    let numstat =
        command::successful_git_at(&worktree, numstat_args).map_err(|error| error.to_string())?;
    let binary = numstat
        .stdout
        .split(|byte| *byte == b'\n')
        .any(|line| line.starts_with(b"-\t-\t"));
    let submodule = patch
        .stdout
        .windows(b"Subproject commit ".len())
        .any(|window| window == b"Subproject commit ");
    let image = if binary && !submodule {
        let (revision, path) = if file.kind == FileChangeKind::Deleted {
            (
                base.as_str(),
                file.previous_path.as_ref().unwrap_or(&file.path),
            )
        } else {
            (request.commit.as_str(), &file.path)
        };
        commit_image_preview(
            &worktree,
            revision,
            path,
            if file.kind == FileChangeKind::Deleted {
                "Previous commit"
            } else {
                "Selected commit"
            },
        )?
    } else {
        None
    };
    let (patch, truncated) = truncate_file_patch(&patch.stdout);
    Ok(FileDiff {
        patch,
        truncated,
        binary,
        submodule,
        image,
        hunks: Vec::new(),
        staged_hunks: Vec::new(),
        unstaged_hunks: Vec::new(),
    })
}

fn commit_image_preview(
    worktree: &Path,
    revision: &str,
    path: &super::models::GitPath,
    label: &str,
) -> Result<Option<super::models::ImagePreview>, String> {
    let mut object = OsString::from(revision);
    object.push(":");
    object.push(path_from_token(&path.token)?);
    let size = command::git_at(
        worktree,
        [
            OsString::from("cat-file"),
            OsString::from("-s"),
            object.clone(),
        ],
    )
    .map_err(|error| error.to_string())?;
    if !size.status.success() {
        return Ok(None);
    }
    let size = String::from_utf8_lossy(&size.stdout)
        .trim()
        .parse::<u64>()
        .map_err(|_| "Git returned an invalid image blob size.".to_string())?;
    if size > 4 * 1024 * 1024 {
        return Ok(None);
    }
    let blob = command::git_at(
        worktree,
        [OsString::from("cat-file"), OsString::from("blob"), object],
    )
    .map_err(|error| error.to_string())?;
    if !blob.status.success() {
        return Ok(None);
    }
    image_preview(&blob.stdout, label)
}

fn commit_base(worktree: &Path, commit: &str) -> Result<String, String> {
    if commit.len() < 40
        || commit.len() > 64
        || !commit.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("The selected commit ID is invalid.".into());
    }
    let revision = format!("{commit}^{{commit}}");
    let verify = command::git_at(
        worktree,
        [
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &revision,
        ],
    )
    .map_err(|error| error.to_string())?;
    if !verify.status.success() {
        return Err("The selected commit no longer exists in this repository.".into());
    }
    let parents =
        command::successful_git_at(worktree, ["rev-list", "--parents", "-n", "1", commit])
            .map_err(|error| error.to_string())?;
    let parents = String::from_utf8_lossy(&parents.stdout);
    match parents.split_whitespace().nth(1) {
        Some(parent) => Ok(parent.to_string()),
        None => empty_tree(worktree),
    }
}

fn parse_changed_files(bytes: &[u8]) -> Result<Vec<CommitChangedFile>, String> {
    let fields: Vec<&[u8]> = bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect();
    let mut files = Vec::new();
    let mut index = 0;
    while index < fields.len() {
        let status = String::from_utf8_lossy(fields[index]).into_owned();
        index += 1;
        let code = status.as_bytes().first().copied().unwrap_or(b'?');
        let (previous, current) = if matches!(code, b'R' | b'C') {
            let previous = fields
                .get(index)
                .ok_or_else(|| "Git omitted a previous commit path.".to_string())?;
            let current = fields
                .get(index + 1)
                .ok_or_else(|| "Git omitted a renamed commit path.".to_string())?;
            index += 2;
            (Some(git_path(previous)), git_path(current))
        } else {
            let current = fields
                .get(index)
                .ok_or_else(|| "Git omitted a changed commit path.".to_string())?;
            index += 1;
            (None, git_path(current))
        };
        let kind = match code {
            b'A' => FileChangeKind::Added,
            b'M' => FileChangeKind::Modified,
            b'D' => FileChangeKind::Deleted,
            b'R' => FileChangeKind::Renamed,
            b'C' => FileChangeKind::Copied,
            b'T' => FileChangeKind::TypeChanged,
            b'U' => FileChangeKind::Unmerged,
            _ => FileChangeKind::Unknown,
        };
        files.push(CommitChangedFile {
            id: current.token.clone(),
            path: current,
            previous_path: previous,
            kind,
            status,
        });
    }
    Ok(files)
}

fn canonical_directory(value: &str) -> Result<PathBuf, String> {
    let path = dunce::canonicalize(value)
        .map_err(|error| format!("The working-copy path could not be opened: {error}"))?;
    if !path.is_dir() {
        return Err("The working-copy path is not a directory.".into());
    }
    Ok(path)
}

fn validate_repository(repository_path: &str, worktree: &Path) -> Result<(), String> {
    let repository = canonical_directory(repository_path)?;
    if Path::new(&common_dir(worktree)?) != Path::new(&common_dir(&repository)?) {
        return Err("The selected worktree does not belong to the selected repository.".into());
    }
    Ok(())
}

fn common_dir(path: &Path) -> Result<String, String> {
    let output = command::successful_git_at(
        path,
        ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .map_err(|error| error.to_string())?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn validate_oid(value: &str) -> Result<(), String> {
    if value.len() < 4 || value.len() > 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("The history cursor is invalid.".into());
    }
    Ok(())
}

fn parse_log(bytes: &[u8]) -> Result<Vec<CommitSummary>, String> {
    let fields: Vec<&[u8]> = bytes.split(|byte| *byte == 0).collect();
    let mut commits = Vec::new();
    let mut index = 0;
    while index < fields.len() {
        while index < fields.len() && fields[index].is_empty() {
            index += 1;
        }
        if index >= fields.len() {
            break;
        }
        if fields.len() - index < 9 {
            return Err("Git returned a truncated history record.".into());
        }
        let signature = match fields[index + 6].first().copied() {
            Some(b'G') => CommitSignature::Good,
            Some(b'B') => CommitSignature::Bad,
            Some(b'U') => CommitSignature::UnknownValidity,
            Some(b'X') => CommitSignature::Expired,
            Some(b'Y') => CommitSignature::ExpiredKey,
            Some(b'R') => CommitSignature::RevokedKey,
            Some(b'E') => CommitSignature::MissingKey,
            Some(b'N') => CommitSignature::Unsigned,
            Some(b'?') => CommitSignature::Error,
            _ => CommitSignature::Unknown,
        };
        commits.push(CommitSummary {
            oid: text(fields[index]),
            parents: text(fields[index + 1])
                .split_whitespace()
                .map(str::to_string)
                .collect(),
            author_name: text(fields[index + 2]),
            author_email: text(fields[index + 3]),
            authored_at: text(fields[index + 4]),
            committed_at: text(fields[index + 5]),
            signature,
            subject: text(fields[index + 7]),
            body: text(fields[index + 8]),
        });
        index += 9;
    }
    Ok(commits)
}

fn text(value: &[u8]) -> String {
    String::from_utf8_lossy(value).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiple_nul_delimited_commits() {
        let input = b"aaaaaaaa\x00bbbb cccc\x00Ada\x00ada@example.com\x002026-01-01T00:00:00Z\x002026-01-01T00:00:01Z\x00G\x00subject\x00body\x00\x00dddddddd\x00\x00Lin\x00lin@example.com\x002026-01-02T00:00:00Z\x002026-01-02T00:00:00Z\x00N\x00root\x00\x00\x00";
        let commits = parse_log(input).expect("history");
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].parents, ["bbbb", "cccc"]);
        assert_eq!(commits[0].signature, CommitSignature::Good);
        assert_eq!(commits[1].signature, CommitSignature::Unsigned);
    }

    #[test]
    fn rejects_non_oid_cursors() {
        assert!(validate_oid("HEAD~1").is_err());
        assert!(validate_oid("abc123").is_ok());
    }

    #[test]
    fn parses_reflog_records_without_losing_selectors_or_subjects() {
        let entries = parse_reflog(
            b"aaaaaaaa\0HEAD@{0}\0reset: moving to HEAD~1\x002026-01-01T00:00:00Z\nbbbbbbbb\0HEAD@{1}\0commit: useful work\x002026-01-01T00:00:01Z\n",
        )
        .expect("reflog");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].selector, "HEAD@{0}");
        assert_eq!(entries[1].subject, "commit: useful work");
    }

    #[test]
    fn pages_real_history_without_repeating_the_cursor() {
        let repository = tempfile::tempdir().expect("repository");
        command::successful_git_at(repository.path(), ["init"]).expect("init");
        command::successful_git_at(repository.path(), ["config", "user.name", "History Test"])
            .expect("name");
        command::successful_git_at(
            repository.path(),
            ["config", "user.email", "history@example.invalid"],
        )
        .expect("email");
        for index in 1..=3 {
            std::fs::write(repository.path().join("file.txt"), index.to_string()).expect("write");
            command::successful_git_at(repository.path(), ["add", "--", "file.txt"])
                .expect("stage");
            command::successful_git_at(
                repository.path(),
                ["commit", "-m", &format!("commit {index}")],
            )
            .expect("commit");
        }
        let path = repository.path().to_string_lossy().into_owned();
        let first = history(HistoryRequest {
            repository_path: path.clone(),
            worktree_path: path.clone(),
            cursor: None,
            query: None,
            comparison_base: None,
            limit: 2,
        })
        .expect("first page");
        assert_eq!(first.commits.len(), 2);
        assert_eq!(first.commits[0].subject, "commit 3");
        let cursor = first.next_cursor.expect("next cursor");
        let second = history(HistoryRequest {
            repository_path: path.clone(),
            worktree_path: path.clone(),
            cursor: Some(cursor.clone()),
            query: None,
            comparison_base: None,
            limit: 2,
        })
        .expect("second page");
        assert_eq!(second.commits.len(), 1);
        assert_eq!(second.commits[0].subject, "commit 1");
        assert_ne!(second.commits[0].oid, cursor);
        assert!(second.next_cursor.is_none());

        let entries = reflog(ReflogRequest {
            repository_path: path.clone(),
            worktree_path: path.clone(),
            limit: 20,
        })
        .expect("reflog");
        assert!(entries.len() >= 3);
        assert_eq!(entries[0].oid, first.commits[0].oid);
        assert!(entries
            .iter()
            .any(|entry| entry.subject.contains("commit 1")));

        let newest = first.commits[0].oid.clone();
        let files = commit_files(CommitFilesRequest {
            repository_path: path.clone(),
            worktree_path: path.clone(),
            commit: newest.clone(),
        })
        .expect("commit files");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path.display, "file.txt");
        let diff = commit_file_diff(CommitFileDiffRequest {
            repository_path: path.clone(),
            worktree_path: path,
            commit: newest,
            path: files[0].path.clone(),
        })
        .expect("commit file diff");
        assert!(diff.patch.contains("+3"));
        assert!(!diff.binary);
    }

    #[test]
    fn searches_messages_and_compares_against_an_exact_branch() {
        let repository = tempfile::tempdir().expect("repository");
        command::successful_git_at(repository.path(), ["init", "-b", "main"]).expect("init");
        command::successful_git_at(repository.path(), ["config", "user.name", "History Test"])
            .expect("name");
        command::successful_git_at(
            repository.path(),
            ["config", "user.email", "history@example.invalid"],
        )
        .expect("email");
        std::fs::write(repository.path().join("file.txt"), "base").expect("write base");
        command::successful_git_at(repository.path(), ["add", "--", "file.txt"]).expect("add");
        command::successful_git_at(repository.path(), ["commit", "-m", "shared foundation"])
            .expect("base commit");
        command::successful_git_at(repository.path(), ["switch", "-c", "feature"])
            .expect("feature");
        std::fs::write(repository.path().join("file.txt"), "feature").expect("write feature");
        command::successful_git_at(repository.path(), ["commit", "-am", "Polish Remote Flow"])
            .expect("feature commit");

        let path = repository.path().to_string_lossy().into_owned();
        let searched = history(HistoryRequest {
            repository_path: path.clone(),
            worktree_path: path.clone(),
            cursor: None,
            query: Some("remote".into()),
            comparison_base: None,
            limit: 50,
        })
        .expect("searched history");
        assert_eq!(searched.commits.len(), 1);
        assert_eq!(searched.commits[0].subject, "Polish Remote Flow");

        let compared = history(HistoryRequest {
            repository_path: path.clone(),
            worktree_path: path,
            cursor: None,
            query: None,
            comparison_base: Some("refs/heads/main".into()),
            limit: 50,
        })
        .expect("compared history");
        assert_eq!(compared.commits.len(), 1);
        assert_eq!(compared.commits[0].subject, "Polish Remote Flow");
    }
}
