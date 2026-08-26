use std::ffi::OsString;
use std::io::Read;
use std::process::{Child, ExitStatus};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::host::HostError;
use crate::operation::OperationToken;
use crate::protocol::{
    read_frame, write_frame, AgentCapability, AgentInfo, AgentRequest, AgentResult,
    RequestEnvelope, ResponseBody, ResponseEnvelope, PROTOCOL_VERSION,
};
use crate::settings::MachineProfile;
use crate::worktree::command;

const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const STANDARD_TIMEOUT: Duration = Duration::from_secs(120);
const SCAN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const EXIT_GRACE_PERIOD: Duration = Duration::from_secs(5);

pub(super) fn execute<F>(
    machine: &MachineProfile,
    request: RequestEnvelope,
    emit: F,
    token: OperationToken,
) -> Result<AgentResult, HostError>
where
    F: Fn(ResponseEnvelope) + Sync,
{
    let cached = super::bootstrap::cached_platform(machine);
    let initial_command = cached.map_or("repola-agent --stdio", |platform| {
        super::bootstrap::managed_agent_command(platform)
    });
    match execute_once(
        machine,
        request.clone(),
        &emit,
        token.clone(),
        initial_command,
    ) {
        Ok(result) => return Ok(result),
        Err(error) if !agent_recovery_allowed(&error) => return Err(error),
        Err(_) => {}
    }

    let platform = super::bootstrap::detect_platform(machine, &token)?;
    let managed_command = super::bootstrap::managed_agent_command(platform);
    if initial_command != managed_command {
        match execute_once(
            machine,
            request.clone(),
            &emit,
            token.clone(),
            managed_command,
        ) {
            Ok(result) => return Ok(result),
            Err(error) if !agent_recovery_allowed(&error) => return Err(error),
            Err(_) => {}
        }
    }

    super::bootstrap::install_verified_agent(machine, platform, &token)?;
    execute_once(machine, request, &emit, token, managed_command)
}

