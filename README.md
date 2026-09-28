<h1 align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Repola icon" width="128" height="128">
  <br><span style="font-family: monospace;">Repola</span>
</h1>

<p align="center">
  A cross-platform Git workbench for local and SSH-hosted working copies.
</p>

<p align="center">
  <a href="https://v2.tauri.app"><img alt="Tauri 2" src="https://img.shields.io/badge/Tauri%202-24C8D8?logo=tauri&logoColor=white"></a>
  <a href="https://www.rust-lang.org"><img alt="Rust" src="https://img.shields.io/badge/Rust-000000?logo=rust&logoColor=white"></a>
  <a href="https://react.dev"><img alt="React 19" src="https://img.shields.io/badge/React%2019-61DAFB?logo=react&logoColor=black"></a>
  <a href="https://www.typescriptlang.org"><img alt="TypeScript 6" src="https://img.shields.io/badge/TypeScript%206-3178C6?logo=typescript&logoColor=white"></a>
</p>

## About

Repola combines the daily workflow of a native Git client with repository-wide worktree inventory and cleanup. Your local computer and every SSH machine run the same `repola-agent`, so remote working copies get the same Git features as local ones, with no mounted filesystems or shadow clones.

Repola only lists repositories you add, clone, create, or drop into it. It never crawls folders or guesses where your code lives.

## Features

- **Changes**: Stage and discard by file, hunk, or line; commit, amend, and undo; fetch, pull, and push
- **History**: Browse and search commits, compare branches, and merge, rebase, cherry-pick, revert, and reset with reflog-based undo points
- **Worktrees**: See every worktree's changes, unpushed commits, disk usage, recent activity, and agent origin, then create, switch, or clean them up
- **SSH machines**: Work on remote repositories through your system OpenSSH config, with a signed agent installed automatically
- **Pull requests**: Show GitHub and Azure DevOps pull-request, review, and check context, and create GitHub pull requests
- **AI commit messages**: Draft commit messages from the selected changes with Codex or Claude Code
- **Desktop integration**: Open worktrees in your editor, terminal, or file manager, with a command palette and keyboard shortcuts

## Safety

- Primary, dirty, detached, locked, and uninspectable worktrees are protected from removal.
- Worktree removal never uses `--force` and never deletes branches.
- Every destructive action is previewed, then revalidated against a fresh fingerprint right before it runs.
- Git receives exact arguments; no shell ever evaluates a path.
- Commit containment is shown as evidence. Repola never infers that a pull request merged from ancestry alone.
- Every action attempt is logged to `actions.jsonl` in the app data directory.

## Development

Running from source requires [Rust](https://rustup.rs/), [Node.js](https://nodejs.org), [pnpm](https://pnpm.io), and Git.

```sh
pnpm install
pnpm tauri dev        # run the app
pnpm tauri build      # build a native bundle for this platform
```

Validation (CI runs the same suite on Ubuntu, macOS, and Windows):

```sh
pnpm typecheck
pnpm lint
pnpm test
pnpm build

cd src-tauri
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

The `repola-cli` example prints a read-only JSON inventory of the repositories you pass it:

```sh
cd src-tauri
cargo run --package repola-engine --example repola-cli -- /path/to/repository
```

## Architecture

- **`src/`**: React and TypeScript frontend
- **`src-tauri/`**: Tauri app crate with thin IPC commands and persisted settings
- **`src-tauri/crates/repola-engine`**: Git, filesystem, process, protocol, and SSH behavior with no Tauri dependency; builds the `repola-agent` binary
- **Protocol**: Requests to the local machine run in process; requests to SSH machines use length-prefixed JSON over OpenSSH stdio

See [`docs/architecture.md`](docs/architecture.md) for process boundaries and safety invariants, [`docs/product-requirements.md`](docs/product-requirements.md) for the acceptance matrix, and [`docs/releasing.md`](docs/releasing.md) for release signing.
