use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::process::Child;
use std::process::{ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Output, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::operation;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MAX_COMMAND_STREAM_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum CommandError {
    #[error("Failed to launch {program}: {source}")]
    Launch {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{program} exited with status {status}: {message}")]
    Failed {
        program: String,
        status: i32,
        message: String,
    },
    #[error("{program} was cancelled")]
    Cancelled { program: String },
    #[error("{program} exceeded its {seconds}-second deadline")]
    Timeout { program: String, seconds: u64 },
    #[error("{program} produced more than {maximum} bytes on {stream}")]
    OutputTooLarge {
        program: String,
        stream: &'static str,
        maximum: usize,
    },
    #[error("Could not save the output of {program}: {source}")]
    SaveOutput {
        program: String,
        #[source]
        source: std::io::Error,
    },
}

/// Where a child's stdout goes: captured in memory up to the stream bound, or
/// copied without a bound into a file the caller owns.
enum StdoutSink {
    Capture,
    File(std::fs::File),
}

/// Windows `CREATE_NO_WINDOW` process-creation flag. Without it every child
/// process spawned from the GUI app flashes a console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A supervised child owns its Windows job for its entire lifetime. Detached
/// editor/terminal launches deliberately do not use this wrapper.
pub(crate) struct ManagedChild {
    #[cfg(windows)]
    inner: Box<dyn process_wrap::std::ChildWrapper>,
    #[cfg(not(windows))]
    inner: Child,
}

impl ManagedChild {
    pub(crate) fn stdin(&mut self) -> &mut Option<ChildStdin> {
        #[cfg(windows)]
        {
            self.inner.stdin()
        }
        #[cfg(not(windows))]
        {
            &mut self.inner.stdin
        }
    }

    pub(crate) fn stdout(&mut self) -> &mut Option<ChildStdout> {
        #[cfg(windows)]
        {
            self.inner.stdout()
        }
        #[cfg(not(windows))]
        {
            &mut self.inner.stdout
        }
    }

    pub(crate) fn stderr(&mut self) -> &mut Option<ChildStderr> {
        #[cfg(windows)]
        {
            self.inner.stderr()
        }
        #[cfg(not(windows))]
        {
            &mut self.inner.stderr
        }
    }

    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        #[cfg(windows)]
        {
            // JobObject is our sole child wrapper. Observe the direct child;
            // Drop terminates remaining descendants before readers are joined.
            self.inner.inner_mut().try_wait()
        }
        #[cfg(not(windows))]
        {
            self.inner.try_wait()
        }
    }

    pub(crate) fn wait(&mut self) -> std::io::Result<ExitStatus> {
        #[cfg(windows)]
        {
            self.inner.inner_mut().wait()
        }
        #[cfg(not(windows))]
        {
            self.inner.wait()
        }
    }

    pub(crate) fn kill(&mut self) -> std::io::Result<()> {
        #[cfg(windows)]
        {
            self.inner.start_kill()
        }
        #[cfg(not(windows))]
        {
            self.inner.kill()
        }
    }
}

#[cfg(windows)]
impl Drop for ManagedChild {
    fn drop(&mut self) {
        // Also end descendants when the shim exits first or an error path
        // drops the owner, before joining readers of their inherited pipes.
        let _ = self.kill();
        let _ = self.wait();
    }
}

fn spawn_managed(command: Command) -> std::io::Result<ManagedChild> {
    #[cfg(windows)]
    {
        use process_wrap::std::{CommandWrap, CreationFlags, JobObject};
        let mut command = CommandWrap::from(command);
        let mut flags = CreationFlags(Default::default());
        flags.0 .0 = CREATE_NO_WINDOW;
        // JobObject suspends the child before assigning it, then resumes it.
        // A .cmd shim cannot spawn an uncontained provider in between.
        let inner = command.wrap(flags).wrap(JobObject).spawn()?;
        Ok(ManagedChild { inner })
    }
    #[cfg(not(windows))]
    {
        let mut command = command;
        command.spawn().map(|inner| ManagedChild { inner })
    }
}

/// Resolved executable locations, keyed by the program name we were asked for.
/// Only successful lookups are cached so a tool installed mid-session is found
/// on the next call without restarting the app.
fn resolved_programs() -> &'static Mutex<HashMap<String, PathBuf>> {
    static CACHE: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Locate `program` on `PATH` the way the platform shell would.
///
/// `std::process::Command` only consults `PATH` for bare names and, on
/// Windows, only appends `.exe`. Tools distributed as `.cmd`/`.bat` shims
/// (the Azure CLI, many npm-installed CLIs) are invisible to it, so every spawn
/// goes through this resolver, which honours `PATHEXT` on Windows.
pub(crate) fn resolve_program(program: &str) -> Result<PathBuf, CommandError> {
    if let Some(path) = resolved_programs()
        .lock()
        .ok()
        .and_then(|cache| cache.get(program).cloned())
    {
        return Ok(path);
    }
    let path = which::which(program).map_err(|error| CommandError::Launch {
        program: program.to_string(),
        source: std::io::Error::new(std::io::ErrorKind::NotFound, error.to_string()),
    })?;
    if let Ok(mut cache) = resolved_programs().lock() {
        cache.insert(program.to_string(), path.clone());
    }
    Ok(path)
}

pub(crate) fn find_program(program: &str) -> Option<PathBuf> {
    resolve_program(program).ok()
}

pub(crate) fn spawn_detached<I, S>(
    program: &Path,
    args: I,
    working_directory: Option<&Path>,
) -> Result<(), CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(working_directory) = working_directory {
        command.current_dir(working_directory);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|source| CommandError::Launch {
            program: program.to_string_lossy().into_owned(),
            source,
        })
}