fn execute_once<F>(
    machine: &MachineProfile,
    request: RequestEnvelope,
    emit: &F,
    token: OperationToken,
    remote_command: &str,
) -> Result<AgentResult, HostError>
where
    F: Fn(ResponseEnvelope) + Sync,
{
    if machine.ssh.is_none() {
        return Err(HostError::InvalidProfile(machine.name.clone()));
    }
    let arguments = arguments_for_command(machine, remote_command)?;
    let timeout = operation_timeout(&request.request);
    let mut child = command::spawn_piped("ssh", &arguments)
        .map_err(|error| HostError::Launch(error.to_string()))?;
    let request_id = request.request_id.clone();

    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| HostError::Transport("OpenSSH stdin was not available".into()))?;
    let handshake_id = format!("{}:handshake", request.request_id);
    let preflight = !matches!(&request.request, AgentRequest::Handshake { .. });
    if preflight {
        let handshake = RequestEnvelope::current(
            handshake_id.clone(),
            AgentRequest::Handshake {
                client_version: env!("CARGO_PKG_VERSION").into(),
                minimum_protocol_version: PROTOCOL_VERSION,
                maximum_protocol_version: PROTOCOL_VERSION,
            },
        );
        if let Err(error) = write_frame(&mut input, &handshake) {
            terminate(&mut child);
            return Err(HostError::Transport(error.to_string()));
        }
    }
    if let Err(error) = write_frame(&mut input, &request) {
        terminate(&mut child);
        return Err(HostError::Transport(error.to_string()));
    }
    drop(input);

    let mut output = child
        .stdout
        .take()
        .ok_or_else(|| HostError::Transport("OpenSSH stdout was not available".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| HostError::Transport("OpenSSH stderr was not available".into()))?;
    let diagnostic_reader = thread::spawn(move || bounded_diagnostics(stderr));

    let (wire_sender, wire_receiver) = mpsc::channel();
    let wire_reader = thread::spawn(move || loop {
        let response = read_frame::<_, ResponseEnvelope>(&mut output);
        let terminal = !matches!(
            &response,
            Ok(Some(ResponseEnvelope {
                body: ResponseBody::Event { .. },
                ..
            }))
        );
        if wire_sender.send(response).is_err() || terminal {
            break;
        }
    });

    let started = Instant::now();
    let mut awaiting_handshake = preflight;
    let terminal = loop {
        if token.is_cancelled() {
            break Err(HostError::Cancelled(machine.name.clone()));
        }
        let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
            break Err(HostError::Timeout {
                machine: machine.name.clone(),
                seconds: timeout.as_secs(),
            });
        };
        let response = match wire_receiver.recv_timeout(remaining.min(Duration::from_millis(100))) {
            Ok(Ok(Some(response))) => response,
            Ok(Ok(None)) => {
                break Err(HostError::Protocol(
                    "the agent closed the stream before a terminal response".into(),
                ));
            }
            Ok(Err(error)) => break Err(HostError::Protocol(error.to_string())),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err(HostError::Protocol(
                    "the agent response reader stopped unexpectedly".into(),
                ));
            }
        };
        if response.protocol_version != PROTOCOL_VERSION {
            break Err(HostError::Protocol(format!(
                "agent responded with protocol version {}; expected {}",
                response.protocol_version, PROTOCOL_VERSION
            )));
        }
        let expected_id = if awaiting_handshake {
            &handshake_id
        } else {
            &request_id
        };
        if response.request_id != *expected_id {
            break Err(HostError::Protocol(format!(
                "agent response ID {:?} did not match request ID {:?}",
                response.request_id, expected_id
            )));
        }
        if awaiting_handshake {
            match &response.body {
                ResponseBody::Success {
                    result: AgentResult::Handshake { agent },
                } => match validate_handshake(agent, &request.request, &machine.name) {
                    Ok(()) => {
                        awaiting_handshake = false;
                        continue;
                    }
                    Err(error) => break Err(error),
                },
                ResponseBody::Success { .. } | ResponseBody::Event { .. } => {
                    break Err(HostError::Protocol(
                        "the agent returned an invalid handshake response".into(),
                    ));
                }
                ResponseBody::Failure { error } => {
                    break Err(HostError::Remote(error.summary.clone()));
                }
            }
        }
        match &response.body {
            ResponseBody::Event { .. } => emit(response),
            ResponseBody::Success { result } => {
                if let AgentResult::Handshake { agent } = result {
                    if let Err(error) = validate_handshake(agent, &request.request, &machine.name) {
                        break Err(error);
                    }
                }
                break Ok(result.clone());
            }
            ResponseBody::Failure { error } => break Err(HostError::Remote(error.summary.clone())),
        }
    };

    if terminal.is_err() {
        terminate(&mut child);
    }
    let status = wait_for_exit(&mut child, EXIT_GRACE_PERIOD)?;
    let _ = wire_reader.join();
    let diagnostics = diagnostic_reader
        .join()
        .unwrap_or_else(|_| "OpenSSH diagnostic reader failed".into());
    if matches!(
        terminal,
        Err(HostError::Timeout { .. } | HostError::Cancelled(_))
    ) {
        return terminal;
    }
    if !status.success() {
        return Err(ssh_exit_error(machine, status, diagnostics));
    }
    terminal
}

fn agent_recovery_allowed(error: &HostError) -> bool {
    matches!(
        error,
        HostError::AgentUnavailable { .. } | HostError::AgentVersionMismatch { .. }
    )
}

