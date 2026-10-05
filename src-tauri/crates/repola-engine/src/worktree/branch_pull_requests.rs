//! The open pull requests that a remote branch's deletion would affect: those
//! that use the branch as their source, and those that merge into it.
//!
//! GitHub closes both kinds when the branch is deleted, and Azure DevOps leaves
//! them unable to complete, so a branch deletion review lists them. The question
//! goes to the provider hosting the exact URL the deletion pushes to, about that
//! exact branch:
//! - GitHub's `associatedPullRequests` finds pull requests from the branch into
//!   any repository, including from a fork; `pullRequests(baseRefName:)` finds
//!   those into it.
//! - Azure DevOps lists pull requests into the repository from the branch,
//!   leaving out those from a fork's branch of the same name, and those into the
//!   branch. A pull request from the branch into a repository it was forked from
//!   is not found.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use super::command;
use super::models::{BranchPullRequests, OpenPullRequest, PullRequestRelation, RemoteProvider};

/// Asking a provider must not hold up a review for long.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(20);

/// Lists the open pull requests that use `remote_ref` on the repository at
/// `url` as their source or target. Never fails: a provider that cannot be
/// asked is reported as such.
pub(crate) fn branch_pull_requests(
    worktree: &Path,
    url: &str,
    remote_ref: &str,
) -> BranchPullRequests {
    let Some(location) = parse_remote_url(url) else {
        return BranchPullRequests::Unsupported;
    };
    let Some(branch) = remote_ref.strip_prefix("refs/heads/") else {
        return BranchPullRequests::Unsupported;
    };
    let location = RemoteLocation {
        host: match ssh_login(url) {
            Some(login) => ssh_hostname(worktree, &login).unwrap_or(location.host),
            None => location.host,
        },
        ..location
    };
    let gh_host = std::env::var("GH_HOST").ok();
    if let Some(repository) = github_repository(&location, gh_host.as_deref()) {
        let withheld = withheld_github_tokens(&repository.host, gh_host);
        return checked(
            RemoteProvider::GitHub,
            ask(
                worktree,
                "gh",
                &github_arguments(&repository, remote_ref, branch),
                withheld,
            )
            .and_then(|bytes| parse_github(&bytes, &repository)),
        );
    }
    if let Some(repository) = azure_repository(&location) {
        let lookups = [PullRequestRelation::Source, PullRequestRelation::Target].map(|relation| {
            ask(
                worktree,
                "az",
                &azure_arguments(&repository, remote_ref, relation),
                &[],
            )
            .and_then(|bytes| parse_azure(&bytes, &repository, relation))
        });
        return checked(
            RemoteProvider::AzureDevOps,
            lookups
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .map(|lists| Listed {
                    more_than_listed: lists.iter().any(|list| list.more_than_listed),
                    pulls: lists.into_iter().flat_map(|list| list.pulls).collect(),
                }),
        );
    }
    BranchPullRequests::Unsupported
}

/// The pull requests one lookup listed, and whether the provider has more.
struct Listed {
    pulls: Vec<OpenPullRequest>,
    more_than_listed: bool,
}

/// Each lookup lists at most this many pull requests of each kind.
const LISTED: usize = 100;

fn checked(provider: RemoteProvider, listed: Result<Listed, String>) -> BranchPullRequests {
    match listed {
        Ok(listed) => BranchPullRequests::Checked {
            provider,
            pulls: listed.pulls,
            more_than_listed: listed.more_than_listed,
        },
        Err(reason) => BranchPullRequests::Unavailable { reason },
    }
}

/// `gh` sends a GitHub Enterprise token from its environment to whichever
/// enterprise host it is asked about. A push URL can name any host, so that
/// token goes only to the host `GH_HOST` says it is for; other hosts are asked
/// with whatever credentials `gh` stores for them.
fn withheld_github_tokens(host: &str, gh_host: Option<String>) -> &'static [&'static str] {
    let intended = gh_host.is_some_and(|gh_host| gh_host.trim().eq_ignore_ascii_case(host));
    if host == "github.com" || intended {
        &[]
    } else {
        &["GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"]
    }
}