/// Build a `Command` for `program` with the platform-appropriate spawn flags.
fn command(program: &str) -> Result<Command, CommandError> {
    let path = resolve_program(program)?;
    #[allow(unused_mut)]
    let mut command = Command::new(path);
    if program == "git" {
        // Repola marks exact paths with `:(literal)`. An inherited literal mode
        // would make that prefix part of the name, and an inherited icase mode
        // would widen it to paths differing only in case.
        for variable in INHERITED_PATHSPEC_MODES {
            command.env_remove(variable);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(command)
}

const INHERITED_PATHSPEC_MODES: [&str; 2] = ["GIT_LITERAL_PATHSPECS", "GIT_ICASE_PATHSPECS"];

fn launch(program: &str, command: Command, input: Option<&[u8]>) -> Result<Output, CommandError> {
    launch_with_timeout(program, command, input, COMMAND_TIMEOUT)
}

fn launch_with_timeout(
    program: &str,
    command: Command,
    input: Option<&[u8]>,
    timeout: Duration,
) -> Result<Output, CommandError> {
    run(program, command, input, timeout, StdoutSink::Capture)
}

fn run(
    program: &str,
    mut command: Command,
    input: Option<&[u8]>,
    timeout: Duration,
    sink: StdoutSink,
) -> Result<Output, CommandError> {
    let token = operation::current_operation();
    if token.is_cancelled() {
        return Err(CommandError::Cancelled {
            program: program.to_string(),
        });
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if input.is_some() {
        command.stdin(Stdio::piped());
    } else {
        // The remote agent's stdin is the framed Repola protocol. No child is
        // allowed to inherit and consume it while attempting an interactive
        // prompt; authentication must go through configured Git/SSH helpers.
        command.stdin(Stdio::null());
    }
    let mut child = spawn_managed(command).map_err(|source| CommandError::Launch {
        program: program.to_string(),
        source,
    })?;
    let stdout = child.stdout().take().expect("captured stdout must exist");
    let stderr = child.stderr().take().expect("captured stderr must exist");
    let stdout_reader = thread::spawn(move || match sink {
        StdoutSink::Capture => Ok(read_bounded(stdout)),
        StdoutSink::File(file) => copy_into(stdout, file).map(|()| (Vec::new(), false)),
    });
    let stderr_reader = thread::spawn(move || read_bounded(stderr));
    let mut input_writer = input.map(|input| {
        let mut stdin = child.stdin().take().expect("piped stdin must exist");
        let input = input.to_vec();
        thread::spawn(move || stdin.write_all(&input))
    });
    let mut finish_input = || -> Result<(), std::io::Error> {
        let Some(writer) = input_writer.take() else {
            return Ok(());
        };
        writer
            .join()
            .map_err(|_| std::io::Error::other("command input writer panicked"))?
    };
    let started = Instant::now();
    let status = loop {
        if token.is_cancelled() {
            terminate_and_reap(&mut child);
            let _ = finish_input();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(CommandError::Cancelled {
                program: program.to_string(),
            });
        }
        if started.elapsed() >= timeout {
            terminate_and_reap(&mut child);
            let _ = finish_input();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(CommandError::Timeout {
                program: program.to_string(),
                seconds: timeout.as_secs(),
            });
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(source) => {
                terminate_and_reap(&mut child);
                let _ = finish_input();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(CommandError::Launch {
                    program: program.to_string(),
                    source,
                });
            }
        }
    };
    drop(child);
    if let Err(source) = finish_input() {
        let _ = stdout_reader.join();
        let _ = stderr_reader.join();
        return Err(CommandError::Launch {
            program: program.to_string(),
            source,
        });
    }
    let (stdout, stdout_truncated) = stdout_reader
        .join()
        .unwrap_or_else(|_| Err(std::io::Error::other("command output reader panicked")))
        .map_err(|source| CommandError::SaveOutput {
            program: program.to_string(),
            source,
        })?;
    let (stderr, stderr_truncated) = stderr_reader.join().unwrap_or_default();
    if stdout_truncated {
        return Err(CommandError::OutputTooLarge {
            program: program.to_string(),
            stream: "stdout",
            maximum: MAX_COMMAND_STREAM_BYTES,
        });
    }
    if stderr_truncated {
        return Err(CommandError::OutputTooLarge {
            program: program.to_string(),
            stream: "stderr",
            maximum: MAX_COMMAND_STREAM_BYTES,
        });
    }
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn copy_into<R: Read>(mut reader: R, mut file: std::fs::File) -> std::io::Result<()> {
    std::io::copy(&mut reader, &mut file)?;
    file.flush()
}

fn read_bounded<R: Read>(mut reader: R) -> (Vec<u8>, bool) {
    let mut captured = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let remaining = MAX_COMMAND_STREAM_BYTES.saturating_sub(captured.len());
                captured.extend_from_slice(&buffer[..read.min(remaining)]);
                truncated |= read > remaining;
            }
        }
    }
    (captured, truncated)
}

fn terminate_and_reap(child: &mut ManagedChild) {
    let _ = child.kill();
    let _ = child.wait();
}

pub fn output<I, S>(program: &str, args: I) -> Result<Output, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = command(program)?;
    command.args(args);
    launch(program, command, None)
}

