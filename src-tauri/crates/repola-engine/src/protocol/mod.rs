//! Versioned Repola desktop-to-agent protocol.
//!
//! Local and SSH-hosted repositories are addressed through the same request
//! contract. The local desktop can dispatch envelopes in process; the agent
//! binary reads and writes the identical envelopes as length-prefixed JSON.

mod frame;
mod messages;
mod service;

pub use frame::{read_frame, write_frame, FrameError, MAX_FRAME_BYTES};
pub use messages::{
    AgentCapability, AgentError, AgentErrorKind, AgentEvent, AgentInfo, AgentRequest, AgentResult,
    RequestEnvelope, ResponseBody, ResponseEnvelope, PROTOCOL_VERSION,
};
pub use service::{execute, execute_cancellable, serve};
