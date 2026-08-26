use std::path::Path;

use serde::Deserialize;

use super::command;
use super::discovery::repository_context;
use super::models::{
    IssueSummary, PullRequestEvidence, PullRequestFetchStatus, PullRequestMutationKind,
    PullRequestMutationRequest, PullRequestMutationResult, PullRequestSummary, RemoteProvider,
    WorkingCopyRequest,
};
use super::working_copy::working_copy_snapshot;

pub fn fetch_pull_requests(
    repository_path: &str,
    branch: &str,
) -> Result<PullRequestEvidence, String> {
    let repository = repository_context(Path::new(repository_path))?;
    let evidence = |status, pulls, detail| PullRequestEvidence {
        status,
        provider: repository.provider,
        branch: branch.to_string(),
        pulls,
        detail,
    };

    let (program, args): (&str, Vec<String>) = match repository.provider {
        RemoteProvider::GitHub => (
            "gh",
            vec![
                "pr".into(),
                "list".into(),
                "--head".into(),
                branch.into(),
                "--state".into(),
                "all".into(),
                "--json".into(),
                "number,title,state,url,isDraft,headRefName,baseRefName,reviewDecision,mergeStateStatus,statusCheckRollup,closingIssuesReferences".into(),
                "--limit".into(),
                "20".into(),
            ],
        ),
        RemoteProvider::AzureDevOps => (
            "az",
            vec![
                "repos".into(),
                "pr".into(),
                "list".into(),
                "--detect".into(),
                "true".into(),
                "--source-branch".into(),
                branch.into(),
                "--status".into(),
                "all".into(),
                "--output".into(),
                "json".into(),
            ],
        ),
        RemoteProvider::Other | RemoteProvider::None => {
            return Ok(evidence(
                PullRequestFetchStatus::Unsupported,
                Vec::new(),
                Some("Pull-request lookup supports GitHub and Azure DevOps remotes.".to_string()),
            ));
        }
    };

    let output = match command::output_at(&repository.path, program, &args) {
        Ok(output) => output,
        Err(command::CommandError::Launch { .. }) => {
            return Ok(evidence(
                PullRequestFetchStatus::CliMissing,
                Vec::new(),
                Some(format!(
                    "The {program} CLI is not installed, so pull-request state cannot be verified."
                )),
            ));
        }
        Err(error) => return Err(error.to_string()),
    };
    if !output.status.success() {
        let detail: String = String::from_utf8_lossy(&output.stderr)
            .trim()
            .chars()
            .take(300)
            .collect();
        return Ok(evidence(
            if authentication_error(&detail) {
                PullRequestFetchStatus::NotAuthenticated
            } else {
                PullRequestFetchStatus::CliError
            },
            Vec::new(),
            Some(detail),
        ));
    }

    let pulls = match repository.provider {
        RemoteProvider::GitHub => parse_github(&output.stdout)?,
        RemoteProvider::AzureDevOps => {
            parse_azure(&output.stdout, repository.remote_url.as_deref())?
        }
        RemoteProvider::Other | RemoteProvider::None => unreachable!(),
    };
    Ok(evidence(PullRequestFetchStatus::Fetched, pulls, None))
}

pub fn mutate_pull_request(
    request: PullRequestMutationRequest,
) -> Result<PullRequestMutationResult, String> {
    let snapshot = working_copy_snapshot(WorkingCopyRequest {
        repository_path: request.repository_path.clone(),
        worktree_path: request.worktree_path.clone(),
    })?;
    if snapshot.head != request.expected_head {
        return Err(
            "The working-copy HEAD changed after the pull-request action was reviewed.".into(),
        );
    }
    if snapshot.branch.as_deref() != Some(request.branch.as_str()) {
        return Err(
            "The checked-out branch changed after the pull-request action was reviewed.".into(),
        );
    }
    if snapshot.operation.is_some() {
        return Err(
            "Finish or abort the in-progress Git operation before changing pull-request state."
                .into(),
        );
    }
    let repository = repository_context(Path::new(&snapshot.repository_path))?;
    if !matches!(
        repository.provider,
        RemoteProvider::GitHub | RemoteProvider::AzureDevOps
    ) {
        return Err("Pull-request mutations support GitHub and Azure DevOps remotes.".into());
    }

    match request.kind {
        PullRequestMutationKind::Create => {
            create_pull_request(request, snapshot, repository.provider)
        }
        PullRequestMutationKind::Checkout => {
            checkout_pull_request(request, snapshot, repository.provider)
        }
    }
}