pub(crate) fn output_with_input<I, S>(
    program: &str,
    args: I,
    input: &[u8],
) -> Result<Output, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = command(program)?;
    command.args(args);
    launch(program, command, Some(input))
}

pub(crate) fn spawn_piped<I, S>(program: &str, args: I) -> Result<ManagedChild, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    spawn_piped_in(None, program, args)
}

pub(crate) fn spawn_piped_at<I, S>(
    directory: &Path,
    program: &str,
    args: I,
) -> Result<ManagedChild, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    spawn_piped_in(Some(directory), program, args)
}

fn spawn_piped_in<I, S>(
    directory: Option<&Path>,
    program: &str,
    args: I,
) -> Result<ManagedChild, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = command(program)?;
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    spawn_managed(command).map_err(|source| CommandError::Launch {
        program: program.to_string(),
        source,
    })
}

pub fn output_at<I, S>(directory: &Path, program: &str, args: I) -> Result<Output, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = command(program)?;
    command.current_dir(directory).args(args);
    launch(program, command, None)
}

pub(crate) fn output_at_with_input_timeout<I, S>(
    directory: &Path,
    program: &str,
    args: I,
    input: &[u8],
    timeout: Duration,
) -> Result<Output, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = command(program)?;
    command.current_dir(directory).args(args);
    launch_with_timeout(program, command, Some(input), timeout)
}

