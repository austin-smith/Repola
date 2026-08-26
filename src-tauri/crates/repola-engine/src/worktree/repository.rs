use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::command;
use super::models::{CloneRepositoryRequest, CreateRepositoryRequest, RepositoryOperationResult};

pub fn clone_repository(
    request: CloneRepositoryRequest,
) -> Result<RepositoryOperationResult, String> {
    validate_source(&request.source)?;
    let destination = validate_destination(&request.destination_path)?;
    let args = [
        OsString::from("clone"),
        OsString::from("--progress"),
        OsString::from("--"),
        OsString::from(request.source),
        destination.as_os_str().to_owned(),
    ];
    let output = command::output("git", args).map_err(|error| error.to_string())?;
    finish_repository_operation(destination, output, "clone")
}

pub fn create_repository(
    request: CreateRepositoryRequest,
) -> Result<RepositoryOperationResult, String> {
    validate_branch_name(&request.initial_branch)?;
    let destination = validate_destination(&request.destination_path)?;
    let args = [
        OsString::from("init"),
        OsString::from("--initial-branch"),
        OsString::from(request.initial_branch),
        OsString::from("--"),
        destination.as_os_str().to_owned(),
    ];
    let output = command::output("git", args).map_err(|error| error.to_string())?;
    finish_repository_operation(destination, output, "initialize")
}

fn validate_source(source: &str) -> Result<(), String> {
    if source.trim() != source
        || source.is_empty()
        || source.len() > 16 * 1024
        || source.contains(['\0', '\n', '\r'])
    {
        return Err("The clone source is empty, malformed, or too long.".into());
    }
    Ok(())
}

fn validate_destination(value: &str) -> Result<PathBuf, String> {
    if value.is_empty() || value.len() > 32 * 1024 || value.contains('\0') {
        return Err("The repository destination is empty, malformed, or too long.".into());
    }
    let destination = PathBuf::from(value);
    if !destination.is_absolute() {
        return Err(
            "The repository destination must be a full path on the selected machine.".into(),
        );
    }
    if destination.exists() {
        if !destination.is_dir() {
            return Err("The repository destination exists and is not a folder.".into());
        }
        if destination
            .read_dir()
            .map_err(|error| format!("Could not inspect the repository destination: {error}"))?
            .next()
            .is_some()
        {
            return Err("The repository destination must not exist or must be empty.".into());
        }
    } else {
        let parent = destination.parent().ok_or_else(|| {
            "The repository destination must have an existing parent folder.".to_string()
        })?;
        if !parent.is_dir() {
            return Err(format!(
                "The destination parent {} does not exist or is not a folder.",
                parent.display()
            ));
        }
    }
    Ok(destination)
}

fn validate_branch_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 1024 || name.contains('\0') {
        return Err("The initial branch name is invalid.".into());
    }
    let output = command::output("git", ["check-ref-format", "--branch", name])
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn finish_repository_operation(
    destination: PathBuf,
    output: std::process::Output,
    verb: &str,
) -> Result<RepositoryOperationResult, String> {
    let diagnostic = combined_output(&output.stdout, &output.stderr);
    if !output.status.success() {
        return Err(if diagnostic.is_empty() {
            format!("Git could not {verb} the repository and returned no diagnostic output.")
        } else {
            diagnostic
        });
    }
    let canonical = dunce::canonicalize(&destination).map_err(|error| {
        format!(
            "Git completed, but Repola could not resolve {}: {error}",
            destination.display()
        )
    })?;
    if !canonical.join(".git").exists() && !is_bare_repository(&canonical) {
        return Err("Git completed without creating a recognizable repository.".into());
    }
    Ok(RepositoryOperationResult {
        repository_path: canonical.to_string_lossy().into_owned(),
        output: diagnostic,
    })
}

fn is_bare_repository(path: &Path) -> bool {
    path.join("HEAD").is_file() && path.join("objects").is_dir() && path.join("refs").is_dir()
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

    #[test]
    fn creates_and_clones_real_repositories() {
        let parent = tempfile::tempdir().expect("parent");
        let source = parent.path().join("source");
        let created = create_repository(CreateRepositoryRequest {
            destination_path: source.to_string_lossy().into_owned(),
            initial_branch: "main".into(),
        })
        .expect("create");
        assert_eq!(
            Path::new(&created.repository_path),
            dunce::canonicalize(&source).expect("canonical source")
        );
        command::successful_git_at(&source, ["config", "user.name", "Repository Test"])
            .expect("name");
        command::successful_git_at(
            &source,
            ["config", "user.email", "repository@example.invalid"],
        )
        .expect("email");
        std::fs::write(source.join("README.md"), "source\n").expect("readme");
        command::successful_git_at(&source, ["add", "--", "README.md"]).expect("stage");
        command::successful_git_at(&source, ["commit", "-m", "initial"]).expect("commit");

        let destination = parent.path().join("clone");
        let cloned = clone_repository(CloneRepositoryRequest {
            source: source.to_string_lossy().into_owned(),
            destination_path: destination.to_string_lossy().into_owned(),
        })
        .expect("clone");
        assert_eq!(
            Path::new(&cloned.repository_path),
            dunce::canonicalize(&destination).expect("canonical clone")
        );
        assert!(destination.join("README.md").is_file());
    }

    #[test]
    fn rejects_nonempty_destinations_before_running_git() {
        let parent = tempfile::tempdir().expect("parent");
        std::fs::write(parent.path().join("occupied"), "content").expect("file");
        let error = create_repository(CreateRepositoryRequest {
            destination_path: parent.path().to_string_lossy().into_owned(),
            initial_branch: "main".into(),
        })
        .expect_err("nonempty destination");
        assert!(error.contains("must not exist or must be empty"));
    }
}
