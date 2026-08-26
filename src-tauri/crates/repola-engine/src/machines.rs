//! Machine profiles: the built-in local machine plus user-defined SSH machines.
//!
//! Profiles reference OpenSSH configuration by host alias and never carry keys or
//! passphrases. Validation lives here so every input path (desktop settings, future
//! CLI flags) rejects the same malformed or option-injecting values.

use serde::{Deserialize, Serialize};

pub const LOCAL_MACHINE_ID: &str = "local";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineProfile {
    pub id: String,
    pub name: String,
    pub kind: MachineKind,
    pub enabled: bool,
    pub ssh: Option<SshProfile>,
}

impl MachineProfile {
    pub fn local() -> Self {
        Self {
            id: LOCAL_MACHINE_ID.into(),
            name: "This computer".into(),
            kind: MachineKind::Local,
            enabled: true,
            ssh: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MachineKind {
    Local,
    Ssh,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshProfile {
    /// An OpenSSH Host alias or hostname. Repola deliberately delegates config,
    /// keys, ProxyJump, and known-host behavior to the system OpenSSH client.
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineProfileInput {
    pub id: Option<String>,
    pub name: String,
    pub enabled: bool,
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
}

#[derive(Debug, thiserror::Error)]
pub enum MachineError {
    #[error("Invalid machine profile: {0}")]
    Invalid(String),
    #[error("Machine {0:?} was not found.")]
    NotFound(String),
}

/// Validate user input and produce a stored SSH profile with a stable UUID.
pub fn normalize_machine(input: MachineProfileInput) -> Result<MachineProfile, MachineError> {
    let name = input.name.trim();
    validate_text("name", name, 80)?;
    let host = input.host.trim();
    validate_ssh_atom("host", host, 255)?;
    let user = input
        .user
        .map(|user| user.trim().to_string())
        .filter(|user| !user.is_empty());
    if let Some(user) = &user {
        validate_ssh_atom("user", user, 255)?;
    }
    if input.port == Some(0) {
        return Err(MachineError::Invalid(
            "port must be between 1 and 65535".into(),
        ));
    }
    let id = match input.id {
        Some(id) => {
            uuid::Uuid::parse_str(&id)
                .map_err(|_| MachineError::Invalid("the machine ID is not valid".into()))?;
            id
        }
        None => uuid::Uuid::new_v4().to_string(),
    };
    Ok(MachineProfile {
        id,
        name: name.to_string(),
        kind: MachineKind::Ssh,
        enabled: input.enabled,
        ssh: Some(SshProfile {
            host: host.to_string(),
            user,
            port: input.port,
        }),
    })
}

fn validate_text(field: &str, value: &str, maximum: usize) -> Result<(), MachineError> {
    if value.is_empty() {
        return Err(MachineError::Invalid(format!("{field} cannot be empty")));
    }
    if value.chars().count() > maximum {
        return Err(MachineError::Invalid(format!(
            "{field} cannot exceed {maximum} characters"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(MachineError::Invalid(format!(
            "{field} cannot contain control characters"
        )));
    }
    Ok(())
}

/// SSH host and user values are passed to the OpenSSH client as arguments, so they
/// must never look like an option or contain separators.
fn validate_ssh_atom(field: &str, value: &str, maximum: usize) -> Result<(), MachineError> {
    validate_text(field, value, maximum)?;
    if value.starts_with('-') {
        return Err(MachineError::Invalid(format!(
            "{field} cannot begin with a hyphen"
        )));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(MachineError::Invalid(format!(
            "{field} cannot contain whitespace"
        )));
    }
    Ok(())
}

pub fn reject_duplicate_connections(machines: &[MachineProfile]) -> Result<(), MachineError> {
    for (index, machine) in machines.iter().enumerate() {
        let Some(ssh) = &machine.ssh else { continue };
        if machines[..index].iter().any(|other| {
            other.ssh.as_ref().is_some_and(|candidate| {
                candidate.host.eq_ignore_ascii_case(&ssh.host)
                    && candidate.user == ssh.user
                    && candidate.port == ssh.port
            })
        }) {
            return Err(MachineError::Invalid(format!(
                "a profile for {} already exists",
                ssh.host
            )));
        }
    }
    Ok(())
}

/// The complete machine list always starts with the built-in local machine.
pub fn with_local_machine(stored: Vec<MachineProfile>) -> Vec<MachineProfile> {
    let mut machines = Vec::with_capacity(stored.len() + 1);
    machines.push(MachineProfile::local());
    machines.extend(stored);
    machines
}

/// Move one stored machine up or down by one position; moving past either end is a no-op.
pub fn move_machine(
    machines: &mut [MachineProfile],
    machine_id: &str,
    delta: i8,
) -> Result<(), MachineError> {
    let index = machines
        .iter()
        .position(|machine| machine.id == machine_id)
        .ok_or_else(|| MachineError::NotFound(machine_id.to_string()))?;
    let target = index as isize + delta as isize;
    if target >= 0 && target < machines.len() as isize {
        machines.swap(index, target as usize);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_order_moves_exactly_one_position() {
        let profile = |id: &str| MachineProfile {
            id: id.into(),
            name: id.into(),
            kind: MachineKind::Ssh,
            enabled: true,
            ssh: Some(SshProfile {
                host: id.into(),
                user: None,
                port: None,
            }),
        };
        let mut machines = vec![profile("one"), profile("two"), profile("three")];
        move_machine(&mut machines, "two", -1).expect("move up");
        assert_eq!(
            machines
                .iter()
                .map(|machine| machine.id.as_str())
                .collect::<Vec<_>>(),
            ["two", "one", "three"]
        );
        move_machine(&mut machines, "two", -1).expect("top is stable");
        assert_eq!(machines[0].id, "two");
        assert!(move_machine(&mut machines, "missing", 1).is_err());
    }

    #[test]
    fn machine_profiles_are_normalized_and_receive_stable_ids() {
        let profile = normalize_machine(MachineProfileInput {
            id: None,
            name: "  Build server  ".into(),
            enabled: true,
            host: "  buildbox  ".into(),
            user: Some("  deploy  ".into()),
            port: Some(2222),
        })
        .expect("valid profile");

        assert!(uuid::Uuid::parse_str(&profile.id).is_ok());
        assert_eq!(profile.name, "Build server");
        assert_eq!(
            profile.ssh,
            Some(SshProfile {
                host: "buildbox".into(),
                user: Some("deploy".into()),
                port: Some(2222),
            })
        );
    }

    #[test]
    fn machine_profiles_reject_ssh_option_injection_and_duplicates() {
        let error = normalize_machine(MachineProfileInput {
            id: None,
            name: "Unsafe".into(),
            enabled: true,
            host: "-oProxyCommand=bad".into(),
            user: None,
            port: None,
        })
        .expect_err("leading option must fail");
        assert!(matches!(error, MachineError::Invalid(_)));

        let first = normalize_machine(MachineProfileInput {
            id: None,
            name: "One".into(),
            enabled: true,
            host: "BuildBox".into(),
            user: Some("deploy".into()),
            port: None,
        })
        .expect("first");
        let second = normalize_machine(MachineProfileInput {
            id: None,
            name: "Two".into(),
            enabled: true,
            host: "buildbox".into(),
            user: Some("deploy".into()),
            port: None,
        })
        .expect("second");
        assert!(reject_duplicate_connections(&[first, second]).is_err());
    }

    #[test]
    fn the_local_machine_is_always_first() {
        let machines = with_local_machine(vec![]);
        assert_eq!(machines, vec![MachineProfile::local()]);
    }
}