pub fn git_at<I, S>(path: &Path, args: I) -> Result<Output, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = command("git")?;
    command.arg("-C").arg(path).args(args);
    launch("git", command, None)
}

pub fn git_at_with_input<I, S>(path: &Path, args: I, input: &[u8]) -> Result<Output, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = command("git")?;
    command.arg("-C").arg(path).args(args);
    launch("git", command, Some(input))
}

/// Like `git_at_with_input`, with a time limit of its own for a step that
/// handles content of any size.
pub(crate) fn git_at_with_input_timeout<I, S>(
    path: &Path,
    args: I,
    input: &[u8],
    timeout: Duration,
) -> Result<Output, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = command("git")?;
    command.arg("-C").arg(path).args(args);
    launch_with_timeout("git", command, Some(input), timeout)
}

pub fn git_at_with_env<I, S, E, K, V>(
    path: &Path,
    args: I,
    environment: E,
) -> Result<Output, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
    E: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut command = command("git")?;
    command.arg("-C").arg(path).args(args).envs(environment);
    launch("git", command, None)
}

pub fn git_at_with_input_and_env<I, S, E, K, V>(
    path: &Path,
    args: I,
    input: &[u8],
    environment: E,
) -> Result<Output, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
    E: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut command = command("git")?;
    command.arg("-C").arg(path).args(args).envs(environment);
    launch("git", command, Some(input))
}

/// Runs Git with stdout copied straight into `file` instead of captured, so
/// object contents larger than the capture bound can be saved. The returned
/// output carries stderr only.
pub fn git_at_to_file<I, S>(
    path: &Path,
    args: I,
    file: std::fs::File,
) -> Result<Output, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = command("git")?;
    command.arg("-C").arg(path).args(args);
    run(
        "git",
        command,
        None,
        COMMAND_TIMEOUT,
        StdoutSink::File(file),
    )
}

