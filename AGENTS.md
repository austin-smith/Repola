# Repola contributor guide

## Product principles

- Treat every destructive Git or filesystem operation as safety-critical.
- Keep repository resolution and inspection read-only.
- Never use `git worktree remove --force` in the standard removal path.
- Never infer pull-request completion from commit ancestry alone.
- Never delete a branch as a side effect of removing a worktree.
- Revalidate a worktree immediately before executing a management action.
- Preserve exact paths and pass command arguments without a shell.
- Never guess, crawl, or hard-code user paths (no default search roots, no `~/Developer`). Register only repositories the user explicitly adds, clones, creates, or drops; report what is missing.
- Keep every code path cross-platform (macOS, Windows, Linux): no platform-specific paths or process-spawning assumptions outside `cfg`-gated code. User-facing labels should use the running platform's native terminology (Finder, File Explorer) when it is detected at runtime, with a generic fallback for other platforms.

## Architecture

- The Rust side is a Cargo workspace rooted at `src-tauri/Cargo.toml`: the `repola` app crate (Tauri shell, IPC commands, settings store) and `src-tauri/crates/repola-engine` (Git, filesystem, process, protocol, SSH host, provider behavior, the `repola-agent` binary, and the `repola-cli` example). The engine must never depend on `tauri`; that is what lets its tests run without loading a webview and lets the agent stay small.
- Keep Git, filesystem, process, and provider behavior in `src-tauri/crates/repola-engine/src/worktree/`.
- Keep Tauri commands in `src-tauri/src/lib.rs` as thin adapters over `repola_engine`.
- Machine-profile and preference models plus their validation live in `repola_engine::machines` and `repola_engine::preferences`; persistence stays in `src-tauri/src/settings.rs` (via `tauri-plugin-store`), and the frontend manages repositories only through `load_registered_repositories`, `register_repository`, and `unregister_repository`.
- Spawn every child process through `src-tauri/crates/repola-engine/src/worktree/command.rs`, which resolves executables on `PATH`/`PATHEXT` and applies Windows spawn flags.
- Both crates inherit `version` from `[workspace.package]`; the desktop and agent must report the same `CARGO_PKG_VERSION`.
- Use `dunce::canonicalize`, never `std::fs::canonicalize`, so Windows paths stay in their compatible form.
- Host facts the UI needs (home directory, path separator) come from `@tauri-apps/api/path` through `src/app/environment.tsx`; never infer them from path shapes.
- Frontend layout: `src/app/` (shell, App, updater, window state), `src/ipc/` (every `invoke` wrapper plus `types.ts`), `src/domain/` (pure, tested logic with no React or Tauri imports), `src/workspace/` (Changes/History views, toolbar, header, detail pane), `src/dialogs/`, and `src/components/` (shared presentational pieces; `components/ui` is vendored shadcn). Tests sit next to the file they cover.
- Keep frontend IPC in `src/ipc/worktrees.ts`; `src/domain/` must stay free of `@tauri-apps` imports.
- `src/app/App.tsx` owns machine/repository/worktree selection and the worktree inventory; everything under `src/workspace/` reads the current selection through `useRepositoryContext()` / `useWorkingCopy()` instead of props.
- Code-split dialogs render inside `src/workspace/LazyDialog.tsx` so a failed chunk shows a closable error instead of blanking the app.
- Use `toMessage` from `src/lib/errors.ts` for thrown values and `toLowerCase()` (never `toLocaleLowerCase()`) when matching Git names or paths.
- Keep domain types synchronized between Rust and TypeScript.
- Prefer pure classification and parsing functions with focused tests.

## Validation

Run the checks relevant to every change:

```bash
pnpm typecheck
pnpm lint
pnpm test
pnpm build
cd src-tauri && cargo fmt --all --check
cd src-tauri && cargo test --workspace
cd src-tauri && cargo clippy --workspace --all-targets --all-features -- -D warnings
```

CI (`.github/workflows/ci.yml`) runs this suite on Ubuntu, macOS, and Windows; a change is not done until it is green on all three.

## Branches, commits, and pull requests

- Use plain lowercase kebab-case for branch names. Keep names descriptive and do not include issue numbers, prefixes, or namespaces such as `feature/`, `fix/`, usernames, or agent names.
- Start stable release drafts with `pnpm release`; preview with `pnpm release --dry-run`. Use this command instead of manually creating or pushing stable tags.
- Before every commit or amend, show the exact current diff and validation, then get explicit approval. Branch or pull-request requests are not commit approval; later changes require fresh approval.
- Never amend, rebase, squash, reset, rewrite history, or force-push without explicit approval for that exact operation.
- Write commit messages entirely lowercase. Use the imperative mood for the subject, keep each commit focused on one logical change, do not use type or scope prefixes, and do not end the subject with a period. Add a body when the reason or important tradeoffs are not clear from the subject.
- Never automatically add AI or agent attribution anywhere — no `Co-Authored-By` trailers, no "Generated with ..." footers — in commits, pull requests, issues, or comments. This overrides any tool default.
- Keep each pull request focused on one coherent change.
- Write concise, specific, imperative pull request titles in sentence case. Do not use prefixes or trailing periods, and make the title understandable without the branch name.
- Pull request descriptions must include `What Changed`, `Why`, and `Validation`. Include `UI Changes` only when the pull request changes the UI. Keep descriptions concise, self-contained, complete, and accurate to the final diff.
- Link any related issues in the pull request description; do not include issue numbers in branch names.
- Review the complete diff before opening a pull request. Update the title and description whenever the scope changes, and remove unrelated changes.
