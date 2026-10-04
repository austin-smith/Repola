use std::ffi::OsString;
use std::io::{Read, Write};
use std::process::ExitStatus;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::host::HostError;
use crate::machines::MachineProfile;
use crate::operation::OperationToken;
use crate::protocol::{
    read_frame, write_frame, AgentCapability, AgentErrorKind, AgentInfo, AgentRequest, AgentResult,
    FrameError, RequestEnvelope, ResponseBody, ResponseEnvelope, PROTOCOL_VERSION,
};
use crate::worktree::command::{self, ManagedChild};

const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const STANDARD_TIMEOUT: Duration = Duration::from_secs(120);
const SCAN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Planning a discard or restore hashes working-copy content of any size.
const RECOVERY_PLAN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Discarding and restoring save and rewrite content of any size. Ending the
/// session does not stop the agent, so running out of time is reported as
/// unconfirmed rather than failed.
const RECOVERY_CHANGE_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
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
    // Another agent is tried only while no agent has received the request, so
    // a request never runs twice.
    let attempt = execute_once(
        machine,
        request.clone(),
        &emit,
        token.clone(),
        initial_command,
    );
    if let Some(outcome) = conclude(attempt) {
        return outcome;
    }

    let platform = super::bootstrap::detect_platform(machine, &token).map_err(not_delivered)?;
    let managed_command = super::bootstrap::managed_agent_command(platform);
    if initial_command != managed_command {
        let attempt = execute_once(
            machine,
            request.clone(),
            &emit,
            token.clone(),
            managed_command,
        );
        if let Some(outcome) = conclude(attempt) {
            return outcome;
        }
    }

    super::bootstrap::install_verified_agent(machine, platform, &token).map_err(not_delivered)?;
    let attempt = execute_once(machine, request, &emit, token, managed_command);
    match attempt.result {
        Ok(result) => Ok(result),
        Err(error) if attempt.delivered => Err(error),
        Err(error) => Err(not_delivered(error)),
    }
}

/// The outcome of an attempt, or `None` when another agent may be tried: only
/// while no agent has received the request, and only for an agent that is
/// missing or of another version.
fn conclude(attempt: Exchange) -> Option<Result<AgentResult, HostError>> {
    match attempt.result {
        Ok(result) => Some(Ok(result)),
        Err(error) if attempt.delivered => Some(Err(error)),
        Err(error) if agent_recovery_allowed(&error) => None,
        Err(error) => Some(Err(not_delivered(error))),
    }
}

fn not_delivered(error: HostError) -> HostError {
    HostError::NotDelivered(Box::new(error))
}

