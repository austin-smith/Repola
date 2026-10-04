use std::io::{Read, Write};
use std::sync::Mutex;

use super::{
    read_frame, write_frame, AgentCapability, AgentError, AgentEvent, AgentInfo, AgentRequest,
    AgentResult, FrameError, RequestEnvelope, ResponseBody, ResponseEnvelope, MAX_FRAME_BYTES,
    PROTOCOL_VERSION,
};
use crate::worktree;
use crate::{diagnostics, operation, operation::OperationToken};

pub fn execute<F>(request: RequestEnvelope, emit: F) -> ResponseEnvelope
where
    F: Fn(ResponseEnvelope) + Sync,
{
    execute_cancellable(request, emit, OperationToken::new())
}

pub fn execute_cancellable<F>(
    request: RequestEnvelope,
    emit: F,
    token: OperationToken,
) -> ResponseEnvelope
where
    F: Fn(ResponseEnvelope) + Sync,
{
    operation::with_operation(token, || execute_inner(request, emit))
}

fn execute_inner<F>(request: RequestEnvelope, emit: F) -> ResponseEnvelope
where
    F: Fn(ResponseEnvelope) + Sync,
{
    let request_id = request.request_id;
    if request.protocol_version != PROTOCOL_VERSION {
        return ResponseEnvelope::new(
            request_id,
            ResponseBody::Failure {
                error: AgentError::protocol(format!(
                    "Protocol version {} is not supported; this agent supports version {}.",
                    request.protocol_version, PROTOCOL_VERSION
                )),
            },
        );
    }

    let result = match request.request {
        AgentRequest::Handshake {
            client_version: _,
            minimum_protocol_version,
            maximum_protocol_version,
        } => {
            if minimum_protocol_version > PROTOCOL_VERSION
                || maximum_protocol_version < PROTOCOL_VERSION
            {
                Err(AgentError::protocol(format!(
                    "No compatible protocol version: client supports {minimum_protocol_version}–{maximum_protocol_version}, agent supports {PROTOCOL_VERSION}."
                )))
            } else {
                Ok(AgentResult::Handshake {
                    agent: AgentInfo {
                        agent_version: env!("CARGO_PKG_VERSION").to_string(),
                        protocol_version: PROTOCOL_VERSION,
                        operating_system: std::env::consts::OS.to_string(),
                        architecture: std::env::consts::ARCH.to_string(),
                        git_version: worktree::git_version().ok(),
                        maximum_frame_bytes: MAX_FRAME_BYTES,
                        capabilities: vec![
                            AgentCapability::RepositoryDiscovery,
                            AgentCapability::WorktreeInventory,
                            AgentCapability::WorkingCopy,
                            AgentCapability::History,
                            AgentCapability::Synchronization,
                            AgentCapability::Branches,
                            AgentCapability::RepositoryManagement,
                            AgentCapability::WorktreeManagement,
                            AgentCapability::Stashes,
                            AgentCapability::ProviderIntegration,
                            AgentCapability::TextGeneration,
                        ],
                    },
                })
            }
        }
        AgentRequest::ScanWorktrees { request } => worktree::scan_streaming(request, |event| {
            emit(ResponseEnvelope::new(
                request_id.clone(),
                ResponseBody::Event {
                    event: AgentEvent::Scan { event },
                },
            ));
        })
        .map(|result| AgentResult::ScanWorktrees { result })
        .map_err(|error| AgentError::operation(error.to_string())),
        AgentRequest::ResolveRepository { path } => worktree::resolve_repository(&path)
            .map(|repository_path| AgentResult::RepositoryResolved { repository_path })
            .map_err(AgentError::operation),
        AgentRequest::FetchPullRequests {
            repository_path,
            branch,
        } => worktree::fetch_pull_requests(&repository_path, &branch)
            .map(|evidence| AgentResult::PullRequests { evidence })
            .map_err(AgentError::operation),
        AgentRequest::MutatePullRequest { request } => worktree::mutate_pull_request(request)
            .map(|result| AgentResult::PullRequestMutation {
                result: Box::new(result),
            })
            .map_err(AgentError::operation),
        AgentRequest::WorktreeChanges {
            repository_path,
            worktree_path,
        } => worktree::worktree_changes(&repository_path, &worktree_path)
            .map(|changes| AgentResult::WorktreeChanges { changes })
            .map_err(AgentError::operation),
        AgentRequest::FileDiff { request, options } => {
            worktree::file_diff_for_display(request, options)
                .map(|diff| AgentResult::FileDiff { diff })
                .map_err(AgentError::operation)
        }
        AgentRequest::WorkingCopySnapshot { request } => worktree::working_copy_snapshot(request)
            .map(|snapshot| AgentResult::WorkingCopySnapshot { snapshot })
            .map_err(AgentError::operation),
        AgentRequest::SetFileStaging { request } => worktree::set_file_staging(request)
            .map(|snapshot| AgentResult::WorkingCopyUpdated { snapshot })
            .map_err(AgentError::operation),
        AgentRequest::Commit { request } => worktree::commit(request)
            .map(|result| AgentResult::Commit { result })
            .map_err(AgentError::operation),
        AgentRequest::GenerateCommitMessage { request } => {
            worktree::generate_commit_message(request)
                .map(|message| AgentResult::GeneratedCommitMessage { message })
                .map_err(AgentError::operation)
        }
        AgentRequest::TextGenerationStatus { provider } => Ok(AgentResult::TextGenerationStatus {
            status: worktree::text_generation_status(provider),
        }),
        AgentRequest::UndoCommit { request } => worktree::undo_commit(request)
            .map(|result| AgentResult::CommitUndone {
                result: Box::new(result),
            })
            .map_err(AgentError::operation),
        AgentRequest::History { request } => worktree::history(request)
            .map(|page| AgentResult::History { page })
            .map_err(AgentError::operation),
        AgentRequest::Reflog { request } => worktree::reflog(request)
            .map(|entries| AgentResult::Reflog { entries })
            .map_err(AgentError::operation),
        AgentRequest::CommitFiles { request } => worktree::commit_files(request)
            .map(|files| AgentResult::CommitFiles { files })
            .map_err(AgentError::operation),
        AgentRequest::CommitFileDiff { request, options } => {
            worktree::commit_file_diff(request, options)
                .map(|diff| AgentResult::CommitFileDiff { diff })
                .map_err(AgentError::operation)
        }
        AgentRequest::MutateHistory { request } => worktree::mutate_history(request)
            .map(|result| AgentResult::HistoryMutation { result })
            .map_err(AgentError::operation),
        AgentRequest::Tags { request } => worktree::tags(request)
            .map(|tags| AgentResult::Tags { tags })
            .map_err(AgentError::operation),
        AgentRequest::MutateTag { request } => worktree::mutate_tag(request)
            .map(|result| AgentResult::TagMutation { result })
            .map_err(AgentError::operation),
        AgentRequest::Synchronize { request } => worktree::synchronize(request)
            .map(|result| AgentResult::Synchronize { result })
            .map_err(AgentError::operation),
        AgentRequest::Branches { request } => worktree::branches(request)
            .map(|branches| AgentResult::Branches { branches })
            .map_err(AgentError::operation),
        AgentRequest::MutateBranch { request } => worktree::mutate_branch(request)
            .map(|result| AgentResult::BranchMutation { result })
            .map_err(AgentError::operation),
        AgentRequest::CloneRepository { request } => worktree::clone_repository(request)
            .map(|result| AgentResult::RepositoryOperation { result })
            .map_err(AgentError::operation),
        AgentRequest::CreateRepository { request } => worktree::create_repository(request)
            .map(|result| AgentResult::RepositoryOperation { result })
            .map_err(AgentError::operation),
        AgentRequest::CreateWorktree { request } => worktree::create_worktree(request)
            .map(|result| AgentResult::WorktreeCreated { result })
            .map_err(AgentError::operation),
        AgentRequest::Stashes { request } => worktree::list_stashes(request)
            .map(|stashes| AgentResult::Stashes { stashes })
            .map_err(AgentError::operation),
        AgentRequest::MutateStash { request } => worktree::mutate_stash(request)
            .map(|result| AgentResult::StashMutation { result })
            .map_err(AgentError::operation),
        AgentRequest::ResolveConflict { request } => worktree::resolve_conflict(request)
            .map(|snapshot| AgentResult::WorkingCopyUpdated { snapshot })
            .map_err(AgentError::operation),
        AgentRequest::ConflictFile { request } => worktree::conflict_file(request)
            .map(|file| AgentResult::ConflictFile { file })
            .map_err(AgentError::operation),
        AgentRequest::DiscardFile { request } => worktree::discard_file(request)
            .map(|snapshot| AgentResult::WorkingCopyUpdated { snapshot })
            .map_err(AgentError::operation),
        AgentRequest::DiscardAll { request } => worktree::discard_all(request)
            .map(|snapshot| AgentResult::WorkingCopyUpdated { snapshot })
            .map_err(AgentError::operation),
        AgentRequest::ApplyPatchHunk { request } => worktree::apply_patch_hunk(request)
            .map(|snapshot| AgentResult::WorkingCopyUpdated { snapshot })
            .map_err(AgentError::operation),
        AgentRequest::MutateRepositoryOperation { request } => worktree::mutate_operation(request)
            .map(|result| AgentResult::RepositoryOperationMutation { result })
            .map_err(AgentError::operation),
        AgentRequest::PrepareWorktreeAction { request } => worktree::prepare_action(request)
            .map(|plan| AgentResult::WorktreeActionPlan { plan })
            .map_err(|error| AgentError::operation(error.to_string())),
        AgentRequest::ExecuteWorktreeAction { request } => worktree::execute_action(request)
            .map(|result| AgentResult::WorktreeActionResult { result })
            .map_err(|error| AgentError::operation(error.to_string())),
    };

    let result = if operation::is_cancelled() {
        Err(AgentError::cancelled())
    } else {
        result
    }
    .map(diagnostics::redact_agent_result)
    .map_err(diagnostics::redact_agent_error);
    match result {
        Ok(result) => ResponseEnvelope::new(request_id, ResponseBody::Success { result }),
        Err(error) => ResponseEnvelope::new(request_id, ResponseBody::Failure { error }),
    }
}

