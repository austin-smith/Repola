//! Execution boundary for machines that own Git working copies.

mod bootstrap;
mod ssh;

use crate::machines::{MachineKind, MachineProfile};
use crate::operation::OperationToken;
use crate::protocol::{
    self, AgentRequest, AgentResult, RequestEnvelope, ResponseBody, ResponseEnvelope,
};

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("Machine {0:?} is disabled.")]
    Disabled(String),
    #[error("Machine profile {0:?} is incomplete.")]
    InvalidProfile(String),
    #[error("Could not start OpenSSH: {0}")]
    Launch(String),
    #[error("Repola is not ready on {machine:?}: {detail}")]
    AgentUnavailable { machine: String, detail: String },
    #[error("Repola agent {actual} on {machine:?} does not match desktop version {expected}.")]
    AgentVersionMismatch {
        machine: String,
        expected: String,
        actual: String,
    },
    #[error("The operation on {machine:?} exceeded its {seconds}-second deadline.")]
    Timeout { machine: String, seconds: u64 },
    #[error("{machine:?} has not finished the change after {minutes} minutes. It may still finish there, so check the working copy and Discarded Changes before trying again.")]
    Unconfirmed { machine: String, minutes: u64 },
    #[error("The operation on {0:?} was cancelled.")]
    Cancelled(String),
    #[error("SSH transport failed: {0}")]
    Transport(String),
    #[error("The remote Repola agent protocol failed: {0}")]
    Protocol(String),
    #[error("The remote Repola agent rejected the operation: {0}")]
    Remote(String),
    /// The error stopped the operation before any agent received the request,
    /// so nothing it asked for ran.
    #[error("{0}")]
    NotDelivered(Box<HostError>),
}

pub fn execute<F>(
    machine: &MachineProfile,
    request: RequestEnvelope,
    emit: F,
    token: OperationToken,
) -> Result<AgentResult, HostError>
where
    F: Fn(ResponseEnvelope) + Sync,
{
    if !machine.enabled {
        return Err(HostError::Disabled(machine.name.clone()));
    }
    match machine.kind {
        MachineKind::Local => terminal_result(protocol::execute_cancellable(request, emit, token)),
        MachineKind::Ssh => ssh::execute(machine, request, emit, token),
    }
}

pub fn handshake(
    machine: &MachineProfile,
    request_id: String,
) -> Result<protocol::AgentInfo, HostError> {
    handshake_with_token(machine, request_id, OperationToken::new())
}

pub fn handshake_with_token(
    machine: &MachineProfile,
    request_id: String,
    token: OperationToken,
) -> Result<protocol::AgentInfo, HostError> {
    let result = execute(
        machine,
        RequestEnvelope::current(
            request_id,
            AgentRequest::Handshake {
                client_version: env!("CARGO_PKG_VERSION").into(),
                minimum_protocol_version: protocol::PROTOCOL_VERSION,
                maximum_protocol_version: protocol::PROTOCOL_VERSION,
            },
        ),
        |_| {},
        token,
    )?;
    match result {
        AgentResult::Handshake { agent } => Ok(agent),
        _ => Err(HostError::Protocol(
            "the agent returned a non-handshake result".into(),
        )),
    }
}

fn terminal_result(response: ResponseEnvelope) -> Result<AgentResult, HostError> {
    match response.body {
        ResponseBody::Success { result } => Ok(result),
        ResponseBody::Failure { error } => Err(HostError::Remote(error.summary)),
        ResponseBody::Event { .. } => Err(HostError::Protocol(
            "an event was returned as the terminal response".into(),
        )),
    }
}