fn execute_once<F>(
    machine: &MachineProfile,
    request: RequestEnvelope,
    emit: &F,
    token: OperationToken,
    remote_command: &str,
) -> Exchange
where
    F: Fn(ResponseEnvelope) + Sync,
{
    let unsent = |error| Exchange {
        result: Err(error),
        answered: false,
        delivered: false,
    };
    if machine.ssh.is_none() {
        return unsent(HostError::InvalidProfile(machine.name.clone()));
    }
    let arguments = match arguments_for_command(machine, remote_command) {
        Ok(arguments) => arguments,
        Err(error) => return unsent(error),
    };
    let timeout = operation_timeout(&request.request);
    let mut child = match command::spawn_piped("ssh", &arguments) {
        Ok(child) => child,
        Err(error) => return unsent(HostError::Launch(error.to_string())),
    };

    let Some(mut input) = child.stdin().take() else {
        terminate(&mut child);
        return unsent(HostError::Transport(
            "OpenSSH stdin was not available".into(),
        ));
    };
    let handshake_id = format!("{}:handshake", request.request_id);
    let preflight = !matches!(&request.request, AgentRequest::Handshake { .. });
    // The request waits for a validated handshake: an agent of another version
    // would otherwise run it, and the retry below with the right agent would
    // run it a second time.
    let first = if preflight {
        RequestEnvelope::current(
            handshake_id.clone(),
            AgentRequest::Handshake {
                client_version: env!("CARGO_PKG_VERSION").into(),
                minimum_protocol_version: PROTOCOL_VERSION,
                maximum_protocol_version: PROTOCOL_VERSION,
            },
        )
    } else {
        request.clone()
    };
    if let Err(error) = write_frame(&mut input, &first) {
        terminate(&mut child);
        return unsent(HostError::Transport(error.to_string()));
    }
    let pending_input = if preflight {
        Some(input)
    } else {
        drop(input);
        None
    };

    let (Some(mut output), Some(stderr)) = (child.stdout().take(), child.stderr().take()) else {
        terminate(&mut child);
        return unsent(HostError::Transport(
            "OpenSSH output was not available".into(),
        ));
    };
    let diagnostic_reader = thread::spawn(move || bounded_diagnostics(stderr));

    let (wire_sender, wire_receiver) = mpsc::channel();
    let reader_handshake_id = handshake_id.clone();
    let wire_reader =
        thread::spawn(move || forward_responses(&mut output, &reader_handshake_id, &wire_sender));

    let mut exchange = converse(
        machine,
        &request,
        pending_input.map(|input| (handshake_id.as_str(), input)),
        &wire_receiver,
        &token,
        timeout,
        emit,
    );

    if exchange.result.is_err() {
        terminate(&mut child);
    }
    let status = wait_for_exit(&mut child, EXIT_GRACE_PERIOD);
    let _ = wire_reader.join();
    let diagnostics = diagnostic_reader
        .join()
        .unwrap_or_else(|_| "OpenSSH diagnostic reader failed".into());
    // The agent's own answer stands; OpenSSH exits by signal once it is
    // terminated after an error answer. Only without one does the exit explain
    // what went wrong.
    if exchange.answered
        || matches!(
            exchange.result,
            Err(HostError::Timeout { .. }
                | HostError::Unconfirmed { .. }
                | HostError::Cancelled(_))
        )
    {
        return exchange;
    }
    match status {
        Ok(status) if !status.success() => {
            exchange.result = Err(ssh_exit_error(
                machine,
                status,
                diagnostics,
                exchange.delivered,
            ));
        }
        Ok(_) => {}
        Err(error) => exchange.result = Err(error),
    }
    exchange
}

/// Forwards the agent's responses until the request's terminal one. Events and
/// the handshake's response come first, so neither ends the stream; a read
/// error or the end of the stream does.
fn forward_responses<R: Read>(
    output: &mut R,
    handshake_id: &str,
    sender: &mpsc::Sender<Result<Option<ResponseEnvelope>, FrameError>>,
) {
    loop {
        let response = read_frame::<_, ResponseEnvelope>(output);
        let terminal = match &response {
            Ok(Some(envelope)) => {
                !matches!(envelope.body, ResponseBody::Event { .. })
                    && envelope.request_id != handshake_id
            }
            Ok(None) | Err(_) => true,
        };
        if sender.send(response).is_err() || terminal {
            break;
        }
    }
}

/// How one exchange with the agent ended.
#[derive(Debug)]
struct Exchange {
    result: Result<AgentResult, HostError>,
    /// The result is the agent's own answer, which nothing that happens to
    /// OpenSSH afterwards can change.
    answered: bool,
    /// A request with effects was written to the agent, which may have run it.
    /// A handshake never counts: it changes nothing, so another agent may
    /// always be asked.
    delivered: bool,
}

/// What a response from the agent calls for next.
enum Step {
    Wait,
    SendRequest,
    Done(Box<Result<AgentResult, HostError>>),
}