fn validate_handshake(
    agent: &AgentInfo,
    request: &AgentRequest,
    machine_name: &str,
) -> Result<(), HostError> {
    if agent.agent_version != env!("CARGO_PKG_VERSION") {
        return Err(HostError::AgentVersionMismatch {
            machine: machine_name.into(),
            expected: env!("CARGO_PKG_VERSION").into(),
            actual: agent.agent_version.clone(),
        });
    }
    if agent.protocol_version != PROTOCOL_VERSION {
        return Err(HostError::Protocol(format!(
            "agent handshake selected protocol {}; expected {}",
            agent.protocol_version, PROTOCOL_VERSION
        )));
    }
    if agent.maximum_frame_bytes < crate::protocol::MAX_FRAME_BYTES {
        return Err(HostError::Protocol(format!(
            "agent frame limit {} is below Repola's required {} bytes",
            agent.maximum_frame_bytes,
            crate::protocol::MAX_FRAME_BYTES
        )));
    }
    let required: &[AgentCapability] = match request {
        AgentRequest::ScanWorktrees { .. } => &[
            AgentCapability::RepositoryDiscovery,
            AgentCapability::WorktreeInventory,
        ],
        AgentRequest::ResolveRepository { .. } => &[AgentCapability::RepositoryDiscovery],
        AgentRequest::FetchPullRequests { .. } | AgentRequest::MutatePullRequest { .. } => {
            &[AgentCapability::ProviderIntegration]
        }
        AgentRequest::WorktreeChanges { .. }
        | AgentRequest::PrepareWorktreeAction { .. }
        | AgentRequest::ExecuteWorktreeAction { .. } => &[AgentCapability::WorktreeInventory],
        AgentRequest::WorkingCopySnapshot { .. }
        | AgentRequest::FileDiff { .. }
        | AgentRequest::SetFileStaging { .. }
        | AgentRequest::ResolveConflict { .. }
        | AgentRequest::ConflictFile { .. }
        | AgentRequest::DiscardFile { .. }
        | AgentRequest::DiscardAll { .. }
        | AgentRequest::ApplyPatchHunk { .. }
        | AgentRequest::MutateRepositoryOperation { .. }
        | AgentRequest::Commit { .. }
        | AgentRequest::UndoCommit { .. } => &[AgentCapability::WorkingCopy],
        AgentRequest::History { .. }
        | AgentRequest::Reflog { .. }
        | AgentRequest::CommitFiles { .. }
        | AgentRequest::CommitFileDiff { .. }
        | AgentRequest::MutateHistory { .. }
        | AgentRequest::Tags { .. }
        | AgentRequest::MutateTag { .. } => &[AgentCapability::History],
        AgentRequest::Synchronize { .. } => &[AgentCapability::Synchronization],
        AgentRequest::Branches { .. } | AgentRequest::MutateBranch { .. } => {
            &[AgentCapability::Branches]
        }
        AgentRequest::CloneRepository { .. } | AgentRequest::CreateRepository { .. } => {
            &[AgentCapability::RepositoryManagement]
        }
        AgentRequest::CreateWorktree { .. } => &[AgentCapability::WorktreeManagement],
        AgentRequest::Stashes { .. } | AgentRequest::MutateStash { .. } => {
            &[AgentCapability::Stashes]
        }
        AgentRequest::Handshake { .. } => &[],
    };
    if let Some(capability) = required
        .iter()
        .find(|capability| !agent.capabilities.contains(capability))
    {
        return Err(HostError::Protocol(format!(
            "agent {} does not advertise required capability {capability:?}",
            agent.agent_version
        )));
    }
    Ok(())
}

fn operation_timeout(request: &AgentRequest) -> Duration {
    match request {
        AgentRequest::Handshake { .. } => HANDSHAKE_TIMEOUT,
        AgentRequest::ScanWorktrees { .. } => SCAN_TIMEOUT,
        AgentRequest::ResolveRepository { .. }
        | AgentRequest::FetchPullRequests { .. }
        | AgentRequest::MutatePullRequest { .. }
        | AgentRequest::WorktreeChanges { .. }
        | AgentRequest::FileDiff { .. }
        | AgentRequest::WorkingCopySnapshot { .. }
        | AgentRequest::SetFileStaging { .. }
        | AgentRequest::ResolveConflict { .. }
        | AgentRequest::ConflictFile { .. }
        | AgentRequest::DiscardFile { .. }
        | AgentRequest::DiscardAll { .. }
        | AgentRequest::ApplyPatchHunk { .. }
        | AgentRequest::MutateRepositoryOperation { .. }
        | AgentRequest::Commit { .. }
        | AgentRequest::UndoCommit { .. }
        | AgentRequest::History { .. }
        | AgentRequest::Reflog { .. }
        | AgentRequest::CommitFiles { .. }
        | AgentRequest::CommitFileDiff { .. }
        | AgentRequest::MutateHistory { .. }
        | AgentRequest::Tags { .. }
        | AgentRequest::MutateTag { .. }
        | AgentRequest::Synchronize { .. }
        | AgentRequest::Branches { .. }
        | AgentRequest::MutateBranch { .. }
        | AgentRequest::CloneRepository { .. }
        | AgentRequest::CreateRepository { .. }
        | AgentRequest::CreateWorktree { .. }
        | AgentRequest::Stashes { .. }
        | AgentRequest::MutateStash { .. }
        | AgentRequest::PrepareWorktreeAction { .. }
        | AgentRequest::ExecuteWorktreeAction { .. } => STANDARD_TIMEOUT,
    }
}