pub fn successful_git_at<I, S>(path: &Path, args: I) -> Result<Output, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let result = git_at(path, args)?;
    if result.status.success() {
        return Ok(result);
    }

    let message = String::from_utf8_lossy(&result.stderr).trim().to_string();
    Err(CommandError::Failed {
        program: "git".to_string(),
        status: result.status.code().unwrap_or(-1),
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_programs_surface_as_not_found_launch_errors() {
        let error = output("repola-definitely-not-installed", ["--version"])
            .expect_err("an absent program must not resolve");
        match error {
            CommandError::Launch { program, source } => {
                assert_eq!(program, "repola-definitely-not-installed");
                assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn resolved_programs_are_cached_by_name() {
        output("git", ["--version"]).expect("git is required for the test suite");
        let cache = resolved_programs().lock().expect("cache lock");
        assert!(cache.get("git").is_some_and(|path| path.is_absolute()));
    }

    #[test]
    fn git_commands_drop_inherited_pathspec_modes() {
        let removed = |program| {
            command(program)
                .expect("git is required for the test suite")
                .get_envs()
                .filter(|(_, value)| value.is_none())
                .map(|(key, _)| key.to_owned())
                .collect::<Vec<_>>()
        };
        for variable in INHERITED_PATHSPEC_MODES {
            assert!(removed("git").iter().any(|key| key == variable));
        }
        assert!(removed("cargo").is_empty());
    }

    #[test]
    fn cancelled_operations_do_not_launch_commands() {
        let token = crate::operation::OperationToken::new();
        token.cancel();
        let error = crate::operation::with_operation(token, || output("git", ["--version"]))
            .expect_err("cancelled command");
        assert!(matches!(error, CommandError::Cancelled { .. }));
    }

    // Re-execute this test binary through a .cmd shim, then spawn a descendant
    // that holds all three inherited pipes open. No installed provider needed.
    #[cfg(windows)]
    #[test]
    fn windows_process_fixture() {
        let Ok(role) = std::env::var("REPOLA_TEST_PROCESS_ROLE") else {
            return;
        };
        if role == "descendant" {
            std::fs::write(std::env::var_os("REPOLA_TEST_READY").unwrap(), "ready").unwrap();
        } else {
            let executable = std::env::current_exe().unwrap();
            let mut descendant = command(executable.to_str().unwrap()).unwrap();
            descendant
                .args([
                    "--exact",
                    "worktree::command::tests::windows_process_fixture",
                    "--nocapture",
                ])
                .env("REPOLA_TEST_PROCESS_ROLE", "descendant");
            // Deliberately leave this child running: the outer test must
            // prove that job termination cleans up an unwaited descendant.
            #[allow(clippy::zombie_processes)]
            let _child = descendant.spawn().unwrap();
            if role == "exit-first" {
                return;
            }
        }
        thread::sleep(Duration::from_secs(60));
    }

    #[cfg(windows)]
    fn windows_shim_fixture(role: &str) -> (tempfile::TempDir, Command) {
        let directory = tempfile::Builder::new()
            .prefix("repola process tree ")
            .tempdir()
            .unwrap();
        let shim = directory.path().join("provider.cmd");
        std::fs::write(&shim, "@\"%REPOLA_TEST_EXE%\" --exact worktree::command::tests::windows_process_fixture --nocapture\r\n").unwrap();
        let mut command = command(shim.to_str().unwrap()).unwrap();
        command
            .env("REPOLA_TEST_EXE", std::env::current_exe().unwrap())
            .env("REPOLA_TEST_PROCESS_ROLE", role)
            .env("REPOLA_TEST_READY", directory.path().join("ready"));
        (directory, command)
    }

    #[cfg(windows)]
    #[test]
    fn windows_timeout_terminates_shim_descendants_and_releases_pipes() {
        let (directory, command) = windows_shim_fixture("parent");
        let started = Instant::now();
        // Enough input to block the writer while the fixture holds stdin open.
        let error = launch_with_timeout(
            "fixture",
            command,
            Some(&vec![b'x'; 1024 * 1024]),
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(matches!(error, CommandError::Timeout { .. }));
        assert!(
            directory.path().join("ready").exists(),
            "descendant must have started"
        );
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[cfg(windows)]
    #[test]
    fn windows_cancellation_terminates_shim_descendants_and_releases_pipes() {
        let (directory, command) = windows_shim_fixture("parent");
        let ready = directory.path().join("ready");
        let token = operation::OperationToken::new();
        let canceller = token.clone();
        let cancel = thread::spawn(move || {
            let started = Instant::now();
            while !ready.exists() && started.elapsed() < Duration::from_secs(10) {
                thread::sleep(Duration::from_millis(10));
            }
            let ready = ready.exists();
            canceller.cancel();
            ready
        });
        let started = Instant::now();
        let result = operation::with_operation(token, || {
            launch_with_timeout(
                "fixture",
                command,
                Some(&vec![b'x'; 1024 * 1024]),
                Duration::from_secs(30),
            )
        });
        assert!(
            cancel.join().unwrap(),
            "descendant must have started before cancellation"
        );
        assert!(matches!(result, Err(CommandError::Cancelled { .. })));
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[cfg(windows)]
    #[test]
    fn windows_shim_exit_cleans_up_descendants_before_reading_output() {
        let (_directory, command) = windows_shim_fixture("exit-first");
        let started = Instant::now();
        let output =
            launch_with_timeout("fixture", command, None, Duration::from_secs(10)).unwrap();
        assert!(output.status.success());
        assert!(started.elapsed() < Duration::from_secs(20));
    }
}
