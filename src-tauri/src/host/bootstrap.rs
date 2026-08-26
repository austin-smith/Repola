use std::collections::HashMap;
use std::io::Read;
use std::process::Output;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use minisign_verify::{PublicKey, Signature};
use reqwest::blocking::Client;
use sha2::{Digest, Sha256};

use crate::diagnostics;
use crate::host::HostError;
use crate::operation::{self, OperationToken};
use crate::settings::MachineProfile;
use crate::worktree::command;

const RELEASE_REPOSITORY: &str = "austin-smith/repola";
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_AGENT_BYTES: usize = 64 * 1024 * 1024;
const MAX_METADATA_BYTES: usize = 32 * 1024;
const UNIX_PLATFORM_PROBE: &str =
    "sh -lc 'printf \"repola-platform:%s:%s\\n\" \"$(uname -s)\" \"$(uname -m)\"'";
const WINDOWS_PLATFORM_PROBE: &str = "powershell.exe -NoLogo -NoProfile -NonInteractive -Command \"$a=[System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString(); Write-Output ('repola-platform:Windows:'+$a)\"";
const UNIX_INSTALL_COMMAND: &str = "sh -lc 'set -eu; umask 077; d=\"$HOME/.local/bin\"; mkdir -p \"$d\"; t=\"$d/.repola-agent.tmp\"; cat > \"$t\"; chmod 700 \"$t\"; \"$t\" --version >/dev/null; mv -f \"$t\" \"$d/repola-agent\"'";
const WINDOWS_INSTALL_COMMAND: &str = "powershell.exe -NoLogo -NoProfile -NonInteractive -Command \"$ErrorActionPreference='Stop'; $d=Join-Path $env:LOCALAPPDATA 'Repola\\bin'; [IO.Directory]::CreateDirectory($d)|Out-Null; $t=Join-Path $d '.repola-agent.tmp.exe'; $f=[IO.File]::Create($t); try {[Console]::OpenStandardInput().CopyTo($f)} finally {$f.Dispose()}; & $t --version | Out-Null; Move-Item -Force $t (Join-Path $d 'repola-agent.exe')\"";

enum BootstrapError {
    Cancelled,
    Other(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RemotePlatform {
    MacArm64,
    MacX64,
    LinuxArm64,
    LinuxX64,
    WindowsX64,
}

impl RemotePlatform {
    fn target(self) -> &'static str {
        match self {
            Self::MacArm64 => "aarch64-apple-darwin",
            Self::MacX64 => "x86_64-apple-darwin",
            Self::LinuxArm64 => "aarch64-unknown-linux-gnu",
            Self::LinuxX64 => "x86_64-unknown-linux-gnu",
            Self::WindowsX64 => "x86_64-pc-windows-msvc",
        }
    }

    fn executable_suffix(self) -> &'static str {
        match self {
            Self::WindowsX64 => ".exe",
            Self::MacArm64 | Self::MacX64 | Self::LinuxArm64 | Self::LinuxX64 => "",
        }
    }

    fn install_command(self) -> &'static str {
        match self {
            Self::WindowsX64 => WINDOWS_INSTALL_COMMAND,
            Self::MacArm64 | Self::MacX64 | Self::LinuxArm64 | Self::LinuxX64 => {
                UNIX_INSTALL_COMMAND
            }
        }
    }
}

fn platform_cache() -> &'static Mutex<HashMap<String, RemotePlatform>> {
    static CACHE: OnceLock<Mutex<HashMap<String, RemotePlatform>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn cached_platform(machine: &MachineProfile) -> Option<RemotePlatform> {
    platform_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(&machine.id).copied())
}

pub(super) fn remember_platform(machine: &MachineProfile, platform: RemotePlatform) {
    if let Ok(mut cache) = platform_cache().lock() {
        cache.insert(machine.id.clone(), platform);
    }
}

