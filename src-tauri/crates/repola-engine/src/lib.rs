//! The Repola engine: everything that touches Git, the filesystem, child processes,
//! hosting providers, and remote machines.
//!
//! This crate has no dependency on Tauri. The desktop app wraps it in thin IPC
//! commands, and the `repola-agent` binary serves the same typed protocol over
//! stdio on remote machines, so both paths share one implementation and one
//! test suite.

pub mod diagnostics;
pub mod host;
pub mod machines;
pub mod operation;
pub mod preferences;
pub mod protocol;
pub mod worktree;