fn wait_for_exit(child: &mut Child, grace_period: Duration) -> Result<ExitStatus, HostError> {
    let deadline = Instant::now() + grace_period;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                terminate(child);
                return child.wait().map_err(|error| {
                    HostError::Transport(format!(
                        "could not reap OpenSSH after termination: {error}"
                    ))
                });
            }
            Err(error) => {
                return Err(HostError::Transport(format!(
                    "could not inspect OpenSSH status: {error}"
                )));
            }
        }
    }
}

#[cfg(test)]
fn ssh_arguments(machine: &MachineProfile) -> Result<Vec<OsString>, HostError> {
    arguments_for_command(machine, "repola-agent --stdio")
}

pub(super) fn arguments_for_command(
    machine: &MachineProfile,
    remote_command: &str,
) -> Result<Vec<OsString>, HostError> {
    let ssh = machine
        .ssh
        .as_ref()
        .ok_or_else(|| HostError::InvalidProfile(machine.name.clone()))?;
    let mut arguments = vec![
        OsString::from("-T"),
        OsString::from("-o"),
        OsString::from("ForwardAgent=no"),
        OsString::from("-o"),
        OsString::from("PermitLocalCommand=no"),
        OsString::from("-o"),
        OsString::from("ClearAllForwardings=yes"),
        OsString::from("-o"),
        OsString::from("ConnectTimeout=15"),
    ];
    if let Some(port) = ssh.port {
        arguments.push(OsString::from("-p"));
        arguments.push(OsString::from(port.to_string()));
    }
    if let Some(user) = &ssh.user {
        arguments.push(OsString::from("-l"));
        arguments.push(OsString::from(user));
    }
    arguments.push(OsString::from(&ssh.host));
    // This is deliberately fixed. Repola does not expose a generic remote
    // command surface and no profile value is interpolated into this command.
    arguments.push(OsString::from(remote_command));
    Ok(arguments)
}

fn bounded_diagnostics<R: Read>(mut reader: R) -> String {
    let mut captured = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let remaining = MAX_DIAGNOSTIC_BYTES.saturating_sub(captured.len());
                captured.extend_from_slice(&buffer[..read.min(remaining)]);
            }
        }
    }
    String::from_utf8_lossy(&captured).trim().to_string()
}

fn ssh_exit_error(machine: &MachineProfile, status: ExitStatus, diagnostics: String) -> HostError {
    let normalized = diagnostics.to_ascii_lowercase();
    if normalized.contains("repola-agent")
        && (normalized.contains("not found")
            || normalized.contains("no such file")
            || normalized.contains("not recognized"))
    {
        return HostError::AgentUnavailable {
            machine: machine.name.clone(),
            detail: "repola-agent is not installed or is not on the non-interactive SSH PATH. Install the matching Repola agent, then test the connection again.".into(),
        };
    }
    let code = status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "terminated by signal".into());
    let diagnostics = crate::diagnostics::redact(&diagnostics);
    let detail = if diagnostics.is_empty() {
        format!("OpenSSH exited with {code}")
    } else {
        format!("OpenSSH exited with {code}: {diagnostics}")
    };
    HostError::Transport(detail)
}