pub(super) fn managed_agent_command(platform: RemotePlatform) -> &'static str {
    match platform {
        RemotePlatform::WindowsX64 => {
            "cmd.exe /d /s /c \"\"%LOCALAPPDATA%\\Repola\\bin\\repola-agent.exe\" --stdio\""
        }
        RemotePlatform::MacArm64
        | RemotePlatform::MacX64
        | RemotePlatform::LinuxArm64
        | RemotePlatform::LinuxX64 => "exec \"$HOME/.local/bin/repola-agent\" --stdio",
    }
}

pub(super) fn detect_platform(
    machine: &MachineProfile,
    token: &OperationToken,
) -> Result<RemotePlatform, HostError> {
    ensure_not_cancelled(machine, token)?;
    let unix = run_ssh(machine, UNIX_PLATFORM_PROBE, &[], token)?;
    if unix.status.success() {
        if let Some(platform) = parse_platform_output(&unix.stdout) {
            remember_platform(machine, platform);
            return Ok(platform);
        }
    }

    ensure_not_cancelled(machine, token)?;
    let windows = run_ssh(machine, WINDOWS_PLATFORM_PROBE, &[], token)?;
    if windows.status.success() {
        if let Some(platform) = parse_platform_output(&windows.stdout) {
            remember_platform(machine, platform);
            return Ok(platform);
        }
    }

    let unix_detail = output_detail(&unix);
    let windows_detail = output_detail(&windows);
    Err(not_ready(
        machine,
        format!(
            "Repola could not identify a supported remote platform. POSIX probe: {unix_detail}. Windows probe: {windows_detail}. Supported targets are macOS arm64/x64, Linux arm64/x64, and Windows x64."
        ),
    ))
}

pub(super) fn install_verified_agent(
    machine: &MachineProfile,
    platform: RemotePlatform,
    token: &OperationToken,
) -> Result<(), HostError> {
    ensure_not_cancelled(machine, token)?;
    let agent = match download_verified_agent(platform, token) {
        Ok(agent) => agent,
        Err(BootstrapError::Cancelled) => return Err(HostError::Cancelled(machine.name.clone())),
        Err(BootstrapError::Other(detail)) => return Err(not_ready(machine, detail)),
    };
    ensure_not_cancelled(machine, token)?;
    let output = run_ssh(machine, platform.install_command(), &agent, token)?;
    if !output.status.success() {
        return Err(not_ready(
            machine,
            format!(
                "The verified agent could not be installed in the remote user profile: {}",
                output_detail(&output)
            ),
        ));
    }
    Ok(())
}

fn run_ssh(
    machine: &MachineProfile,
    remote_command: &str,
    input: &[u8],
    token: &OperationToken,
) -> Result<Output, HostError> {
    let arguments = super::ssh::arguments_for_command(machine, remote_command)?;
    operation::with_operation(token.clone(), || {
        command::output_with_input("ssh", &arguments, input)
    })
    .map_err(|error| match error {
        command::CommandError::Cancelled { .. } => HostError::Cancelled(machine.name.clone()),
        command::CommandError::Timeout { seconds, .. } => HostError::Timeout {
            machine: machine.name.clone(),
            seconds,
        },
        other => HostError::Transport(other.to_string()),
    })
}

fn parse_platform_output(output: &[u8]) -> Option<RemotePlatform> {
    let output = String::from_utf8_lossy(output);
    let line = output
        .lines()
        .find(|line| line.starts_with("repola-platform:"))?;
    let mut parts = line.trim().split(':');
    if parts.next()? != "repola-platform" {
        return None;
    }
    let operating_system = parts.next()?.to_ascii_lowercase();
    let architecture = parts.next()?.to_ascii_lowercase();
    if parts.next().is_some() {
        return None;
    }
    match (operating_system.as_str(), architecture.as_str()) {
        ("darwin", "arm64" | "aarch64") => Some(RemotePlatform::MacArm64),
        ("darwin", "x86_64" | "amd64" | "x64") => Some(RemotePlatform::MacX64),
        ("linux", "arm64" | "aarch64") => Some(RemotePlatform::LinuxArm64),
        ("linux", "x86_64" | "amd64" | "x64") => Some(RemotePlatform::LinuxX64),
        ("windows", "x86_64" | "amd64" | "x64") => Some(RemotePlatform::WindowsX64),
        _ => None,
    }
}