/// Exchanges frames with the agent until its terminal response. With a
/// `handshake`, its ID names the handshake that was sent first, and the request
/// waits for the input beside it: the request is written only once the
/// handshake is valid, so an agent of another version never receives it.
fn converse<W, F>(
    machine: &MachineProfile,
    request: &RequestEnvelope,
    handshake: Option<(&str, W)>,
    responses: &mpsc::Receiver<Result<Option<ResponseEnvelope>, FrameError>>,
    token: &OperationToken,
    timeout: Duration,
    emit: &F,
) -> Exchange
where
    W: Write,
    F: Fn(ResponseEnvelope) + Sync,
{
    let (handshake_id, mut pending_input) = handshake.unzip();
    // Nothing with effects is written until the handshake is valid. Without
    // a handshake first, the request is itself a handshake.
    let mut delivered = false;
    let unanswered = |result, delivered| Exchange {
        result,
        answered: false,
        delivered,
    };
    let started = Instant::now();
    loop {
        if token.is_cancelled() {
            break unanswered(Err(HostError::Cancelled(machine.name.clone())), delivered);
        }
        let limit = phase_timeout(timeout, pending_input.is_some());
        let Some(remaining) = limit.checked_sub(started.elapsed()) else {
            let error = if delivered && changes_working_copy(&request.request) {
                HostError::Unconfirmed {
                    machine: machine.name.clone(),
                    minutes: limit.as_secs() / 60,
                }
            } else {
                HostError::Timeout {
                    machine: machine.name.clone(),
                    seconds: limit.as_secs(),
                }
            };
            break unanswered(Err(error), delivered);
        };
        let response = match responses.recv_timeout(remaining.min(Duration::from_millis(100))) {
            Ok(Ok(Some(response))) => response,
            Ok(Ok(None)) => {
                break unanswered(
                    Err(HostError::Protocol(
                        "the agent closed the stream before a terminal response".into(),
                    )),
                    delivered,
                );
            }
            Ok(Err(error)) => {
                break unanswered(Err(HostError::Protocol(error.to_string())), delivered)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break unanswered(
                    Err(HostError::Protocol(
                        "the agent response reader stopped unexpectedly".into(),
                    )),
                    delivered,
                );
            }
        };
        let awaiting_handshake = handshake_id.filter(|_| !delivered);
        match interpret(machine, request, awaiting_handshake, response, emit) {
            Step::Wait => {}
            Step::SendRequest => {
                let Some(mut input) = pending_input.take() else {
                    break unanswered(
                        Err(HostError::Protocol(
                            "the request was sent before its handshake".into(),
                        )),
                        delivered,
                    );
                };
                if let Err(error) = write_frame(&mut input, request) {
                    // A partly written frame may still reach the agent.
                    break unanswered(Err(HostError::Transport(error.to_string())), true);
                }
                drop(input);
                delivered = true;
            }
            Step::Done(result) => {
                break Exchange {
                    result: *result,
                    answered: true,
                    delivered,
                }
            }
        }
    }
}

/// Decides what one response calls for, while `awaiting_handshake` names the
/// handshake whose response comes first.
fn interpret<F>(
    machine: &MachineProfile,
    request: &RequestEnvelope,
    awaiting_handshake: Option<&str>,
    response: ResponseEnvelope,
    emit: &F,
) -> Step
where
    F: Fn(ResponseEnvelope) + Sync,
{
    if response.protocol_version != PROTOCOL_VERSION {
        let detail = format!(
            "agent responded with protocol version {}; expected {}",
            response.protocol_version, PROTOCOL_VERSION
        );
        return Step::Done(Box::new(Err(if awaiting_handshake.is_some() {
            preflight_protocol_error(machine, detail)
        } else {
            HostError::Protocol(detail)
        })));
    }
    let expected_id = awaiting_handshake.unwrap_or(request.request_id.as_str());
    if response.request_id != expected_id {
        return Step::Done(Box::new(Err(HostError::Protocol(format!(
            "agent response ID {:?} did not match request ID {:?}",
            response.request_id, expected_id
        )))));
    }
    if awaiting_handshake.is_some() {
        return match &response.body {
            ResponseBody::Success {
                result: AgentResult::Handshake { agent },
            } => match validate_handshake(agent, &request.request, &machine.name) {
                Ok(()) => Step::SendRequest,
                Err(error) => Step::Done(Box::new(Err(error))),
            },
            ResponseBody::Success { .. } | ResponseBody::Event { .. } => Step::Done(Box::new(Err(
                HostError::Protocol("the agent returned an invalid handshake response".into()),
            ))),
            ResponseBody::Failure { error } => {
                Step::Done(Box::new(Err(if error.kind == AgentErrorKind::Protocol {
                    preflight_protocol_error(machine, error.summary.clone())
                } else {
                    HostError::Remote(error.summary.clone())
                })))
            }
        };
    }
    match &response.body {
        ResponseBody::Event { .. } => {
            emit(response);
            Step::Wait
        }
        ResponseBody::Success { result } => {
            if let AgentResult::Handshake { agent } = result {
                if let Err(error) = validate_handshake(agent, &request.request, &machine.name) {
                    return Step::Done(Box::new(Err(error)));
                }
            }
            Step::Done(Box::new(Ok(result.clone())))
        }
        ResponseBody::Failure { error } => {
            Step::Done(Box::new(Err(HostError::Remote(error.summary.clone()))))
        }
    }
}