fn terminate(child: &mut Child) {
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{MachineKind, SshProfile};

    fn profile() -> MachineProfile {
        MachineProfile {
            id: "test".into(),
            name: "Build server".into(),
            kind: MachineKind::Ssh,
            enabled: true,
            ssh: Some(SshProfile {
                host: "buildbox".into(),
                user: Some("deploy".into()),
                port: Some(2222),
            }),
        }
    }

    #[test]
    fn ssh_arguments_keep_profile_values_separate_from_the_fixed_command() {
        let arguments = ssh_arguments(&profile()).expect("arguments");
        assert_eq!(
            arguments,
            [
                "-T",
                "-o",
                "ForwardAgent=no",
                "-o",
                "PermitLocalCommand=no",
                "-o",
                "ClearAllForwardings=yes",
                "-o",
                "ConnectTimeout=15",
                "-p",
                "2222",
                "-l",
                "deploy",
                "buildbox",
                "repola-agent --stdio",
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn diagnostic_capture_is_bounded_but_drains_the_reader() {
        let input = vec![b'x'; MAX_DIAGNOSTIC_BYTES + 10_000];
        let result = bounded_diagnostics(input.as_slice());
        assert_eq!(result.len(), MAX_DIAGNOSTIC_BYTES);
    }

    #[test]
    fn requests_have_bounded_operation_deadlines() {
        let handshake = AgentRequest::Handshake {
            client_version: "test".into(),
            minimum_protocol_version: 1,
            maximum_protocol_version: 1,
        };
        let scan = AgentRequest::ScanWorktrees {
            request: crate::worktree::ScanRequest {
                repository_paths: vec![],
            },
        };
        assert_eq!(operation_timeout(&handshake), HANDSHAKE_TIMEOUT);
        assert_eq!(operation_timeout(&scan), SCAN_TIMEOUT);
    }

    #[test]
    fn missing_agent_diagnostic_becomes_an_actionable_error() {
        #[cfg(unix)]
        let status = std::os::unix::process::ExitStatusExt::from_raw(127 << 8);
        #[cfg(windows)]
        let status = std::os::windows::process::ExitStatusExt::from_raw(127);
        let error = ssh_exit_error(
            &profile(),
            status,
            "sh: repola-agent: command not found".into(),
        );
        assert!(matches!(error, HostError::AgentUnavailable { .. }));
        assert!(error.to_string().contains("matching Repola agent"));
    }

    #[test]
    fn handshake_validation_requires_protocol_frame_size_and_capabilities() {
        let request = AgentRequest::ScanWorktrees {
            request: crate::worktree::ScanRequest {
                repository_paths: vec![],
            },
        };
        let valid = AgentInfo {
            agent_version: env!("CARGO_PKG_VERSION").into(),
            protocol_version: PROTOCOL_VERSION,
            operating_system: "linux".into(),
            architecture: "x86_64".into(),
            git_version: Some("git version 2.50.0".into()),
            maximum_frame_bytes: crate::protocol::MAX_FRAME_BYTES,
            capabilities: vec![
                AgentCapability::RepositoryDiscovery,
                AgentCapability::WorktreeInventory,
            ],
        };
        assert!(validate_handshake(&valid, &request, "Build server").is_ok());

        let mut missing_capability = valid.clone();
        missing_capability.capabilities.clear();
        assert!(validate_handshake(&missing_capability, &request, "Build server").is_err());

        let mut small_frames = valid;
        small_frames.maximum_frame_bytes -= 1;
        assert!(validate_handshake(&small_frames, &request, "Build server").is_err());
    }

    #[test]
    fn handshake_validation_requires_the_exact_desktop_agent_version() {
        let agent = AgentInfo {
            agent_version: "999.0.0".into(),
            protocol_version: PROTOCOL_VERSION,
            operating_system: "linux".into(),
            architecture: "x86_64".into(),
            git_version: Some("git version 2.50.0".into()),
            maximum_frame_bytes: crate::protocol::MAX_FRAME_BYTES,
            capabilities: vec![],
        };
        let request = AgentRequest::Handshake {
            client_version: env!("CARGO_PKG_VERSION").into(),
            minimum_protocol_version: PROTOCOL_VERSION,
            maximum_protocol_version: PROTOCOL_VERSION,
        };
        assert!(matches!(
            validate_handshake(&agent, &request, "Build server"),
            Err(HostError::AgentVersionMismatch { .. })
        ));
    }
}
