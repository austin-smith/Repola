use std::ffi::OsString;
#[cfg(not(target_os = "macos"))]
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::command;
use crate::settings::{AppPreferences, MachineKind, MachineProfile};

const DISCOVERY_CACHE_TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorktreeTool {
    Editor,
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalTool {
    pub id: String,
    pub label: String,
    pub supports_remote_workspaces: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalToolAvailability {
    pub editors: Vec<ExternalTool>,
    pub terminals: Vec<ExternalTool>,
}

#[derive(Clone)]
struct CachedAvailability {
    discovered_at: Instant,
    availability: ExternalToolAvailability,
}

fn discovery_cache() -> &'static Mutex<Option<CachedAvailability>> {
    static CACHE: OnceLock<Mutex<Option<CachedAvailability>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

#[derive(Debug)]
struct EditorDefinition {
    id: &'static str,
    label: &'static str,
    commands: &'static [&'static str],
    base_args: &'static [&'static str],
    remote_workspace: bool,
    #[cfg(target_os = "macos")]
    mac_app_names: &'static [&'static str],
    #[cfg(target_os = "macos")]
    mac_cli_paths: &'static [&'static str],
}

macro_rules! editor {
    ($id:literal, $label:literal, [$($command:literal),* $(,)?], [$($base_arg:literal),* $(,)?], $remote:literal, [$($mac_app:literal),* $(,)?], [$($mac_cli:literal),* $(,)?]) => {
        EditorDefinition {
            id: $id,
            label: $label,
            commands: &[$($command),*],
            base_args: &[$($base_arg),*],
            remote_workspace: $remote,
            #[cfg(target_os = "macos")]
            mac_app_names: &[$($mac_app),*],
            #[cfg(target_os = "macos")]
            mac_cli_paths: &[$($mac_cli),*],
        }
    };
}

// Ordered by the default preference used when no saved choice is available.
// The core registry follows T3 Code's capability model and adds established
// GitHub Desktop integrations that can reliably open a repository directory.
static EDITORS: &[EditorDefinition] = &[
    editor!(
        "cursor",
        "Cursor",
        ["cursor"],
        [],
        true,
        ["Cursor"],
        ["/Applications/Cursor.app/Contents/Resources/app/bin/cursor"]
    ),
    editor!(
        "vscode",
        "Visual Studio Code",
        ["code"],
        [],
        true,
        ["Visual Studio Code"],
        ["/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code"]
    ),
    editor!(
        "vscode-insiders",
        "Visual Studio Code Insiders",
        ["code-insiders"],
        [],
        true,
        ["Visual Studio Code - Insiders"],
        ["/Applications/Visual Studio Code - Insiders.app/Contents/Resources/app/bin/code"]
    ),
    editor!(
        "vscodium",
        "VSCodium",
        ["codium"],
        [],
        true,
        ["VSCodium"],
        ["/Applications/VSCodium.app/Contents/Resources/app/bin/codium"]
    ),
    editor!(
        "zed",
        "Zed",
        ["zed", "zeditor"],
        [],
        false,
        ["Zed"],
        ["/Applications/Zed.app/Contents/MacOS/cli"]
    ),
    editor!("trae", "Trae", ["trae"], [], false, ["Trae"], []),
    editor!("kiro", "Kiro", ["kiro"], ["ide"], false, ["Kiro"], []),
    editor!(
        "antigravity",
        "Antigravity",
        ["agy"],
        [],
        false,
        ["Antigravity"],
        []
    ),
    editor!(
        "idea",
        "IntelliJ IDEA",
        ["idea"],
        [],
        false,
        [
            "IntelliJ IDEA",
            "IntelliJ IDEA Ultimate",
            "IntelliJ IDEA CE"
        ],
        []
    ),
    editor!("aqua", "Aqua", ["aqua"], [], false, ["Aqua"], []),
    editor!("clion", "CLion", ["clion"], [], false, ["CLion"], []),
    editor!(
        "datagrip",
        "DataGrip",
        ["datagrip"],
        [],
        false,
        ["DataGrip"],
        []
    ),
    editor!(
        "dataspell",
        "DataSpell",
        ["dataspell"],
        [],
        false,
        ["DataSpell"],
        []
    ),
    editor!("goland", "GoLand", ["goland"], [], false, ["GoLand"], []),
    editor!(
        "phpstorm",
        "PhpStorm",
        ["phpstorm"],
        [],
        false,
        ["PhpStorm"],
        []
    ),
    editor!(
        "pycharm",
        "PyCharm",
        ["pycharm"],
        [],
        false,
        ["PyCharm", "PyCharm Professional", "PyCharm CE"],
        []
    ),
    editor!("rider", "Rider", ["rider"], [], false, ["Rider"], []),
    editor!(
        "rubymine",
        "RubyMine",
        ["rubymine"],
        [],
        false,
        ["RubyMine"],
        []
    ),
    editor!(
        "rustrover",
        "RustRover",
        ["rustrover"],
        [],
        false,
        ["RustRover"],
        []
    ),
    editor!(
        "webstorm",
        "WebStorm",
        ["webstorm"],
        [],
        false,
        ["WebStorm"],
        []
    ),
    editor!(
        "android-studio",
        "Android Studio",
        ["studio"],
        [],
        false,
        ["Android Studio"],
        []
    ),
    editor!("fleet", "Fleet", ["fleet"], [], false, ["Fleet"], []),
    editor!(
        "sublime-text",
        "Sublime Text",
        ["subl"],
        [],
        false,
        ["Sublime Text"],
        ["/Applications/Sublime Text.app/Contents/SharedSupport/bin/subl"]
    ),
    editor!(
        "bbedit",
        "BBEdit",
        ["bbedit"],
        [],
        false,
        ["BBEdit"],
        ["/Applications/BBEdit.app/Contents/Helpers/bbedit_tool"]
    ),
    editor!("nova", "Nova", ["nova"], [], false, ["Nova"], []),
    editor!("xcode", "Xcode", ["xed"], [], false, ["Xcode"], []),
];

