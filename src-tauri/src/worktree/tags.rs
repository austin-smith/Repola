use std::path::Path;

use super::command;
use super::models::{
    TagInfo, TagMutationKind, TagMutationRequest, TagMutationResult, TagRequest, WorkingCopyRequest,
};
use super::working_copy::working_copy_snapshot;

pub fn tags(request: TagRequest) -> Result<Vec<TagInfo>, String> {
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path,
        worktree_path: request.worktree_path,
    })?;
    list(Path::new(&snapshot.worktree_path))
}

pub fn mutate_tag(request: TagMutationRequest) -> Result<TagMutationResult, String> {
    validate_tag_name(&request.tag)?;
    let before = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path,
        worktree_path: request.worktree_path,
    })?;
    if before.head != request.expected_head {
        return Err("The working-copy HEAD changed after this tag action was reviewed. Refresh and try again.".into());
    }
    if before.operation.is_some() {
        return Err("Finish or abort the in-progress Git operation before changing tags.".into());
    }
    let worktree = Path::new(&before.worktree_path);
    let existing = list(worktree)?;
    let selected = existing.iter().find(|tag| tag.name == request.tag);
    let output = match request.kind {
        TagMutationKind::CreateLightweight | TagMutationKind::CreateAnnotated => {
            if selected.is_some() {
                return Err(format!("Tag {} already exists.", request.tag));
            }
            let requested_target = request
                .target
                .as_deref()
                .ok_or_else(|| "Choose a commit for the new tag.".to_string())?;
            let target = resolve_commit(worktree, requested_target)?;
            if request.kind == TagMutationKind::CreateAnnotated {
                let message = request
                    .message
                    .as_deref()
                    .map(str::trim)
                    .filter(|message| !message.is_empty())
                    .ok_or_else(|| "An annotated tag requires a message.".to_string())?;
                validate_message(message)?;
                command::git_at(
                    worktree,
                    [
                        "tag",
                        "--annotate",
                        "--message",
                        message,
                        "--",
                        &request.tag,
                        &target,
                    ],
                )
            } else {
                command::git_at(worktree, ["tag", "--", &request.tag, &target])
            }
        }
        TagMutationKind::Delete | TagMutationKind::Push => {
            let selected =
                selected.ok_or_else(|| format!("Tag {} no longer exists.", request.tag))?;
            let expected = request.expected_tag_target.as_deref().ok_or_else(|| {
                "The tag action is missing its reviewed target. Refresh and try again.".to_string()
            })?;
            if selected.target != expected {
                return Err(format!(
                    "Tag {} moved after this action was reviewed. Refresh and try again.",
                    request.tag
                ));
            }
            if request.kind == TagMutationKind::Delete {
                command::git_at(worktree, ["tag", "--delete", "--", &request.tag])
            } else {
                let remote = before.remote.as_deref().ok_or_else(|| {
                    "Configure a remote for the current branch before publishing tags.".to_string()
                })?;
                let refspec = format!("refs/tags/{0}:refs/tags/{0}", request.tag);
                command::git_at(worktree, ["push", remote, &refspec])
            }
        }
    }
    .map_err(|error| error.to_string())?;
    let diagnostic = combined_output(&output.stdout, &output.stderr);
    if !output.status.success() {
        return Err(if diagnostic.is_empty() {
            "Git rejected the tag action without diagnostic output.".into()
        } else {
            diagnostic
        });
    }
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: before.repository_path,
        worktree_path: before.worktree_path,
    })?;
    let tags = list(Path::new(&snapshot.worktree_path))?;
    Ok(TagMutationResult {
        tags,
        snapshot,
        output: diagnostic,
    })
}

fn list(worktree: &Path) -> Result<Vec<TagInfo>, String> {
    let output = command::successful_git_at(
        worktree,
        [
            "for-each-ref",
            "--format=%(refname:strip=2)%00%(objectname)%00%(*objectname)%00%(objecttype)%00%(subject)",
            "refs/tags",
        ],
    )
    .map_err(|error| error.to_string())?;
    let mut tags = Vec::new();
    for line in output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let fields: Vec<_> = line.split(|byte| *byte == 0).collect();
        if fields.len() != 5 {
            return Err("Git returned malformed tag metadata.".into());
        }
        let object = String::from_utf8_lossy(fields[1]).into_owned();
        let peeled = String::from_utf8_lossy(fields[2]).into_owned();
        let annotated = fields[3] == b"tag";
        tags.push(TagInfo {
            name: String::from_utf8_lossy(fields[0]).into_owned(),
            target: if annotated && !peeled.is_empty() {
                peeled
            } else {
                object.clone()
            },
            object,
            annotated,
            subject: String::from_utf8_lossy(fields[4]).into_owned(),
        });
    }
    tags.sort_by_key(|tag| tag.name.to_lowercase());
    Ok(tags)
}

