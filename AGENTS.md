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
- Keep every code path cross-platform (macOS, Windows, Linux): no platform-specific paths, labels, or process-spawning assumptions outside `cfg`-gated code.

## Architecture

- Keep Git, filesystem, process, and provider behavior in `src-tauri/src/worktree/`.
- Keep Tauri commands in `src-tauri/src/lib.rs` as thin adapters.
- Keep persistent settings in `src-tauri/src/settings.rs` (via `tauri-plugin-store`); the frontend manages repositories only through `load_registered_repositories`, `register_repository`, and `unregister_repository`.
- Spawn every child process through `src-tauri/src/worktree/command.rs`, which resolves executables on `PATH`/`PATHEXT` and applies Windows spawn flags.
- Use `dunce::canonicalize`, never `std::fs::canonicalize`, so Windows paths stay in their compatible form.
- Host facts the UI needs (home directory, path separator) come from `@tauri-apps/api/path` through `src/environment.tsx`; never infer them from path shapes.
- Keep frontend IPC in `src/worktrees.ts`.
- `src/App.tsx` owns machine/repository/worktree selection and the worktree inventory; the Changes and History views, toolbar, header, and detail pane live in `src/workspace/` and read the current selection through `useRepositoryContext()` / `useWorkingCopy()` instead of props.
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
cd src-tauri && cargo fmt --check
cd src-tauri && cargo test
cd src-tauri && cargo clippy --all-targets --all-features -- -D warnings
```

CI (`.github/workflows/ci.yml`) runs this suite on Ubuntu, macOS, and Windows; a change is not done until it is green on all three.

## Git

- Use lowercase imperative commit messages.
- Do not add agent attribution.
- Do not commit without explicit user approval after showing the exact diff and validation.