#[derive(Debug, Clone, Copy)]
enum TerminalLaunchStyle {
    #[cfg(not(target_os = "macos"))]
    InheritWorkingDirectory,
    Prefix(&'static [&'static str]),
    Joined(&'static str),
    #[cfg(target_os = "macos")]
    MacOpen,
}

#[derive(Debug)]
struct TerminalDefinition {
    id: &'static str,
    label: &'static str,
    commands: &'static [&'static str],
    base_args: &'static [&'static str],
    style: TerminalLaunchStyle,
    #[cfg(target_os = "macos")]
    mac_app_names: &'static [&'static str],
    #[cfg(target_os = "macos")]
    mac_cli_paths: &'static [&'static str],
}

#[cfg(target_os = "macos")]
static TERMINALS: &[TerminalDefinition] = &[
    TerminalDefinition {
        id: "terminal",
        label: "Terminal",
        commands: &[],
        base_args: &[],
        style: TerminalLaunchStyle::MacOpen,
        mac_app_names: &["Terminal"],
        mac_cli_paths: &[],
    },
    TerminalDefinition {
        id: "ghostty",
        label: "Ghostty",
        commands: &["ghostty"],
        base_args: &[],
        style: TerminalLaunchStyle::Joined("--working-directory="),
        mac_app_names: &["Ghostty"],
        mac_cli_paths: &["/Applications/Ghostty.app/Contents/MacOS/ghostty"],
    },
    TerminalDefinition {
        id: "iterm2",
        label: "iTerm2",
        commands: &[],
        base_args: &[],
        style: TerminalLaunchStyle::MacOpen,
        mac_app_names: &["iTerm", "iTerm2"],
        mac_cli_paths: &[],
    },
    TerminalDefinition {
        id: "warp",
        label: "Warp",
        commands: &[],
        base_args: &[],
        style: TerminalLaunchStyle::MacOpen,
        mac_app_names: &["Warp"],
        mac_cli_paths: &[],
    },
    TerminalDefinition {
        id: "wezterm",
        label: "WezTerm",
        commands: &["wezterm"],
        base_args: &[],
        style: TerminalLaunchStyle::Prefix(&["start", "--cwd"]),
        mac_app_names: &["WezTerm"],
        mac_cli_paths: &["/Applications/WezTerm.app/Contents/MacOS/wezterm"],
    },
    TerminalDefinition {
        id: "kitty",
        label: "kitty",
        commands: &["kitty"],
        base_args: &[],
        style: TerminalLaunchStyle::Prefix(&["--directory"]),
        mac_app_names: &["kitty"],
        mac_cli_paths: &["/Applications/kitty.app/Contents/MacOS/kitty"],
    },
    TerminalDefinition {
        id: "alacritty",
        label: "Alacritty",
        commands: &["alacritty"],
        base_args: &[],
        style: TerminalLaunchStyle::Prefix(&["--working-directory"]),
        mac_app_names: &["Alacritty"],
        mac_cli_paths: &["/Applications/Alacritty.app/Contents/MacOS/alacritty"],
    },
];

