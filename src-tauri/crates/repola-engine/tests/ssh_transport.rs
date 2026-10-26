//! The SSH transport end to end, with a stand-in `ssh` that runs the agent
//! built alongside these tests instead of connecting anywhere. It lives in its
//! own test binary because it points `PATH` at the stand-in.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

use repola_engine::host;
use repola_engine::machines::{MachineKind, MachineProfile, SshProfile};
use repola_engine::operation::OperationToken;
use repola_engine::protocol::{AgentRequest, AgentResult, RequestEnvelope};
use repola_engine::worktree::WorkingCopyRequest;

#[test]
fn a_request_over_ssh_gets_its_own_answer_after_the_handshake() {
    let bin = tempfile::tempdir().expect("bin");
    let ssh = bin.path().join("ssh");
    std::fs::write(
        &ssh,
        format!(
            "#!/bin/sh\nexec '{}' --stdio\n",
            env!("CARGO_BIN_EXE_repola-agent")
        ),
    )
    .expect("stand-in ssh");
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).expect("executable");
    let path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{path}", bin.path().display()));

    let repository = tempfile::tempdir().expect("repository");
    let status = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(repository.path())
        .status()
        .expect("git init");
    assert!(status.success());
    let repository = repository.path().to_string_lossy().into_owned();

    let machine = MachineProfile {
        id: "stand-in".into(),
        name: "Stand-in".into(),
        kind: MachineKind::Ssh,
        enabled: true,
        ssh: Some(SshProfile {
            host: "stand-in".into(),
            user: None,
            port: None,
        }),
    };
    let result = host::execute(
        &machine,
        RequestEnvelope::current(
            "transport-1",
            AgentRequest::RecoveryPoints {
                request: WorkingCopyRequest {
                    repository_path: repository.clone(),
                    worktree_path: repository,
                },
            },
        ),
        |_| {},
        OperationToken::new(),
    )
    .expect("round trip over ssh");
    assert!(matches!(result, AgentResult::RecoveryPoints { .. }));
}