/// Runs a provider CLI without the `withheld` environment variables, and
/// returns what it printed, or why it could not.
fn ask(
    worktree: &Path,
    program: &str,
    arguments: &[String],
    withheld: &[&str],
) -> Result<Vec<u8>, String> {
    let output = command::output_at_with_timeout_without_env(
        worktree,
        program,
        arguments,
        withheld,
        LOOKUP_TIMEOUT,
    )
    .map_err(|error| match error {
        command::CommandError::Launch { .. } => {
            format!("The {program} CLI is not installed on this machine.")
        }
        other => other.to_string(),
    })?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    let detail: String = String::from_utf8_lossy(&output.stderr)
        .trim()
        .chars()
        .take(300)
        .collect();
    Err(if detail.is_empty() {
        format!("{program} exited without saying why.")
    } else {
        detail
    })
}

/// A remote URL's host and path, without user, port, or a `.git` suffix.
#[derive(Debug, PartialEq)]
struct RemoteLocation {
    host: String,
    path: Vec<String>,
}

fn parse_remote_url(url: &str) -> Option<RemoteLocation> {
    let (authority, path) = match url.split_once("://") {
        Some((scheme, rest)) => {
            if !matches!(
                scheme.to_ascii_lowercase().as_str(),
                "https" | "http" | "git" | "ssh" | "git+ssh" | "ssh+git"
            ) {
                return None;
            }
            rest.split_once('/')?
        }
        // `[user@]host:path`, the form scp uses. A slash before the colon, or a
        // drive letter, makes it a local path instead.
        None => {
            let (authority, path) = url.split_once(':')?;
            if authority.contains('/') || authority.len() < 2 {
                return None;
            }
            (authority, path)
        }
    };
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = match host.rsplit_once(':') {
        Some((host, port)) if port.bytes().all(|byte| byte.is_ascii_digit()) => host,
        _ => host,
    };
    if host.is_empty() || host.starts_with('-') {
        return None;
    }
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let path = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(percent_decode)
        .collect::<Option<Vec<String>>>()?;
    (!path.is_empty()).then(|| RemoteLocation {
        host: host.to_ascii_lowercase(),
        path,
    })
}