#[cfg(target_os = "windows")]
static TERMINALS: &[TerminalDefinition] = &[
    TerminalDefinition {
        id: "windows-terminal",
        label: "Windows Terminal",
        commands: &["wt"],
        base_args: &[],
        style: TerminalLaunchStyle::Prefix(&["-d"]),
    },
    TerminalDefinition {
        id: "powershell-core",
        label: "PowerShell",
        commands: &["pwsh"],
        base_args: &["-NoExit"],
        style: TerminalLaunchStyle::InheritWorkingDirectory,
    },
    TerminalDefinition {
        id: "powershell",
        label: "Windows PowerShell",
        commands: &["powershell"],
        base_args: &["-NoExit"],
        style: TerminalLaunchStyle::InheritWorkingDirectory,
    },
    TerminalDefinition {
        id: "command-prompt",
        label: "Command Prompt",
        commands: &["cmd"],
        base_args: &["/K"],
        style: TerminalLaunchStyle::InheritWorkingDirectory,
    },
    TerminalDefinition {
        id: "ghostty",
        label: "Ghostty",
        commands: &["ghostty"],
        base_args: &[],
        style: TerminalLaunchStyle::Joined("--working-directory="),
    },
    TerminalDefinition {
        id: "wezterm",
        label: "WezTerm",
        commands: &["wezterm"],
        base_args: &[],
        style: TerminalLaunchStyle::Prefix(&["start", "--cwd"]),
    },
    TerminalDefinition {
        id: "kitty",
        label: "kitty",
        commands: &["kitty"],
        base_args: &[],
        style: TerminalLaunchStyle::Prefix(&["--directory"]),
    },
    TerminalDefinition {
        id: "alacritty",
        label: "Alacritty",
        commands: &["alacritty"],
        base_args: &[],
        style: TerminalLaunchStyle::Prefix(&["--working-directory"]),
    },
];

#[cfg(all(unix, not(target_os = "macos")))]
static TERMINALS: &[TerminalDefinition] = &[
    TerminalDefinition {
        id: "default-terminal",
        label: "Default terminal",
        commands: &["x-terminal-emulator"],
        base_args: &[],
        style: TerminalLaunchStyle::InheritWorkingDirectory,
    },
    TerminalDefinition {
        id: "ghostty",
        label: "Ghostty",
        commands: &["ghostty"],
        base_args: &[],
        style: TerminalLaunchStyle::Joined("--working-directory="),
    },
    TerminalDefinition {
        id: "wezterm",
        label: "WezTerm",
        commands: &["wezterm"],
        base_args: &[],
        style: TerminalLaunchStyle::Prefix(&["start", "--cwd"]),
    },
    TerminalDefinition {
        id: "kitty",
        label: "kitty",
        commands: &["kitty"],
        base_args: &[],
        style: TerminalLaunchStyle::Prefix(&["--directory"]),
    },
    TerminalDefinition {
        id: "alacritty",
        label: "Alacritty",
        commands: &["alacritty"],
        base_args: &[],
        style: TerminalLaunchStyle::Prefix(&["--working-directory"]),
    },
    TerminalDefinition {
        id: "gnome-terminal",
        label: "GNOME Terminal",
        commands: &["gnome-terminal"],
        base_args: &[],
        style: TerminalLaunchStyle::Joined("--working-directory="),
    },
    TerminalDefinition {
        id: "konsole",
        label: "Konsole",
        commands: &["konsole"],
        base_args: &[],
        style: TerminalLaunchStyle::Prefix(&["--workdir"]),
    },
    TerminalDefinition {
        id: "xfce-terminal",
        label: "Xfce Terminal",
        commands: &["xfce4-terminal"],
        base_args: &[],
        style: TerminalLaunchStyle::Joined("--working-directory="),
    },
    TerminalDefinition {
        id: "foot",
        label: "foot",
        commands: &["foot"],
        base_args: &[],
        style: TerminalLaunchStyle::Prefix(&["--working-directory"]),
    },
];