fn validate_tag_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 1024
        || name.starts_with('-')
        || name.chars().any(char::is_control)
    {
        return Err("The tag name is invalid.".into());
    }
    let reference = format!("refs/tags/{name}");
    let output = command::output("git", ["check-ref-format", &reference])
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err("The tag name is not a valid Git reference.".into())
    }
}

fn validate_message(message: &str) -> Result<(), String> {
    if message.len() > 16 * 1024 || message.contains('\0') {
        return Err("The tag message must be 16 KiB or fewer and cannot contain NUL bytes.".into());
    }
    Ok(())
}

fn resolve_commit(worktree: &Path, target: &str) -> Result<String, String> {
    if target.is_empty()
        || target.len() > 1024
        || target.starts_with('-')
        || target.chars().any(char::is_control)
    {
        return Err("The selected tag target is invalid.".into());
    }
    let revision = format!("{target}^{{commit}}");
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
        return Err("The selected tag target no longer exists. Refresh and try again.".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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

    fn repository() -> (tempfile::TempDir, tempfile::TempDir, String) {
        let repository = tempfile::tempdir().expect("repository");
        let remote = tempfile::tempdir().expect("remote");
        command::successful_git_at(repository.path(), ["init", "-b", "main"]).expect("init");
        command::successful_git_at(repository.path(), ["config", "user.name", "Tag Tests"])
            .expect("name");
        command::successful_git_at(
            repository.path(),
            ["config", "user.email", "tags@example.invalid"],
        )
        .expect("email");
        std::fs::write(repository.path().join("file.txt"), "tagged\n").expect("file");
        command::successful_git_at(repository.path(), ["add", "."]).expect("add");
        command::successful_git_at(repository.path(), ["commit", "-m", "tagged commit"])
            .expect("commit");
        command::successful_git_at(remote.path(), ["init", "--bare"]).expect("bare");
        command::successful_git_at(
            repository.path(),
            ["remote", "add", "origin", &remote.path().to_string_lossy()],
        )
        .expect("remote");
        let path = repository.path().to_string_lossy().into_owned();
        (repository, remote, path)
    }

    fn request(
        path: &str,
        kind: TagMutationKind,
        tag: &str,
        target: Option<String>,
        expected_tag_target: Option<String>,
    ) -> TagMutationRequest {
        let snapshot = working_copy_snapshot(WorkingCopyRequest {
            repository_path: path.into(),
            worktree_path: path.into(),
        })
        .expect("snapshot");
        TagMutationRequest {
            repository_path: path.into(),
            worktree_path: path.into(),
            kind,
            tag: tag.into(),
            target,
            message: (kind == TagMutationKind::CreateAnnotated).then(|| "Release notes".into()),
            expected_head: snapshot.head,
            expected_tag_target,
        }
    }

    #[test]
    fn creates_lists_pushes_and_deletes_reviewed_tags() {
        let (_repository, remote, path) = repository();
        let head = working_copy_snapshot(WorkingCopyRequest {
            repository_path: path.clone(),
            worktree_path: path.clone(),
        })
        .expect("snapshot")
        .head
        .expect("head");
        let created = mutate_tag(request(
            &path,
            TagMutationKind::CreateAnnotated,
            "v1.0.0",
            Some(head.clone()),
            None,
        ))
        .expect("create");
        assert_eq!(created.tags.len(), 1);
        assert!(created.tags[0].annotated);
        assert_eq!(created.tags[0].target, head);
        mutate_tag(request(
            &path,
            TagMutationKind::Push,
            "v1.0.0",
            None,
            Some(head.clone()),
        ))
        .expect("push");
        command::successful_git_at(remote.path(), ["show-ref", "--verify", "refs/tags/v1.0.0"])
            .expect("remote tag");
        let deleted = mutate_tag(request(
            &path,
            TagMutationKind::Delete,
            "v1.0.0",
            None,
            Some(head),
        ))
        .expect("delete");
        assert!(deleted.tags.is_empty());
    }
}