fn create_pull_request(
    request: PullRequestMutationRequest,
    snapshot: super::models::WorkingCopySnapshot,
    provider: RemoteProvider,
) -> Result<PullRequestMutationResult, String> {
    let title = request.title.as_deref().map(str::trim).unwrap_or("");
    if title.is_empty() || title.chars().count() > 1024 {
        return Err("Enter a pull-request title between 1 and 1,024 characters.".into());
    }
    let body = request.body.as_deref().unwrap_or("");
    if body.len() > 1_048_576 {
        return Err(
            "The pull-request description is larger than Repola's 1 MiB safety limit.".into(),
        );
    }
    if let Some(base) = request
        .base_branch
        .as_deref()
        .filter(|base| !base.is_empty())
    {
        super::branches::validate_branch_name(base)?;
    }

    let (program, mut args) = match provider {
        RemoteProvider::GitHub => (
            "gh",
            vec![
                "pr".to_string(),
                "create".into(),
                "--head".into(),
                request.branch.clone(),
                "--title".into(),
                title.to_string(),
                "--body".into(),
                body.to_string(),
            ],
        ),
        RemoteProvider::AzureDevOps => (
            "az",
            vec![
                "repos".to_string(),
                "pr".into(),
                "create".into(),
                "--detect".into(),
                "true".into(),
                "--source-branch".into(),
                request.branch.clone(),
                "--title".into(),
                title.to_string(),
                "--description".into(),
                body.to_string(),
                "--output".into(),
                "json".into(),
            ],
        ),
        RemoteProvider::Other | RemoteProvider::None => unreachable!(),
    };
    if let Some(base) = request
        .base_branch
        .as_deref()
        .filter(|base| !base.is_empty())
    {
        args.extend([
            if provider == RemoteProvider::GitHub {
                "--base"
            } else {
                "--target-branch"
            }
            .into(),
            base.into(),
        ]);
    }
    if request.draft {
        args.extend(if provider == RemoteProvider::GitHub {
            vec!["--draft".into()]
        } else {
            vec!["--draft".into(), "true".into()]
        });
    }
    let output = run_provider_command(&snapshot.worktree_path, program, &args)?;
    let evidence = fetch_pull_requests(&snapshot.repository_path, &request.branch)?;
    let created_url = evidence
        .pulls
        .iter()
        .find(|pull| {
            pull.head_branch == request.branch && matches!(pull.state.as_str(), "open" | "draft")
        })
        .and_then(|pull| pull.url.clone())
        .or_else(|| {
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout
                .split_whitespace()
                .find(|value| value.starts_with("https://"))
                .map(str::to_string)
        });
    Ok(PullRequestMutationResult {
        evidence,
        snapshot: None,
        created_url,
        output: combined_output(&output),
    })
}

fn checkout_pull_request(
    request: PullRequestMutationRequest,
    snapshot: super::models::WorkingCopySnapshot,
    provider: RemoteProvider,
) -> Result<PullRequestMutationResult, String> {
    if snapshot.changes.iter().any(|change| !change.ignored) {
        return Err("Commit or stash local changes before checking out a pull request.".into());
    }
    let expected = request
        .expected_pull
        .as_ref()
        .ok_or_else(|| "The reviewed pull request is missing.".to_string())?;
    let evidence = fetch_pull_requests(&snapshot.repository_path, &request.branch)?;
    if evidence.status != PullRequestFetchStatus::Fetched {
        return Err(evidence
            .detail
            .unwrap_or_else(|| "The pull request could not be revalidated.".into()));
    }
    let current = evidence
        .pulls
        .iter()
        .find(|pull| pull.number == expected.number);
    if current != Some(expected) {
        return Err(
            "The pull request changed after it was reviewed. Refresh it before checking it out."
                .into(),
        );
    }
    let number = expected.number.to_string();
    let (program, args) = match provider {
        RemoteProvider::GitHub => ("gh", vec!["pr", "checkout", number.as_str()]),
        RemoteProvider::AzureDevOps => (
            "az",
            vec![
                "repos",
                "pr",
                "checkout",
                "--id",
                number.as_str(),
                "--detect",
                "true",
            ],
        ),
        RemoteProvider::Other | RemoteProvider::None => unreachable!(),
    };
    let args: Vec<String> = args.into_iter().map(str::to_string).collect();
    let output = run_provider_command(&snapshot.worktree_path, program, &args)?;
    let updated = working_copy_snapshot(WorkingCopyRequest {
        repository_path: snapshot.repository_path,
        worktree_path: snapshot.worktree_path,
    })?;
    Ok(PullRequestMutationResult {
        evidence,
        snapshot: Some(updated),
        created_url: None,
        output: combined_output(&output),
    })
}