#[derive(Debug, Clone)]
enum ResolvedLauncher {
    Executable(PathBuf),
    #[cfg(target_os = "macos")]
    MacApplication(&'static str),
}

impl ResolvedLauncher {
    fn executable(&self) -> Option<&PathBuf> {
        match self {
            Self::Executable(path) => Some(path),
            #[cfg(target_os = "macos")]
            Self::MacApplication(_) => None,
        }
    }
}

#[derive(Debug, Clone)]
struct ResolvedEditor {
    definition: &'static EditorDefinition,
    launcher: ResolvedLauncher,
}

#[derive(Debug, Clone)]
struct ResolvedTerminal {
    definition: &'static TerminalDefinition,
    launcher: ResolvedLauncher,
}

impl ResolvedEditor {
    fn supports_remote_workspaces(&self) -> bool {
        self.definition.remote_workspace && self.launcher.executable().is_some()
    }

    fn option(&self) -> ExternalTool {
        ExternalTool {
            id: self.definition.id.to_string(),
            label: self.definition.label.to_string(),
            supports_remote_workspaces: self.supports_remote_workspaces(),
        }
    }
}

impl ResolvedTerminal {
    fn option(&self) -> ExternalTool {
        ExternalTool {
            id: self.definition.id.to_string(),
            label: self.definition.label.to_string(),
            supports_remote_workspaces: false,
        }
    }
}

pub fn available_external_tools() -> ExternalToolAvailability {
    if let Ok(cache) = discovery_cache().lock() {
        if let Some(cached) = cache.as_ref() {
            if cached.discovered_at.elapsed() < DISCOVERY_CACHE_TTL {
                return cached.availability.clone();
            }
        }
    }

    let availability = discover_external_tools();
    if let Ok(mut cache) = discovery_cache().lock() {
        *cache = Some(CachedAvailability {
            discovered_at: Instant::now(),
            availability: availability.clone(),
        });
    }
    availability
}

fn discover_external_tools() -> ExternalToolAvailability {
    ExternalToolAvailability {
        editors: resolved_editors()
            .into_iter()
            .map(|editor| editor.option())
            .collect(),
        terminals: resolved_terminals()
            .into_iter()
            .map(|terminal| terminal.option())
            .collect(),
    }
}

fn resolved_editors() -> Vec<ResolvedEditor> {
    EDITORS.iter().filter_map(resolve_editor).collect()
}

fn resolved_terminals() -> Vec<ResolvedTerminal> {
    TERMINALS.iter().filter_map(resolve_terminal).collect()
}

fn resolve_editor(definition: &'static EditorDefinition) -> Option<ResolvedEditor> {
    resolve_launcher(
        definition.commands,
        #[cfg(target_os = "macos")]
        definition.mac_cli_paths,
        #[cfg(target_os = "macos")]
        definition.mac_app_names,
    )
    .map(|launcher| ResolvedEditor {
        definition,
        launcher,
    })
}

fn resolve_terminal(definition: &'static TerminalDefinition) -> Option<ResolvedTerminal> {
    resolve_launcher(
        definition.commands,
        #[cfg(target_os = "macos")]
        definition.mac_cli_paths,
        #[cfg(target_os = "macos")]
        definition.mac_app_names,
    )
    .map(|launcher| ResolvedTerminal {
        definition,
        launcher,
    })
}

fn resolve_launcher(
    commands: &[&str],
    #[cfg(target_os = "macos")] mac_cli_paths: &[&str],
    #[cfg(target_os = "macos")] mac_app_names: &[&'static str],
) -> Option<ResolvedLauncher> {
    if let Some(path) = commands
        .iter()
        .find_map(|command| command::find_program(command))
    {
        return Some(ResolvedLauncher::Executable(path));
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(path) = mac_cli_paths
            .iter()
            .map(PathBuf::from)
            .find(|path| path.is_file())
        {
            return Some(ResolvedLauncher::Executable(path));
        }
        if let Some(name) = mac_app_names
            .iter()
            .copied()
            .find(|name| macos_application_available(name))
        {
            return Some(ResolvedLauncher::MacApplication(name));
        }
    }

    None
}

#[cfg(target_os = "macos")]
fn macos_application_available(name: &str) -> bool {
    command::output("open", ["-Ra", name])
        .map(|output| output.status.success())
        .unwrap_or(false)
}

pub fn launch_worktree_tool(
    machine: &MachineProfile,
    preferences: &AppPreferences,
    path: &str,
    tool: WorktreeTool,
) -> Result<String, String> {
    validate_path(path, machine.kind == MachineKind::Local)?;
    match (machine.kind, tool) {
        (MachineKind::Local, WorktreeTool::Editor) => {
            let editor = choose_editor(preferences.editor_id.as_deref(), false)?;
            launch_local_editor(&editor, path)?;
            Ok(format!("Opened {path} in {}.", editor.definition.label))
        }
        (MachineKind::Local, WorktreeTool::Terminal) => {
            let terminal = choose_terminal(preferences.terminal_id.as_deref())?;
            launch_local_terminal(&terminal, path)?;
            Ok(format!("Opened {path} in {}.", terminal.definition.label))
        }
        (MachineKind::Ssh, WorktreeTool::Editor) => {
            let editor = choose_editor(preferences.editor_id.as_deref(), true)?;
            launch_remote_editor(machine, &editor, path)?;
            Ok(format!(
                "Opened {path} in {} through its SSH workspace support.",
                editor.definition.label
            ))
        }
        (MachineKind::Ssh, WorktreeTool::Terminal) => Err(
            "Repola does not open an interactive remote shell. Use your terminal's SSH workflow; repository commands continue to run through the bounded Repola agent protocol.".into(),
        ),
    }
}

fn choose_editor(preferred_id: Option<&str>, remote: bool) -> Result<ResolvedEditor, String> {
    let editors: Vec<_> = resolved_editors()
        .into_iter()
        .filter(|editor| !remote || editor.supports_remote_workspaces())
        .collect();
    let selected = preferred_id
        .and_then(|id| {
            editors
                .iter()
                .find(|editor| editor.definition.id == id)
                .cloned()
        })
        .or_else(|| editors.first().cloned());
    selected.ok_or_else(|| {
        if remote {
            "No installed editor with SSH workspace support was found. Install Cursor, Visual Studio Code, Visual Studio Code Insiders, or VSCodium.".into()
        } else {
            "No supported editor was found on this computer.".into()
        }
    })
}

fn choose_terminal(preferred_id: Option<&str>) -> Result<ResolvedTerminal, String> {
    let terminals = resolved_terminals();
    preferred_id
        .and_then(|id| {
            terminals
                .iter()
                .find(|terminal| terminal.definition.id == id)
                .cloned()
        })
        .or_else(|| terminals.first().cloned())
        .ok_or_else(|| "No supported terminal was found on this computer.".into())
}

fn launch_local_editor(editor: &ResolvedEditor, path: &str) -> Result<(), String> {
    match &editor.launcher {
        ResolvedLauncher::Executable(program) => {
            let args = editor_local_arguments(editor.definition, path);
            command::spawn_detached(program, &args, None).map_err(|error| error.to_string())
        }
        #[cfg(target_os = "macos")]
        ResolvedLauncher::MacApplication(name) => launch_macos_application(name, path),
    }
}

fn launch_remote_editor(
    machine: &MachineProfile,
    editor: &ResolvedEditor,
    path: &str,
) -> Result<(), String> {
    let host = &machine
        .ssh
        .as_ref()
        .ok_or_else(|| "The SSH machine profile is incomplete.".to_string())?
        .host;
    let program = editor.launcher.executable().ok_or_else(|| {
        format!(
            "{} is installed but its command-line launcher is unavailable, so it cannot open a remote worktree.",
            editor.definition.label
        )
    })?;
    let destination = format!("ssh-remote+{host}");
    let args = [
        OsString::from("--remote"),
        OsString::from(destination),
        OsString::from(path),
    ];
    command::spawn_detached(program, args, None).map_err(|error| error.to_string())
}

fn editor_local_arguments(definition: &EditorDefinition, path: &str) -> Vec<OsString> {
    definition
        .base_args
        .iter()
        .map(OsString::from)
        .chain(std::iter::once(OsString::from(path)))
        .collect()
}

fn launch_local_terminal(terminal: &ResolvedTerminal, path: &str) -> Result<(), String> {
    match &terminal.launcher {
        ResolvedLauncher::Executable(program) => {
            let args = terminal_arguments(terminal.definition, path);
            #[cfg(target_os = "macos")]
            let working_directory = None;
            #[cfg(not(target_os = "macos"))]
            let working_directory = matches!(
                terminal.definition.style,
                TerminalLaunchStyle::InheritWorkingDirectory
            )
            .then_some(Path::new(path));
            command::spawn_detached(program, args, working_directory)
                .map_err(|error| error.to_string())
        }
        #[cfg(target_os = "macos")]
        ResolvedLauncher::MacApplication(name) => launch_macos_application(name, path),
    }
}

fn terminal_arguments(definition: &TerminalDefinition, path: &str) -> Vec<OsString> {
    let mut args: Vec<OsString> = definition.base_args.iter().map(OsString::from).collect();
    match definition.style {
        #[cfg(not(target_os = "macos"))]
        TerminalLaunchStyle::InheritWorkingDirectory => {}
        TerminalLaunchStyle::Prefix(prefix) => {
            args.extend(prefix.iter().map(OsString::from));
            args.push(OsString::from(path));
        }
        TerminalLaunchStyle::Joined(prefix) => args.push(OsString::from(format!("{prefix}{path}"))),
        #[cfg(target_os = "macos")]
        TerminalLaunchStyle::MacOpen => {}
    }
    args
}

#[cfg(target_os = "macos")]
fn launch_macos_application(name: &str, path: &str) -> Result<(), String> {
    let open = command::resolve_program("open").map_err(|error| error.to_string())?;
    command::spawn_detached(&open, ["-a", name, path], None).map_err(|error| error.to_string())
}

fn validate_path(path: &str, local: bool) -> Result<(), String> {
    if path.is_empty() || path.len() > 32 * 1024 || path.chars().any(char::is_control) {
        return Err("The worktree path is invalid.".into());
    }
    if local {
        let metadata = std::fs::metadata(path)
            .map_err(|error| format!("Could not inspect the worktree path: {error}"))?;
        if !metadata.is_dir() {
            return Err("The selected worktree path is not a directory.".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn editor_and_terminal_ids_are_unique_and_human_labeled() {
        let mut ids = HashSet::new();
        for editor in EDITORS {
            assert!(
                ids.insert(("editor", editor.id)),
                "duplicate editor {}",
                editor.id
            );
            assert!(!editor.label.trim().is_empty());
        }
        for terminal in TERMINALS {
            assert!(
                ids.insert(("terminal", terminal.id)),
                "duplicate terminal {}",
                terminal.id
            );
            assert!(!terminal.label.trim().is_empty());
        }
    }

    #[test]
    fn editor_arguments_preserve_paths_as_one_argument() {
        let kiro = EDITORS
            .iter()
            .find(|editor| editor.id == "kiro")
            .expect("Kiro");
        assert_eq!(
            editor_local_arguments(kiro, "/tmp/project with spaces"),
            vec![
                OsString::from("ide"),
                OsString::from("/tmp/project with spaces")
            ]
        );
    }

    #[test]
    fn terminal_arguments_preserve_paths_as_one_argument() {
        let terminal = TerminalDefinition {
            id: "test",
            label: "Test",
            commands: &["test-terminal"],
            base_args: &["start", "--cwd"],
            style: TerminalLaunchStyle::Prefix(&["--new-window"]),
            #[cfg(target_os = "macos")]
            mac_app_names: &[],
            #[cfg(target_os = "macos")]
            mac_cli_paths: &[],
        };
        assert_eq!(
            terminal_arguments(&terminal, "/tmp/project with spaces"),
            vec![
                OsString::from("start"),
                OsString::from("--cwd"),
                OsString::from("--new-window"),
                OsString::from("/tmp/project with spaces"),
            ]
        );
    }

    #[test]
    fn local_paths_must_exist_but_remote_paths_are_never_resolved_locally() {
        assert!(validate_path("/definitely/not/a/local/repola/path", true).is_err());
        assert!(validate_path("/srv/project", false).is_ok());
        assert!(validate_path("bad\npath", false).is_err());
    }
}