fn agent_recovery_allowed(error: &HostError) -> bool {
    matches!(
        error,
        HostError::AgentUnavailable { .. } | HostError::AgentVersionMismatch { .. }
    )
}

fn preflight_protocol_error(machine: &MachineProfile, detail: String) -> HostError {
    HostError::AgentUnavailable {
        machine: machine.name.clone(),
        detail,
    }
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
        | AgentRequest::PlanDiscard { .. }
        | AgentRequest::Discard { .. }
        | AgentRequest::RecoveryPoints { .. }
        | AgentRequest::PlanRecoveryRestore { .. }
        | AgentRequest::RestoreRecoveryPoint { .. }
        | AgentRequest::RecoveryFileDiff { .. }
        | AgentRequest::DeleteRecoveryPoints { .. }
        | AgentRequest::ApplyPatchHunk { .. }
        | AgentRequest::MutateRepositoryOperation { .. }
        | AgentRequest::Commit { .. }
        | AgentRequest::UndoCommit { .. } => &[AgentCapability::WorkingCopy],
        AgentRequest::GenerateCommitMessage { .. } | AgentRequest::TextGenerationStatus { .. } => {
            &[AgentCapability::TextGeneration]
        }
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

/// How long the exchange may run so far: an agent that has not answered the
/// handshake gets the handshake's time whatever the request is.
fn phase_timeout(timeout: Duration, awaiting_handshake: bool) -> Duration {
    if awaiting_handshake {
        timeout.min(HANDSHAKE_TIMEOUT)
    } else {
        timeout
    }
}

/// Requests that change files or recovery points, which the agent finishes
/// even after Repola stops waiting.
fn changes_working_copy(request: &AgentRequest) -> bool {
    matches!(
        request,
        AgentRequest::Discard { .. }
            | AgentRequest::RestoreRecoveryPoint { .. }
            | AgentRequest::DeleteRecoveryPoints { .. }
    )
}

fn operation_timeout(request: &AgentRequest) -> Duration {
    match request {
        AgentRequest::Discard { .. } | AgentRequest::RestoreRecoveryPoint { .. } => {
            RECOVERY_CHANGE_TIMEOUT
        }
        AgentRequest::PlanDiscard { .. } | AgentRequest::PlanRecoveryRestore { .. } => {
            RECOVERY_PLAN_TIMEOUT
        }
        AgentRequest::Handshake { .. } => HANDSHAKE_TIMEOUT,
        AgentRequest::ScanWorktrees { .. } => SCAN_TIMEOUT,
        AgentRequest::GenerateCommitMessage { request } => {
            // Allow a normal Git-operation budget for each context pass (before
            // and after generation), in addition to sequential provider work.
            HANDSHAKE_TIMEOUT
                + STANDARD_TIMEOUT * 2
                + crate::worktree::commit_message_provider_timeout(
                    request
                        .text_generation_selection
                        .as_ref()
                        .map(|selection| selection.provider),
                )
        }
        AgentRequest::TextGenerationStatus { provider } => (HANDSHAKE_TIMEOUT
            + crate::worktree::text_generation_status_timeout(*provider))
        .max(STANDARD_TIMEOUT),
        AgentRequest::ResolveRepository { .. }
        | AgentRequest::FetchPullRequests { .. }
        | AgentRequest::MutatePullRequest { .. }
        | AgentRequest::WorktreeChanges { .. }
        | AgentRequest::FileDiff { .. }
        | AgentRequest::WorkingCopySnapshot { .. }
        | AgentRequest::SetFileStaging { .. }
        | AgentRequest::ResolveConflict { .. }
        | AgentRequest::ConflictFile { .. }
        | AgentRequest::RecoveryPoints { .. }
        | AgentRequest::RecoveryFileDiff { .. }
        | AgentRequest::DeleteRecoveryPoints { .. }
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

fn wait_for_exit(
    child: &mut ManagedChild,
    grace_period: Duration,
) -> Result<ExitStatus, HostError> {
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
        // A connection that stops answering is noticed within a minute, even
        // for requests that wait hours.
        OsString::from("-o"),
        OsString::from("ServerAliveInterval=15"),
        OsString::from("-o"),
        OsString::from("ServerAliveCountMax=4"),
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

/// Why OpenSSH exited without the agent's answer. Before a request is
/// `delivered`, a shell that cannot find the agent means it is not installed;
/// after, the agent ran, so its answer was lost in transport.
fn ssh_exit_error(
    machine: &MachineProfile,
    status: ExitStatus,
    diagnostics: String,
    delivered: bool,
) -> HostError {
    let normalized = diagnostics.to_ascii_lowercase();
    if !delivered
        && normalized.contains("repola-agent")
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

fn terminate(child: &mut ManagedChild) {
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machines::{MachineKind, SshProfile};

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
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=4",
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
    fn preflight_protocol_errors_allow_the_managed_agent_to_be_refreshed() {
        let error = preflight_protocol_error(&profile(), "protocol 13 is too old".into());
        assert!(agent_recovery_allowed(&error));
        assert!(matches!(
            error,
            HostError::AgentUnavailable { machine, detail }
                if machine == "Build server" && detail == "protocol 13 is too old"
        ));
    }

    #[test]
    fn diagnostic_capture_is_bounded_but_drains_the_reader() {
        let input = vec![b'x'; MAX_DIAGNOSTIC_BYTES + 10_000];
        let result = bounded_diagnostics(input.as_slice());
        assert_eq!(result.len(), MAX_DIAGNOSTIC_BYTES);
    }

    #[test]
    fn changes_wait_long_but_never_on_an_unanswered_handshake() {
        use crate::worktree::{
            DeleteRecoveryPointsRequest, DiscardRequest, DiscardTarget, RecoveryPointReference,
            RecoveryRestoreRequest,
        };
        let point = RecoveryPointReference {
            id: "refs/repola/discarded/x".into(),
            oid: "0".repeat(40),
        };
        let discard = AgentRequest::Discard {
            request: DiscardRequest {
                repository_path: "repo".into(),
                worktree_path: "repo".into(),
                target: DiscardTarget::All,
                fingerprint: String::new(),
            },
        };
        let restore = AgentRequest::RestoreRecoveryPoint {
            request: RecoveryRestoreRequest {
                repository_path: "repo".into(),
                worktree_path: "repo".into(),
                point: point.clone(),
                fingerprint: String::new(),
            },
        };
        let delete = AgentRequest::DeleteRecoveryPoints {
            request: DeleteRecoveryPointsRequest {
                repository_path: "repo".into(),
                worktree_path: "repo".into(),
                points: vec![point],
            },
        };
        for request in [&discard, &restore, &delete] {
            assert_eq!(
                phase_timeout(operation_timeout(request), true),
                HANDSHAKE_TIMEOUT
            );
            assert!(changes_working_copy(request));
        }
        assert_eq!(operation_timeout(&discard), RECOVERY_CHANGE_TIMEOUT);
        assert_eq!(operation_timeout(&restore), RECOVERY_CHANGE_TIMEOUT);
        assert_eq!(operation_timeout(&delete), STANDARD_TIMEOUT);
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
    fn generation_deadlines_cover_provider_fallback_and_context_work() {
        use crate::preferences::{TextGenerationProvider, TextGenerationSelection};
        // Codex status: 4 x 20s; generation: 2 x 20s + 180s.
        // Claude status: 3 x 20s; generation: status + 180s.
        // Automatic discovery can check both before using the slower provider.
        for (provider, status_seconds, generation_seconds) in [
            (Some(TextGenerationProvider::Codex), 80, 220),
            (Some(TextGenerationProvider::Claude), 60, 240),
            (None, 140, 380),
        ] {
            let status = AgentRequest::TextGenerationStatus { provider };
            assert_eq!(
                operation_timeout(&status),
                (HANDSHAKE_TIMEOUT + Duration::from_secs(status_seconds)).max(STANDARD_TIMEOUT)
            );
            let generate = AgentRequest::GenerateCommitMessage {
                request: crate::worktree::GenerateCommitMessageRequest {
                    repository_path: "repo".into(),
                    worktree_path: "repo".into(),
                    expected_head: None,
                    included_changes: vec![],
                    amend: false,
                    text_generation_selection: provider.map(|provider| TextGenerationSelection {
                        provider,
                        model: None,
                        reasoning_effort: None,
                    }),
                },
            };
            assert_eq!(
                operation_timeout(&generate),
                HANDSHAKE_TIMEOUT + STANDARD_TIMEOUT * 2 + Duration::from_secs(generation_seconds)
            );
        }
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
            false,
        );
        assert!(matches!(error, HostError::AgentUnavailable { .. }));
        assert!(error.to_string().contains("matching Repola agent"));

        // Once the agent has the request it ran, whatever its shell printed.
        let error = ssh_exit_error(
            &profile(),
            status,
            "sh: repola-agent: command not found".into(),
            true,
        );
        assert!(matches!(error, HostError::Transport(_)), "{error:?}");
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

    fn scan_request() -> RequestEnvelope {
        RequestEnvelope::current(
            "scan-1",
            AgentRequest::ScanWorktrees {
                request: crate::worktree::ScanRequest {
                    repository_paths: vec![],
                },
            },
        )
    }

    fn agent(version: &str) -> AgentInfo {
        AgentInfo {
            agent_version: version.into(),
            protocol_version: PROTOCOL_VERSION,
            operating_system: "linux".into(),
            architecture: "x86_64".into(),
            git_version: Some("git version 2.50.0".into()),
            maximum_frame_bytes: crate::protocol::MAX_FRAME_BYTES,
            capabilities: vec![
                AgentCapability::RepositoryDiscovery,
                AgentCapability::WorktreeInventory,
            ],
        }
    }

    fn handshake_response(version: &str) -> ResponseEnvelope {
        ResponseEnvelope::new(
            "scan-1:handshake",
            ResponseBody::Success {
                result: AgentResult::Handshake {
                    agent: agent(version),
                },
            },
        )
    }

    fn answer() -> ResponseEnvelope {
        ResponseEnvelope::new(
            "scan-1",
            ResponseBody::Success {
                result: AgentResult::RepositoryResolved {
                    repository_path: "/work/repository".into(),
                },
            },
        )
    }

    #[test]
    fn responses_are_read_past_the_handshake_until_the_requests_answer() {
        let mut wire = Vec::new();
        for response in [
            handshake_response(env!("CARGO_PKG_VERSION")),
            answer(),
            answer(),
        ] {
            write_frame(&mut wire, &response).expect("write response");
        }
        let (sender, receiver) = mpsc::channel();
        forward_responses(&mut std::io::Cursor::new(wire), "scan-1:handshake", &sender);
        drop(sender);

        let forwarded: Vec<String> = receiver
            .iter()
            .map(|response| response.expect("frame").expect("response").request_id)
            .collect();
        assert_eq!(forwarded, ["scan-1:handshake", "scan-1"]);
    }

    fn exchange(responses: Vec<ResponseEnvelope>, input: &mut Vec<u8>) -> Exchange {
        let (sender, receiver) = mpsc::channel();
        for response in responses {
            sender.send(Ok(Some(response))).expect("queue response");
        }
        drop(sender);
        converse(
            &profile(),
            &scan_request(),
            Some(("scan-1:handshake", input)),
            &receiver,
            &OperationToken::new(),
            Duration::from_secs(5),
            &|_| {},
        )
    }

    #[test]
    fn the_request_is_sent_only_after_a_valid_handshake() {
        let mut input = Vec::new();
        let exchange = exchange(
            vec![handshake_response(env!("CARGO_PKG_VERSION")), answer()],
            &mut input,
        );
        assert!(
            matches!(exchange.result, Ok(AgentResult::RepositoryResolved { .. })),
            "{exchange:?}"
        );
        assert!(exchange.answered && exchange.delivered);
        let sent: RequestEnvelope = read_frame(&mut std::io::Cursor::new(input))
            .expect("read request")
            .expect("one request");
        assert_eq!(sent.request_id, "scan-1");
    }

    #[test]
    fn an_agent_of_another_version_never_receives_the_request() {
        let mut input = Vec::new();
        let exchange = exchange(vec![handshake_response("999.0.0")], &mut input);
        assert!(
            matches!(exchange.result, Err(HostError::AgentVersionMismatch { .. })),
            "{exchange:?}"
        );
        assert!(exchange.answered, "the mismatch is the agent's own answer");
        assert!(!exchange.delivered);
        assert!(input.is_empty(), "the request must not be written");
    }

    #[test]
    fn a_request_left_unanswered_may_have_run() {
        let mut input = Vec::new();
        let exchange = exchange(
            vec![handshake_response(env!("CARGO_PKG_VERSION"))],
            &mut input,
        );
        assert!(exchange.result.is_err());
        assert!(!exchange.answered);
        assert!(exchange.delivered);
    }

    #[test]
    fn only_an_agent_that_never_received_the_request_is_replaced() {
        let attempt = |result, delivered| Exchange {
            result,
            answered: false,
            delivered,
        };
        let mismatch = || HostError::AgentVersionMismatch {
            machine: "Build server".into(),
            expected: "1".into(),
            actual: "2".into(),
        };
        assert!(conclude(attempt(Err(mismatch()), false)).is_none());
        assert!(matches!(
            conclude(attempt(Err(mismatch()), true)),
            Some(Err(HostError::AgentVersionMismatch { .. }))
        ));
        assert!(matches!(
            conclude(attempt(Err(HostError::Transport("refused".into())), false)),
            Some(Err(HostError::NotDelivered(_)))
        ));
        assert!(matches!(
            conclude(attempt(Err(HostError::Transport("reset".into())), true)),
            Some(Err(HostError::Transport(_)))
        ));
    }

    #[test]
    fn a_handshake_never_counts_as_delivered() {
        let handshake = RequestEnvelope::current(
            "probe",
            AgentRequest::Handshake {
                client_version: env!("CARGO_PKG_VERSION").into(),
                minimum_protocol_version: PROTOCOL_VERSION,
                maximum_protocol_version: PROTOCOL_VERSION,
            },
        );
        let (sender, receiver) = mpsc::channel();
        drop(sender);
        let exchange = converse::<Vec<u8>, _>(
            &profile(),
            &handshake,
            None,
            &receiver,
            &OperationToken::new(),
            Duration::from_secs(5),
            &|_| {},
        );
        assert!(exchange.result.is_err());
        assert!(
            !exchange.delivered,
            "another agent may always be asked for a handshake"
        );
    }
}