fn download_verified_agent(
    platform: RemotePlatform,
    token: &OperationToken,
) -> Result<Vec<u8>, BootstrapError> {
    let public_key = option_env!("REPOLA_SIGNING_PUBLIC_KEY")
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .ok_or_else(|| BootstrapError::Other(
            "This Repola build has no release-signing public key, so automatic agent installation is disabled. Install the matching agent manually or use a production-signed Repola build.".to_string()
        ))?;
    let version = env!("CARGO_PKG_VERSION");
    let asset_name = format!(
        "repola-agent-{}{}",
        platform.target(),
        platform.executable_suffix()
    );
    let release_base =
        format!("https://github.com/{RELEASE_REPOSITORY}/releases/download/v{version}");
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(DOWNLOAD_TIMEOUT)
        .user_agent(format!("Repola/{version}"))
        .build()
        .map_err(|error| {
            BootstrapError::Other(format!("Could not initialize the release client: {error}"))
        })?;

    let checksum = download_bounded(
        &client,
        &format!("{release_base}/{asset_name}.sha256"),
        MAX_METADATA_BYTES,
        token,
    )?;
    let signature = download_bounded(
        &client,
        &format!("{release_base}/{asset_name}.sig"),
        MAX_METADATA_BYTES,
        token,
    )?;
    let agent = download_bounded(
        &client,
        &format!("{release_base}/{asset_name}"),
        MAX_AGENT_BYTES,
        token,
    )?;
    verify_agent(&agent, &checksum, &signature, public_key, &asset_name)
        .map_err(BootstrapError::Other)?;
    Ok(agent)
}

fn download_bounded(
    client: &Client,
    url: &str,
    maximum: usize,
    token: &OperationToken,
) -> Result<Vec<u8>, BootstrapError> {
    if token.is_cancelled() {
        return Err(BootstrapError::Cancelled);
    }
    let mut response = client
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| {
            BootstrapError::Other(format!(
                "Could not download a Repola release asset: {error}"
            ))
        })?;
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(BootstrapError::Other(format!(
            "A Repola release asset exceeded its {maximum}-byte size limit."
        )));
    }
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if token.is_cancelled() {
            return Err(BootstrapError::Cancelled);
        }
        let read = response.read(&mut buffer).map_err(|error| {
            BootstrapError::Other(format!("Could not read a Repola release asset: {error}"))
        })?;
        if read == 0 {
            break;
        }
        if bytes.len().saturating_add(read) > maximum {
            return Err(BootstrapError::Other(format!(
                "A Repola release asset exceeded its {maximum}-byte size limit."
            )));
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    Ok(bytes)
}

fn verify_agent(
    agent: &[u8],
    checksum: &[u8],
    signature: &[u8],
    public_key: &str,
    asset_name: &str,
) -> Result<(), String> {
    let checksum = std::str::from_utf8(checksum)
        .map_err(|_| "The agent checksum was not UTF-8.".to_string())?;
    let mut fields = checksum.split_whitespace();
    let expected_digest = fields
        .next()
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| "The agent checksum was malformed.".to_string())?;
    let expected_name = fields
        .next()
        .ok_or_else(|| "The agent checksum did not name its asset.".to_string())?
        .trim_start_matches('*');
    if fields.next().is_some() || expected_name != asset_name {
        return Err("The agent checksum did not match the requested release asset.".into());
    }
    let actual_digest = format!("{:x}", Sha256::digest(agent));
    if !actual_digest.eq_ignore_ascii_case(expected_digest) {
        return Err("The downloaded agent failed its SHA-256 checksum.".into());
    }

    let public_key = decode_tauri_signer_document(public_key, "public key")?;
    let public_key = PublicKey::decode(&public_key)
        .map_err(|error| format!("The embedded Repola signing public key is invalid: {error}"))?;
    let signature = std::str::from_utf8(signature)
        .map_err(|_| "The agent signature was not UTF-8.".to_string())?;
    let signature = decode_tauri_signer_document(signature, "signature")?;
    let signature = Signature::decode(&signature)
        .map_err(|error| format!("The agent signature was malformed: {error}"))?;
    public_key
        .verify(agent, &signature, false)
        .map_err(|error| format!("The downloaded agent signature was not trusted: {error}"))
}