pub fn serve<R, W>(reader: &mut R, writer: &mut W) -> Result<(), FrameError>
where
    R: Read,
    W: Write + Send,
{
    while let Some(request) = read_frame::<_, RequestEnvelope>(reader)? {
        let shared_writer = Mutex::new(&mut *writer);
        let write_error = Mutex::new(None);
        let response = execute(request, |event| {
            let Ok(mut first_error) = write_error.lock() else {
                return;
            };
            if first_error.is_some() {
                return;
            }
            match shared_writer.lock() {
                Ok(mut output) => {
                    *first_error = write_frame(&mut **output, &event).err();
                }
                Err(error) => {
                    *first_error = Some(FrameError::Write(std::io::Error::other(format!(
                        "protocol writer lock was poisoned: {error}"
                    ))));
                }
            }
        });
        if let Some(error) = write_error.into_inner().map_err(|error| {
            FrameError::Write(std::io::Error::other(format!(
                "protocol error lock was poisoned: {error}"
            )))
        })? {
            return Err(error);
        }
        let output = shared_writer.into_inner().map_err(|error| {
            FrameError::Write(std::io::Error::other(format!(
                "protocol writer lock was poisoned: {error}"
            )))
        })?;
        write_frame(output, &response)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn handshake_reports_platform_and_capabilities() {
        let response = execute(
            RequestEnvelope::current(
                "handshake-1",
                AgentRequest::Handshake {
                    client_version: "test".into(),
                    minimum_protocol_version: PROTOCOL_VERSION,
                    maximum_protocol_version: PROTOCOL_VERSION,
                },
            ),
            |_| panic!("handshake must not emit progress"),
        );

        assert_eq!(response.request_id, "handshake-1");
        match response.body {
            ResponseBody::Success {
                result: AgentResult::Handshake { agent },
            } => {
                assert_eq!(agent.protocol_version, PROTOCOL_VERSION);
                assert!(!agent.operating_system.is_empty());
                assert!(agent
                    .capabilities
                    .contains(&AgentCapability::WorktreeInventory));
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[test]
    fn incompatible_versions_fail_explicitly() {
        let response = execute(
            RequestEnvelope {
                protocol_version: PROTOCOL_VERSION + 1,
                request_id: "future".into(),
                request: AgentRequest::Handshake {
                    client_version: "future".into(),
                    minimum_protocol_version: PROTOCOL_VERSION + 1,
                    maximum_protocol_version: PROTOCOL_VERSION + 1,
                },
            },
            |_| {},
        );

        assert!(matches!(
            response.body,
            ResponseBody::Failure {
                error: AgentError {
                    kind: super::super::AgentErrorKind::Protocol,
                    ..
                }
            }
        ));
    }

    #[test]
    fn a_pre_cancelled_request_returns_structured_cancellation() {
        let token = OperationToken::new();
        token.cancel();
        let response = execute_cancellable(
            RequestEnvelope::current(
                "cancelled",
                AgentRequest::Handshake {
                    client_version: "test".into(),
                    minimum_protocol_version: PROTOCOL_VERSION,
                    maximum_protocol_version: PROTOCOL_VERSION,
                },
            ),
            |_| {},
            token,
        );
        assert!(matches!(
            response.body,
            ResponseBody::Failure {
                error: AgentError {
                    kind: super::super::AgentErrorKind::Cancelled,
                    retryable: true,
                    ..
                }
            }
        ));
    }

    #[test]
    fn stdio_service_round_trips_framed_requests_and_responses() {
        let request = RequestEnvelope::current(
            "wire-handshake",
            AgentRequest::Handshake {
                client_version: "test".into(),
                minimum_protocol_version: PROTOCOL_VERSION,
                maximum_protocol_version: PROTOCOL_VERSION,
            },
        );
        let mut input_bytes = Vec::new();
        write_frame(&mut input_bytes, &request).expect("encode request");
        let mut input = Cursor::new(input_bytes);
        let mut output = Vec::new();

        serve(&mut input, &mut output).expect("serve request");

        let response: ResponseEnvelope = read_frame(&mut Cursor::new(output))
            .expect("decode response")
            .expect("response frame");
        assert_eq!(response.request_id, "wire-handshake");
        assert!(matches!(
            response.body,
            ResponseBody::Success {
                result: AgentResult::Handshake { .. }
            }
        ));
    }
}
