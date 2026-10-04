<h1 align="center">
  <img src="src-tauri/assets/icons/128x128@2x.png" alt="Repola icon" width="128" height="128">
  <br><span style="font-family: monospace;">Repola</span>
</h1>

<p align="center">
  Cross-platform Git client
</p>

<p align="center">
  <a href="https://tauri.app"><img alt="Tauri" src="https://img.shields.io/badge/Tauri-24C8D8?logo=tauri&logoColor=white"></a>
  <a href="https://www.rust-lang.org"><img alt="Rust" src="https://img.shields.io/badge/Rust-000000?logo=rust&logoColor=white"></a>
  <a href="https://react.dev"><img alt="React" src="https://img.shields.io/badge/React-61DAFB?logo=react&logoColor=black"></a>
  <a href="https://www.typescriptlang.org"><img alt="TypeScript" src="https://img.shields.io/badge/TypeScript-3178C6?logo=typescript&logoColor=white"></a>
</p>

## About

Cross-platform Git client. Review and commit changes, browse history, and manage every worktree in a repository — locally or over SSH.

## Features

- **Changes**: Stage, discard, and commit files, hunks, or lines
- **History**: Search commits, compare branches, merge, rebase, and cherry-pick
- **Worktrees**: Create, switch, and clean up worktrees across a repository
- **SSH**: Work with repositories on remote machines
- **Pull requests**: View and create GitHub pull requests
- **AI commit messages**: Generate commit messages with Codex or Claude Code
- **Desktop integration**: Open a worktree in your editor, terminal, or file manager

## Development

Running from source requires [Rust](https://rustup.rs/), [Node.js](https://nodejs.org), [pnpm](https://pnpm.io), and Git.

```sh
pnpm install
pnpm tauri dev      # run the app
pnpm tauri build    # build a native bundle
pnpm test           # frontend tests
cargo test --workspace --manifest-path src-tauri/Cargo.toml   # Rust tests
```