fn decode_tauri_signer_document(value: &str, kind: &str) -> Result<String, String> {
    let decoded = BASE64
        .decode(value.trim())
        .map_err(|_| format!("The Tauri {kind} was not valid base64."))?;
    let document = String::from_utf8(decoded)
        .map_err(|_| format!("The decoded Tauri {kind} was not UTF-8."))?;
    if document.trim().is_empty() {
        return Err(format!("The decoded Tauri {kind} was empty."));
    }
    Ok(document)
}

fn ensure_not_cancelled(machine: &MachineProfile, token: &OperationToken) -> Result<(), HostError> {
    if token.is_cancelled() {
        Err(HostError::Cancelled(machine.name.clone()))
    } else {
        Ok(())
    }
}

fn output_detail(output: &Output) -> String {
    let status = output
        .status
        .code()
        .map_or_else(|| "signal".to_string(), |code| code.to_string());
    let stderr = diagnostics::redact(String::from_utf8_lossy(&output.stderr).trim());
    let stdout = diagnostics::redact(String::from_utf8_lossy(&output.stdout).trim());
    let message = if !stderr.is_empty() { stderr } else { stdout };
    if message.is_empty() {
        format!("exit {status}")
    } else {
        format!("exit {status}: {message}")
    }
}

fn not_ready(machine: &MachineProfile, detail: impl Into<String>) -> HostError {
    HostError::AgentUnavailable {
        machine: machine.name.clone(),
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PUBLIC_KEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    const TEST_SIGNATURE: &str = "untrusted comment: signature from minisign secret key\nRUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\ntrusted comment: timestamp:1556193335\tfile:test\ny/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg==";

    fn tauri_public_key() -> String {
        BASE64.encode(format!(
            "untrusted comment: minisign public key\n{TEST_PUBLIC_KEY}\n"
        ))
    }

    fn tauri_signature() -> Vec<u8> {
        BASE64.encode(TEST_SIGNATURE).into_bytes()
    }

    #[test]
    fn maps_only_supported_release_targets() {
        assert_eq!(
            parse_platform_output(b"repola-platform:Darwin:arm64\n"),
            Some(RemotePlatform::MacArm64)
        );
        assert_eq!(
            parse_platform_output(b"repola-platform:Linux:x86_64\n"),
            Some(RemotePlatform::LinuxX64)
        );
        assert_eq!(
            parse_platform_output(b"repola-platform:Windows:X64\r\n"),
            Some(RemotePlatform::WindowsX64)
        );
        assert_eq!(
            parse_platform_output(b"repola-platform:Linux:aarch64\n"),
            Some(RemotePlatform::LinuxArm64)
        );
    }

    #[test]
    fn verifies_the_checksum_name_and_minisign_signature() {
        let checksum = b"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08  repola-agent-test\n";
        verify_agent(
            b"test",
            checksum,
            &tauri_signature(),
            &tauri_public_key(),
            "repola-agent-test",
        )
        .expect("the published Minisign fixture must verify");
    }

    #[test]
    fn rejects_a_checksum_for_another_asset_before_signature_verification() {
        let checksum =
            b"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08  another-agent\n";
        let error = verify_agent(
            b"test",
            checksum,
            &tauri_signature(),
            &tauri_public_key(),
            "repola-agent-test",
        )
        .expect_err("the asset name is part of the checksum contract");
        assert!(error.contains("requested release asset"));
    }
}