fn run_provider_command(
    path: &str,
    program: &str,
    args: &[String],
) -> Result<std::process::Output, String> {
    let output =
        command::output_at(Path::new(path), program, args).map_err(|error| match error {
            command::CommandError::Launch { .. } => {
                format!("The {program} CLI is not installed on this machine.")
            }
            other => other.to_string(),
        })?;
    if output.status.success() {
        Ok(output)
    } else {
        let detail = combined_output(&output);
        if authentication_error(&detail) {
            Err(format!("Provider authentication is required. {detail}"))
        } else {
            Err(detail)
        }
    }
}

fn combined_output(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    match (stdout.is_empty(), stderr.is_empty()) {
        (false, false) => format!("{stdout}\n{stderr}"),
        (false, true) => stdout,
        (true, false) => stderr,
        (true, true) => "The provider CLI completed without output.".into(),
    }
}

fn authentication_error(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    [
        "not logged in",
        "authentication required",
        "authenticate first",
        "run:  gh auth login",
        "gh auth login",
        "az login",
        "please login",
        "unauthorized",
        "http 401",
    ]
    .iter()
    .any(|needle| detail.contains(needle))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GitHubPull {
    number: u64,
    title: String,
    state: String,
    url: Option<String>,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    head_ref_name: String,
    #[serde(default)]
    base_ref_name: String,
    #[serde(default)]
    review_decision: String,
    #[serde(default)]
    merge_state_status: String,
    #[serde(default)]
    status_check_rollup: Vec<serde_json::Value>,
    #[serde(default)]
    closing_issues_references: Vec<GitHubIssue>,
}

#[derive(Deserialize)]
struct GitHubIssue {
    number: u64,
    title: String,
    url: Option<String>,
}

fn parse_github(bytes: &[u8]) -> Result<Vec<PullRequestSummary>, String> {
    let pulls: Vec<GitHubPull> = serde_json::from_slice(bytes)
        .map_err(|error| format!("Could not parse the gh CLI response: {error}"))?;
    Ok(pulls
        .into_iter()
        .map(|pull| {
            let (checks_passed, checks_pending, checks_failed) =
                check_counts(&pull.status_check_rollup);
            PullRequestSummary {
                number: pull.number,
                title: pull.title,
                state: if pull.is_draft && pull.state.eq_ignore_ascii_case("open") {
                    "draft".to_string()
                } else {
                    pull.state.to_ascii_lowercase()
                },
                url: pull.url,
                head_branch: pull.head_ref_name,
                base_branch: pull.base_ref_name,
                review_state: pull.review_decision.to_ascii_lowercase(),
                merge_state: pull.merge_state_status.to_ascii_lowercase(),
                checks_total: pull.status_check_rollup.len().min(u32::MAX as usize) as u32,
                checks_passed,
                checks_pending,
                checks_failed,
                linked_issues: pull
                    .closing_issues_references
                    .into_iter()
                    .map(|issue| IssueSummary {
                        number: issue.number,
                        title: issue.title,
                        url: issue.url,
                    })
                    .collect(),
            }
        })
        .collect())
}

fn check_counts(checks: &[serde_json::Value]) -> (u32, u32, u32) {
    let mut passed = 0_u32;
    let mut pending = 0_u32;
    let mut failed = 0_u32;
    for check in checks {
        let conclusion = check
            .get("conclusion")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        let status = check
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(
            conclusion.as_str(),
            "failure" | "cancelled" | "timed_out" | "action_required" | "startup_failure"
        ) {
            failed += 1;
        } else if conclusion.is_empty() && !matches!(status.as_str(), "completed" | "success") {
            pending += 1;
        } else {
            passed += 1;
        }
    }
    (passed, pending, failed)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AzurePull {
    pull_request_id: u64,
    title: String,
    status: String,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    source_ref_name: String,
    #[serde(default)]
    target_ref_name: String,
    #[serde(default)]
    reviewers: Vec<AzureReviewer>,
}

#[derive(Deserialize)]
struct AzureReviewer {
    #[serde(default)]
    vote: i16,
}

fn parse_azure(bytes: &[u8], remote_url: Option<&str>) -> Result<Vec<PullRequestSummary>, String> {
    let pulls: Vec<AzurePull> = serde_json::from_slice(bytes)
        .map_err(|error| format!("Could not parse the az CLI response: {error}"))?;
    Ok(pulls
        .into_iter()
        .map(|pull| PullRequestSummary {
            number: pull.pull_request_id,
            title: pull.title,
            state: match pull.status.as_str() {
                "completed" => "merged".to_string(),
                "active" if pull.is_draft => "draft".to_string(),
                "active" => "open".to_string(),
                "abandoned" => "closed".to_string(),
                other => other.to_string(),
            },
            url: remote_url.and_then(|remote| azure_pull_url(remote, pull.pull_request_id)),
            head_branch: short_ref(&pull.source_ref_name),
            base_branch: short_ref(&pull.target_ref_name),
            review_state: azure_review_state(&pull.reviewers),
            merge_state: pull.status.to_ascii_lowercase(),
            checks_total: 0,
            checks_passed: 0,
            checks_pending: 0,
            checks_failed: 0,
            linked_issues: Vec::new(),
        })
        .collect())
}

fn short_ref(reference: &str) -> String {
    reference
        .strip_prefix("refs/heads/")
        .unwrap_or(reference)
        .to_string()
}

fn azure_review_state(reviewers: &[AzureReviewer]) -> String {
    if reviewers.iter().any(|reviewer| reviewer.vote <= -10) {
        "changes_requested".into()
    } else if !reviewers.is_empty() && reviewers.iter().all(|reviewer| reviewer.vote >= 5) {
        "approved".into()
    } else {
        "review_required".into()
    }
}

fn azure_pull_url(remote: &str, number: u64) -> Option<String> {
    let trimmed = remote.trim_end_matches(".git").trim_end_matches('/');
    if trimmed.starts_with("https://dev.azure.com/") || trimmed.contains(".visualstudio.com/") {
        return Some(format!("{trimmed}/pullrequest/{number}"));
    }
    let path = trimmed.strip_prefix("git@ssh.dev.azure.com:v3/")?;
    let mut parts = path.split('/');
    let organization = parts.next()?;
    let project = parts.next()?;
    let repository = parts.next()?;
    Some(format!(
        "https://dev.azure.com/{organization}/{project}/_git/{repository}/pullrequest/{number}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_adapter_parses_reviews_checks_queue_state_and_linked_issues() {
        let pulls = parse_github(br#"[{
            "number":42,"title":"Ship it","state":"OPEN","url":"https://github.example/pull/42","isDraft":false,
            "headRefName":"feature","baseRefName":"main","reviewDecision":"APPROVED","mergeStateStatus":"QUEUED",
            "statusCheckRollup":[{"status":"COMPLETED","conclusion":"SUCCESS"},{"status":"IN_PROGRESS","conclusion":""},{"status":"COMPLETED","conclusion":"FAILURE"}],
            "closingIssuesReferences":[{"number":7,"title":"Tracked work","url":"https://github.example/issues/7"}]
        }]"#).expect("GitHub pulls");
        let pull = &pulls[0];
        assert_eq!(pull.review_state, "approved");
        assert_eq!(pull.merge_state, "queued");
        assert_eq!(
            (pull.checks_passed, pull.checks_pending, pull.checks_failed),
            (1, 1, 1)
        );
        assert_eq!(pull.linked_issues[0].number, 7);
    }

    #[test]
    fn azure_adapter_parses_reviews_and_builds_browser_urls_for_https_and_ssh() {
        let json = br#"[{
            "pullRequestId":19,"title":"Azure change","status":"active","isDraft":false,
            "sourceRefName":"refs/heads/feature","targetRefName":"refs/heads/main",
            "reviewers":[{"vote":10},{"vote":5}]
        }]"#;
        let pulls = parse_azure(
            json,
            Some("git@ssh.dev.azure.com:v3/acme/project/repository"),
        )
        .expect("Azure pulls");
        assert_eq!(pulls[0].review_state, "approved");
        assert_eq!(pulls[0].head_branch, "feature");
        assert_eq!(
            pulls[0].url.as_deref(),
            Some("https://dev.azure.com/acme/project/_git/repository/pullrequest/19")
        );
        assert_eq!(
            azure_pull_url("https://dev.azure.com/acme/project/_git/repository.git", 20).as_deref(),
            Some("https://dev.azure.com/acme/project/_git/repository/pullrequest/20")
        );
    }

    #[test]
    fn provider_authentication_failures_are_distinct_from_other_cli_errors() {
        assert!(authentication_error(
            "To get started with GitHub CLI, please run: gh auth login"
        ));
        assert!(authentication_error(
            "Please run 'az login' to setup account. HTTP 401"
        ));
        assert!(!authentication_error(
            "GraphQL: Could not resolve to a Repository"
        ));
    }
}
