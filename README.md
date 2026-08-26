# Repola

A cross-platform Git workbench for local repositories and working copies hosted on machines reached through SSH. Repola combines the focused daily workflow of a native Git client with safe, repository-wide worktree inventory and management.

Repola is being built around one context hierarchy: machine → repository → worktree → Git operation. The built-in local machine and SSH machines share a versioned agent protocol, so local and remote working copies receive the same Git behavior rather than separate feature sets.

The inherited worktree engine inspects explicitly added repositories and their linked-worktree registries, inventories every registered worktree, measures allocated disk usage, inspects local changes, and presents explicit commit-containment evidence. It deliberately does not call a worktree “merged” based on ancestry alone.

The app never guesses where your code lives and never crawls parent folders. A repository enters the list only when you explicitly add, clone, create, or drag it into Repola. The same rule applies independently to every configured machine. Machine profiles, the per-machine repository list, workspace context, window and pane layout, filters, theme, editor, terminal, and Git preferences are stored as independently versioned records in Repola's platform app-data directory. A registered repository that is temporarily missing or unreadable is reported and skipped, never silently replaced with guessed repositories.

Age filters and ordering use the latest observed activity from the worktree’s files, its HEAD commit, or the folder creation timestamp—not folder creation alone. A `≥` size means some entries could not be measured and the displayed allocation is a lower bound.

## Safety model

- Primary, dirty, detached, locked, and uninspectable worktrees are protected from normal removal.
- Removal uses `git worktree remove <path>` and never adds `--force`.
- Branches are retained when a worktree is removed.
- Every action gets a fresh Rust-side preflight and is revalidated after confirmation.
- The revalidation fingerprint includes the worktree path, repository, HEAD, branch, and affected paths.
- Commands receive exact arguments directly; no shell evaluates worktree paths.
- Repository-wide pruning previews every registration Git currently marks prunable.
- Successful and failed execution attempts are appended to `actions.jsonl` in the app data directory.
- Commit containment is shown as evidence, not pull-request status.

## Architecture

- Tauri 2 provides the native shell and typed command boundary on macOS, Windows, and Linux.
- A packaged `repola-agent` owns Git and filesystem operations on the machine containing a working copy. Local requests use the same typed envelopes in process; remote requests use length-prefixed JSON over system OpenSSH stdio.
- SSH profiles reference OpenSSH configuration and never store keys or passphrases. Repola disables agent forwarding and exposes no generic remote-command endpoint.
- Persistent settings (including each machine's explicit repository list) go through `tauri-plugin-store`, owned by `src-tauri/src/settings.rs`; the frontend never reads or writes them directly.
- Every child process (`git`, `gh`, `az`) is resolved on `PATH` the way the platform shell would (including `PATHEXT` shims on Windows) and spawned without a console window; see `src-tauri/src/worktree/command.rs`.
- Rust owns discovery, Git execution, status parsing, sizing, safety classification, actions, and auditing.
- Agent detection is defined by one backend registry in `src-tauri/src/worktree/agents.rs`; the API returns generic, self-describing origin data and the frontend contains no agent-specific names or path rules.
- React and TypeScript provide the inventory workbench, filters, evidence inspector, and confirmation flow.
- The `repola-cli` example exposes the same read-only scanner as JSON for diagnostics without becoming part of the app bundle.

## Development

Requirements: a current Rust toolchain, Node.js, pnpm, and Git.

```bash
pnpm install
pnpm tauri dev
```

Run the complete validation suite:

```bash
pnpm typecheck
pnpm lint
pnpm test
pnpm build
cd src-tauri
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

CI runs the same suite on Ubuntu, macOS, and Windows (`.github/workflows/ci.yml`).

Build a native bundle for the current platform:

```bash
pnpm tauri build
```

Run a read-only JSON inventory from the command line:

```bash
cd src-tauri
cargo run --example repola-cli -- /path/to/repository
```

The CLI takes exact Git repository paths as arguments; with none it exits with an error rather than scanning a guessed location.

The complete product acceptance matrix lives in [`docs/product-requirements.md`](docs/product-requirements.md), with process boundaries and safety invariants in [`docs/architecture.md`](docs/architecture.md).