/// Decodes `%XX` escapes, which URLs use for spaces and other characters in
/// project and repository names.
fn percent_decode(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = segment.get(index + 1..index + 3)?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

/// The `[user@]host` and port Git passes to `ssh` to reach a remote. OpenSSH
/// configuration can match on either, as in `Match user git`.
#[derive(Debug, PartialEq)]
struct SshLogin {
    destination: String,
    port: Option<String>,
}

/// How Git logs in to reach `url` over SSH, worked out as `parse_connect_url`,
/// `get_host_and_port`, and `get_port` in Git's `connect.c` do, or `None` when
/// Git does not use SSH for `url` or refuses its host.
fn ssh_login(url: &str) -> Option<SshLogin> {
    // `<transport>::<address>` goes to a remote helper, and Git refuses rsync.
    let scheme_length = url
        .bytes()
        .enumerate()
        .take_while(|(index, byte)| {
            byte.is_ascii_alphanumeric() || (*index > 0 && matches!(byte, b'+' | b'-' | b'.'))
        })
        .count();
    if url[scheme_length..].starts_with("::") || url.starts_with("rsync:") {
        return None;
    }
    let (host, separator) = match url.split_once("://") {
        Some((scheme, rest)) => {
            if !matches!(scheme, "ssh" | "git+ssh" | "ssh+git") {
                return None;
            }
            (git_url_decode(rest)?, '/')
        }
        None => {
            let colon = url.find(':')?;
            let local_path = url.find('/').is_some_and(|slash| slash < colon)
                || (cfg!(windows) && colon == 1 && url.as_bytes()[0].is_ascii_alphabetic());
            if local_path {
                return None;
            }
            (url.to_string(), ':')
        }
    };
    // The path starts at the first separator after a bracketed host.
    let searched_from = bracketed(&host).map_or(0, |(_, close)| close);
    let path_start = searched_from + host[searched_from..].find(separator)?;
    let host_and_port = &host[..path_start];

    let (mut destination, mut port) = match bracketed(host_and_port) {
        Some((open, close)) => {
            let host = format!(
                "{}{}",
                &host_and_port[..open],
                &host_and_port[open + 1..close]
            );
            let port = host_and_port[close + 1..]
                .split_once(':')
                .map(|(_, port)| port)
                .filter(|port| is_port(port));
            (host, port.map(str::to_string))
        }
        None => match host_and_port.split_once(':') {
            Some((host, port)) if is_port(port) => (host.to_string(), Some(port.to_string())),
            Some((host, "")) => (host.to_string(), None),
            _ => (host_and_port.to_string(), None),
        },
    };
    if port.is_none() {
        if let Some((host, found)) = destination.split_once(':') {
            if is_port(found) {
                port = Some(found.to_string());
                destination = host.to_string();
            }
        }
    }
    // Git refuses a host that `ssh` would read as an option.
    (!destination.starts_with('-')).then_some(SshLogin { destination, port })
}

/// Where `host` brackets an address, as in `[::1]` or `user@[::1]`: the
/// positions of `[` and of the `]` that closes it.
fn bracketed(host: &str) -> Option<(usize, usize)> {
    let open = host.find("@[").map_or(0, |at| at + 1);
    if !host[open..].starts_with('[') {
        return None;
    }
    let close = open + 1 + host[open + 1..].find(']')?;
    Some((open, close))
}

/// Whether Git reads `text` as a port: what `strtol` reads as a whole number
/// from 0 through 65535, including leading whitespace and a sign.
fn is_port(text: &str) -> bool {
    let text = text.trim_start_matches([' ', '\t', '\n', '\u{b}', '\u{c}', '\r']);
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    match digits.parse::<u32>() {
        Ok(0) => true,
        Ok(value) => !negative && value < 65_536,
        Err(_) => false,
    }
}

/// Decodes `%XX` escapes as Git decodes a URL: an escape that is not two hex
/// digits, or that encodes a NUL, stays as written. `None` when the result is
/// not text.
fn git_url_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = (bytes[index] == b'%')
            .then(|| text.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            .filter(|value| *value != 0);
        match escaped {
            Some(value) => {
                decoded.push(value);
                index += 3;
            }
            None => {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).ok()
}

/// The host an SSH login connects to, as OpenSSH resolves its configuration
/// for the same destination and port Git passes.
fn ssh_hostname(worktree: &Path, login: &SshLogin) -> Option<String> {
    let mut arguments = vec!["-G".to_string()];
    if let Some(port) = &login.port {
        arguments.extend(["-p".to_string(), port.clone()]);
    }
    arguments.extend(["--".to_string(), login.destination.clone()]);
    let output = command::output_at(worktree, "ssh", &arguments).ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("hostname "))
        .map(|hostname| hostname.trim().to_ascii_lowercase())
        .filter(|hostname| !hostname.is_empty())
}

#[derive(Debug, PartialEq)]
struct GitHubRepository {
    host: String,
    owner: String,
    name: String,
}

/// GitHub.com, GitHub Enterprise Cloud (`*.ghe.com`), or a GitHub Enterprise
/// Server: the host `gh_host` (`GH_HOST`) names, or one with a `github` label,
/// as in `github.example.com`.
fn github_repository(location: &RemoteLocation, gh_host: Option<&str>) -> Option<GitHubRepository> {
    let host = location.host.as_str();
    let is_github = host == "github.com"
        || host.ends_with(".ghe.com")
        || host.split('.').any(|label| label == "github")
        || gh_host.is_some_and(|gh_host| gh_host.trim().eq_ignore_ascii_case(host));
    if !is_github {
        return None;
    }
    let [owner, name] = location.path.as_slice() else {
        return None;
    };
    Some(GitHubRepository {
        // GitHub serves SSH over port 443 from this host too.
        host: if host == "ssh.github.com" {
            "github.com".into()
        } else {
            host.to_string()
        },
        owner: owner.clone(),
        name: name.clone(),
    })
}

const GITHUB_QUERY: &str = "query($owner: String!, $name: String!, $ref: String!, $branch: String!) { repository(owner: $owner, name: $name) { ref(qualifiedName: $ref) { associatedPullRequests(states: OPEN, first: 100) { totalCount nodes { ...pull } } } pullRequests(baseRefName: $branch, states: OPEN, first: 100) { totalCount nodes { ...pull } } } } fragment pull on PullRequest { number title url baseRefName headRefName baseRepository { nameWithOwner } headRepository { nameWithOwner } }";

/// `gh api graphql` sends `-f` values as text, never reading a file for a
/// leading `@` as `-F` would.
fn github_arguments(repository: &GitHubRepository, remote_ref: &str, branch: &str) -> Vec<String> {
    vec![
        "api".into(),
        "graphql".into(),
        format!("--hostname={}", repository.host),
        "-f".into(),
        format!("query={GITHUB_QUERY}"),
        "-f".into(),
        format!("owner={}", repository.owner),
        "-f".into(),
        format!("name={}", repository.name),
        "-f".into(),
        format!("ref={remote_ref}"),
        "-f".into(),
        format!("branch={branch}"),
    ]
}

fn parse_github(bytes: &[u8], repository: &GitHubRepository) -> Result<Listed, String> {
    #[derive(Deserialize)]
    struct Response {
        data: Option<Data>,
    }
    #[derive(Deserialize)]
    struct Data {
        repository: Option<Repository>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Repository {
        #[serde(rename = "ref")]
        reference: Option<Reference>,
        pull_requests: Connection,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Reference {
        associated_pull_requests: Connection,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Connection {
        total_count: usize,
        nodes: Vec<Pull>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Pull {
        number: u64,
        title: String,
        url: Option<String>,
        base_ref_name: String,
        head_ref_name: String,
        base_repository: Option<NamedRepository>,
        head_repository: Option<NamedRepository>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct NamedRepository {
        name_with_owner: String,
    }

    let response: Response = serde_json::from_slice(bytes)
        .map_err(|error| format!("GitHub returned an unexpected response: {error}"))?;
    let found = response
        .data
        .and_then(|data| data.repository)
        .ok_or("GitHub did not find the repository the deletion pushes to.")?;
    let this = format!("{}/{}", repository.owner, repository.name);
    let named = |repository: Option<NamedRepository>, branch: String| match repository {
        Some(repository) => format!("{}:{branch}", repository.name_with_owner),
        // The fork it came from was deleted.
        None => branch,
    };
    let pull = |relation, pull: Pull| OpenPullRequest {
        repository: pull
            .base_repository
            .as_ref()
            .map_or_else(|| this.clone(), |base| base.name_with_owner.clone()),
        number: pull.number,
        title: pull.title,
        url: pull.url,
        relation,
        from: named(pull.head_repository, pull.head_ref_name),
        into: named(pull.base_repository, pull.base_ref_name),
    };
    // GitHub no longer having the branch leaves no pull request from it.
    let from_branch = found
        .reference
        .map(|reference| reference.associated_pull_requests)
        .unwrap_or(Connection {
            total_count: 0,
            nodes: Vec::new(),
        });
    let into_branch = found.pull_requests;
    let more_than_listed = from_branch.total_count > from_branch.nodes.len()
        || into_branch.total_count > into_branch.nodes.len();
    Ok(Listed {
        pulls: from_branch
            .nodes
            .into_iter()
            .map(|found| pull(PullRequestRelation::Source, found))
            .chain(
                into_branch
                    .nodes
                    .into_iter()
                    .map(|found| pull(PullRequestRelation::Target, found)),
            )
            .collect(),
        more_than_listed,
    })
}

#[derive(Debug, Clone, PartialEq)]
struct AzureRepository {
    organization_url: String,
    project: String,
    name: String,
}

fn azure_repository(location: &RemoteLocation) -> Option<AzureRepository> {
    let path: Vec<&str> = location.path.iter().map(String::as_str).collect();
    let (organization_url, project, name) = match (location.host.as_str(), path.as_slice()) {
        ("dev.azure.com", [organization, project, "_git", name]) => (
            format!("https://dev.azure.com/{organization}"),
            *project,
            *name,
        ),
        ("ssh.dev.azure.com" | "vs-ssh.visualstudio.com", ["v3", organization, project, name]) => (
            format!("https://dev.azure.com/{organization}"),
            *project,
            *name,
        ),
        (host, [project, "_git", name] | ["DefaultCollection", project, "_git", name])
            if host.ends_with(".visualstudio.com") =>
        {
            (format!("https://{host}"), *project, *name)
        }
        _ => return None,
    };
    Some(AzureRepository {
        organization_url,
        project: project.to_string(),
        name: name.to_string(),
    })
}

/// Every value is joined to its option, so one that starts with `-` cannot be
/// read as another option.
fn azure_arguments(
    repository: &AzureRepository,
    remote_ref: &str,
    relation: PullRequestRelation,
) -> Vec<String> {
    let branch_filter = match relation {
        PullRequestRelation::Source => "--source-branch",
        PullRequestRelation::Target => "--target-branch",
    };
    vec![
        "repos".into(),
        "pr".into(),
        "list".into(),
        format!("--organization={}", repository.organization_url),
        format!("--project={}", repository.project),
        format!("--repository={}", repository.name),
        format!("{branch_filter}={remote_ref}"),
        "--status=active".into(),
        format!("--top={LISTED}"),
        "--output=json".into(),
    ]
}

fn parse_azure(
    bytes: &[u8],
    repository: &AzureRepository,
    relation: PullRequestRelation,
) -> Result<Listed, String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Pull {
        pull_request_id: u64,
        title: String,
        source_ref_name: String,
        target_ref_name: String,
        fork_source: Option<serde_json::Value>,
    }

    let pulls: Vec<Pull> = serde_json::from_slice(bytes)
        .map_err(|error| format!("Azure DevOps returned an unexpected response: {error}"))?;
    // `az` does not say how many there are, only that it stopped at the limit.
    let more_than_listed = pulls.len() >= LISTED;
    let branch = |reference: &str| {
        format!(
            "{}:{}",
            repository.name,
            reference.strip_prefix("refs/heads/").unwrap_or(reference)
        )
    };
    let pulls = pulls
        .into_iter()
        // From a fork, the source is a different branch with the same name.
        .filter(|pull| {
            relation == PullRequestRelation::Target
                || pull
                    .fork_source
                    .as_ref()
                    .is_none_or(serde_json::Value::is_null)
        })
        .map(|pull| OpenPullRequest {
            repository: format!("{}/{}", repository.project, repository.name),
            number: pull.pull_request_id,
            title: pull.title,
            url: Some(format!(
                "{}/{}/_git/{}/pullrequest/{}",
                repository.organization_url,
                percent_encode(&repository.project),
                percent_encode(&repository.name),
                pull.pull_request_id
            )),
            relation,
            from: if pull
                .fork_source
                .as_ref()
                .is_some_and(|fork| !fork.is_null())
            {
                format!("a fork's {}", branch(&pull.source_ref_name))
            } else {
                branch(&pull.source_ref_name)
            },
            into: branch(&pull.target_ref_name),
        })
        .collect();
    Ok(Listed {
        pulls,
        more_than_listed,
    })
}

/// Escapes what a URL path segment cannot hold as-is.
fn percent_encode(segment: &str) -> String {
    segment
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(url: &str) -> RemoteLocation {
        parse_remote_url(url).unwrap_or_else(|| panic!("{url} should parse"))
    }

    fn github(path: &[&str]) -> GitHubRepository {
        GitHubRepository {
            host: "github.com".into(),
            owner: path[0].into(),
            name: path[1].into(),
        }
    }

    #[test]
    fn parses_the_remote_url_forms_git_accepts() {
        for url in [
            "https://github.com/octo/app.git",
            "https://user:token@github.com/octo/app",
            "https://github.com/octo/app/",
            "git://GitHub.com/octo/app.git",
            "ssh://git@github.com/octo/app.git",
            "ssh://git@github.com:22/octo/app.git",
            "git@github.com:octo/app.git",
            "github.com:octo/app",
        ] {
            assert_eq!(
                location(url),
                RemoteLocation {
                    host: "github.com".into(),
                    path: vec!["octo".into(), "app".into()],
                },
                "{url}"
            );
        }
        for local in [
            "/srv/git/app.git",
            "../app.git",
            "file:///srv/git/app.git",
            "C:\\code\\app.git",
            "C:/code/app.git",
            "ssh://-oProxyCommand=x/octo/app",
        ] {
            assert_eq!(parse_remote_url(local), None, "{local}");
        }
        assert_eq!(
            location("https://dev.azure.com/org/My%20Project/_git/My%20Repo").path,
            ["org", "My Project", "_git", "My Repo"]
        );
        assert_eq!(parse_remote_url("https://host/bad%zz/app"), None);
    }

    /// What Git reports it would pass to `ssh` for `url`, from its own
    /// diagnostics, or `None` when it would not use SSH.
    fn git_ssh_login(repository: &Path, url: &str) -> Option<SshLogin> {
        let output =
            command::git_at(repository, ["fetch-pack", "--diag-url", url]).expect("run fetch-pack");
        let report = String::from_utf8_lossy(&output.stdout);
        let field = |name: &str| {
            report
                .lines()
                .find_map(|line| line.strip_prefix(&format!("Diag: {name}=")))
                .map(str::to_string)
        };
        if !output.status.success() || field("protocol").as_deref() != Some("ssh") {
            return None;
        }
        let destination = field("userandhost").expect("the user and host");
        // Git checks this only once it runs `ssh`, after its diagnostics.
        if destination.starts_with('-') {
            return None;
        }
        Some(SshLogin {
            destination,
            port: field("port").filter(|port| port != "NONE"),
        })
    }

    #[test]
    fn logs_in_over_ssh_exactly_as_git_does() {
        let temp = tempfile::tempdir().expect("temp directory");
        command::successful_git_at(temp.path(), ["init", "--quiet"]).expect("git init");
        for url in [
            "ssh://git@Alias:2222/octo/app.git",
            "git+ssh://git@alias:22/octo/app",
            "ssh+git://alias/octo/app",
            "git@alias:octo/app.git",
            "alias:octo/app",
            "ssh://first.last%40corp@alias/octo/app",
            "ssh://a%zzb/octo/app",
            "ssh://a%00b/octo/app",
            "ssh://alias:/octo/app",
            "ssh://alias:99999/octo/app",
            "ssh://alias:022/octo/app",
            "ssh://alias:+22/octo/app",
            "ssh://alias:-0/octo/app",
            "ssh://alias:-1/octo/app",
            "ssh://alias:%2022/octo/app",
            "ssh://user:secret@alias/octo/app",
            "ssh://git@[::1]:22/octo/app",
            "ssh://git@[::1]x:22/octo/app",
            "[alias:2222]:octo/app",
            "git@[alias:2222]:octo/app",
            "ssh://-oProxyCommand=x/octo/app",
            "ssh://alias",
            "https://github.com/octo/app.git",
            "git://alias/octo/app",
            "SSH://alias/octo/app",
            "/srv/git/app.git",
            "../app.git",
            "c:octo/app",
            "C:\\code\\app.git",
        ] {
            assert_eq!(ssh_login(url), git_ssh_login(temp.path(), url), "{url}");
        }
    }

    #[test]
    fn remote_helper_urls_do_not_use_ssh() {
        // Git hands these to a remote helper before parsing them for SSH.
        assert_eq!(ssh_login("alias::octo/app"), None);
        assert_eq!(ssh_login("rsync:octo/app"), None);
        assert_eq!(
            ssh_login("git@alias:octo/app::x"),
            Some(SshLogin {
                destination: "git@alias".into(),
                port: None,
            })
        );
    }

    #[test]
    fn asks_only_hosts_that_are_github() {
        let found = |url: &str| github_repository(&location(url), None).map(|found| found.host);
        assert_eq!(
            found("git@ssh.github.com:octo/app.git"),
            Some("github.com".into())
        );
        assert_eq!(
            found("https://octo.ghe.com/team/app.git"),
            Some("octo.ghe.com".into())
        );
        assert_eq!(
            found("https://github.example.com/team/app.git"),
            Some("github.example.com".into())
        );
        assert_eq!(found("https://notgithub.com/octo/app.git"), None);
        assert_eq!(found("https://gitlab.com/octo/app.git"), None);
        assert_eq!(found("https://github.com/octo/app/extra"), None);
        // An Enterprise Server on any host name, once `GH_HOST` names it.
        assert_eq!(found("https://git.company.com/team/app.git"), None);
        assert_eq!(
            github_repository(
                &location("https://git.company.com/team/app.git"),
                Some("Git.Company.com")
            )
            .map(|found| found.host),
            Some("git.company.com".into())
        );
    }

    #[test]
    fn finds_azure_devops_repositories_in_each_url_form() {
        let expected = AzureRepository {
            organization_url: "https://dev.azure.com/contoso".into(),
            project: "Platform".into(),
            name: "app".into(),
        };
        for url in [
            "https://contoso@dev.azure.com/contoso/Platform/_git/app",
            "git@ssh.dev.azure.com:v3/contoso/Platform/app",
            "contoso@vs-ssh.visualstudio.com:v3/contoso/Platform/app",
        ] {
            assert_eq!(
                azure_repository(&location(url)),
                Some(expected.clone()),
                "{url}"
            );
        }
        for url in [
            "https://contoso.visualstudio.com/Platform/_git/app",
            "https://contoso.visualstudio.com/DefaultCollection/Platform/_git/app",
        ] {
            assert_eq!(
                azure_repository(&location(url)),
                Some(AzureRepository {
                    organization_url: "https://contoso.visualstudio.com".into(),
                    ..expected.clone()
                }),
                "{url}"
            );
        }
    }

    #[test]
    fn asks_about_exactly_the_reviewed_branch_with_values_joined_to_options() {
        let arguments =
            github_arguments(&github(&["octo", "app"]), "refs/heads/feature", "feature");
        assert_eq!(&arguments[..3], ["api", "graphql", "--hostname=github.com"]);
        for expected in [
            "owner=octo",
            "name=app",
            "ref=refs/heads/feature",
            "branch=feature",
        ] {
            assert!(arguments.contains(&expected.to_string()), "{expected}");
        }

        let repository = AzureRepository {
            organization_url: "https://dev.azure.com/contoso".into(),
            project: "-Platform".into(),
            name: "app".into(),
        };
        let arguments = azure_arguments(
            &repository,
            "refs/heads/feature",
            PullRequestRelation::Target,
        );
        assert!(arguments.contains(&"--project=-Platform".to_string()));
        assert!(arguments.contains(&"--target-branch=refs/heads/feature".to_string()));
    }

    #[test]
    fn reads_pull_requests_from_and_into_the_branch_from_github() {
        let listed = parse_github(
            br#"{"data":{"repository":{
                "ref":{"associatedPullRequests":{"totalCount":1,"nodes":[
                    {"number":42,"title":"Ship it","url":"https://github.com/upstream/app/pull/42","baseRefName":"main","headRefName":"feature","baseRepository":{"nameWithOwner":"upstream/app"},"headRepository":{"nameWithOwner":"octo/app"}}
                ]}},
                "pullRequests":{"totalCount":1,"nodes":[
                    {"number":42,"title":"Stacked","url":"https://github.com/octo/app/pull/42","baseRefName":"feature","headRefName":"next","baseRepository":{"nameWithOwner":"octo/app"},"headRepository":null}
                ]}
            }}}"#,
            &github(&["octo", "app"]),
        )
        .expect("GitHub response");
        assert!(!listed.more_than_listed);
        assert_eq!(
            listed.pulls,
            [
                OpenPullRequest {
                    repository: "upstream/app".into(),
                    number: 42,
                    title: "Ship it".into(),
                    url: Some("https://github.com/upstream/app/pull/42".into()),
                    relation: PullRequestRelation::Source,
                    from: "octo/app:feature".into(),
                    into: "upstream/app:main".into(),
                },
                OpenPullRequest {
                    repository: "octo/app".into(),
                    number: 42,
                    title: "Stacked".into(),
                    url: Some("https://github.com/octo/app/pull/42".into()),
                    relation: PullRequestRelation::Target,
                    from: "next".into(),
                    into: "octo/app:feature".into(),
                },
            ]
        );
        assert!(parse_github(
            br#"{"data":{"repository":{"ref":null,"pullRequests":{"totalCount":0,"nodes":[]}}}}"#,
            &github(&["octo", "app"])
        )
        .expect("no branch")
        .pulls
        .is_empty());
        // GitHub counts every open one, and lists at most a hundred of each kind.
        assert!(parse_github(
            br#"{"data":{"repository":{"ref":null,"pullRequests":{"totalCount":101,"nodes":[]}}}}"#,
            &github(&["octo", "app"])
        )
        .expect("more than listed")
        .more_than_listed);
        assert!(parse_github(
            br#"{"data":{"repository":null}}"#,
            &github(&["octo", "app"])
        )
        .is_err());
        assert!(parse_github(b"not json", &github(&["octo", "app"])).is_err());
    }

    #[test]
    fn reads_azure_devops_pull_requests_leaving_out_a_forks_namesake() {
        let repository = AzureRepository {
            organization_url: "https://dev.azure.com/contoso".into(),
            project: "My Project".into(),
            name: "app".into(),
        };
        let response = br#"[
            {"pullRequestId":7,"title":"Ship it","sourceRefName":"refs/heads/feature","targetRefName":"refs/heads/main","forkSource":null},
            {"pullRequestId":8,"title":"From a fork","sourceRefName":"refs/heads/feature","targetRefName":"refs/heads/main","forkSource":{"name":"refs/heads/feature"}}
        ]"#;
        let from = parse_azure(response, &repository, PullRequestRelation::Source)
            .expect("Azure response");
        assert!(!from.more_than_listed);
        assert_eq!(
            from.pulls,
            [OpenPullRequest {
                repository: "My Project/app".into(),
                number: 7,
                title: "Ship it".into(),
                url: Some(
                    "https://dev.azure.com/contoso/My%20Project/_git/app/pullrequest/7".into()
                ),
                relation: PullRequestRelation::Source,
                from: "app:feature".into(),
                into: "app:main".into(),
            }]
        );
        // Into the branch, a fork's pull request is affected like any other.
        let into = parse_azure(response, &repository, PullRequestRelation::Target)
            .expect("Azure response");
        assert_eq!(into.pulls.len(), 2);
        assert_eq!(into.pulls[1].from, "a fork's app:feature");

        // `az` stops at the limit without saying how many there are.
        let full = format!(
            "[{}]",
            (0..LISTED)
                .map(|number| format!(r#"{{"pullRequestId":{number},"title":"t","sourceRefName":"refs/heads/feature","targetRefName":"refs/heads/main","forkSource":null}}"#))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert!(
            parse_azure(full.as_bytes(), &repository, PullRequestRelation::Source)
                .expect("Azure response")
                .more_than_listed
        );
    }

    #[test]
    fn an_enterprise_token_goes_only_to_the_host_it_is_for() {
        let withheld = ["GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"];
        assert!(withheld_github_tokens("github.com", None).is_empty());
        assert_eq!(withheld_github_tokens("github.example.com", None), withheld);
        assert_eq!(
            withheld_github_tokens("github.example.com", Some("github.other.com".into())),
            withheld
        );
        assert!(
            withheld_github_tokens("github.example.com", Some(" GitHub.Example.com ".into()))
                .is_empty()
        );
    }

    #[test]
    fn a_local_remote_has_no_provider_to_ask() {
        let temp = tempfile::tempdir().expect("temp directory");
        assert_eq!(
            branch_pull_requests(temp.path(), "/srv/git/app.git", "refs/heads/feature"),
            BranchPullRequests::Unsupported
        );
    }
}
