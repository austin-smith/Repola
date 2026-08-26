# Repola architecture

## One product model

Every screen and command addresses a single context:

```text
Machine → Repository → Worktree → Git operation
```

`MachineId` and `WorktreeId` are opaque identifiers. A repository is currently addressed by `(MachineId, canonical repository path)`, so the same path on two machines never collides. The built-in local machine and SSH machines implement the same `HostClient` contract, so UI features cannot accidentally become local-only.

The repository toolbar follows the context order: machine, repository, worktree, branch, synchronization. Changes and History are two views over the selected working copy. Repository-wide worktree health is embedded in these selectors and in a contextual maintenance surface; the full inventory is a zoomed-out view of the same model.

## Process boundaries

```text
React UI
  │ typed Tauri commands/events
Desktop host service (Rust)
  │
  ├── Local transport ─────── repola-agent protocol handler in-process
  │
  └── SSH transport ──────── system ssh ─────── repola-agent --stdio
                                                     │
                                                     ├── git
                                                     ├── filesystem inspection
                                                     └── host capabilities
```

The desktop process owns application settings, machine profiles, operation coordination, audit storage, and UI event delivery. The agent owns filesystem and Git operations on the machine containing the working copy. Provider operations use the official provider CLI on that same machine, so repository discovery and provider commands agree about the remote without copying a working tree or sending a provider token through Repola. Provider CLI credentials remain in the CLI's native secure store and are never added to Repola settings. This is separate from both the desktop's SSH authentication and Git's origin transport credentials.

## Protocol

- UTF-8 JSON messages use an explicit length-prefixed frame, not newline delimiting. Git output may contain newlines or arbitrary bytes.
- Every request includes a protocol version, request ID, and typed operation payload. Deadlines are enforced by the owning desktop transport and by each child-command runner.
- Every response includes the matching request ID and is exactly one of success, structured failure, or a typed progress event.
- Handshake exchanges protocol range, agent build version, OS, architecture, Git version, capabilities, and maximum frame size.
- The desktop requires an exact agent build-version match. A missing or mismatched remote agent triggers a fixed platform probe, a bounded download from the pinned Repola release, SHA-256 and Minisign verification against the public key compiled into the desktop, an atomic per-user install, and a fresh handshake. The original operation is not sent until that handshake succeeds.
- Unknown fields are tolerated within a compatible protocol version; unknown operations and incompatible versions fail explicitly.
- Each desktop invocation has a collision-resistant operation ID registered before execution. Cancelling that ID propagates an operation token through local scans and Git children; SSH cancellation terminates and reaps the owned OpenSSH process, which closes the agent session. A future multiplexed protocol may carry cancellation frames without requiring a connection teardown.
- stdout is reserved for frames in stdio mode. Diagnostics go to stderr and never include secrets.
- Captured output is bounded. Inventory progress streams as typed events, file diffs have display ceilings, and history uses cursor-based pages rather than one unbounded response.
- Handshakes, ordinary requests, scans, individual commands, process exit, protocol frames, and diagnostics all have explicit size or time bounds.
- Bootstrap commands are fixed per supported platform; no profile or repository value is interpolated into remote shell source. Agent forwarding remains disabled during probing and installation.

## Command safety

- No shell evaluates user-controlled paths, revisions, messages, remote names, or ref names.
- Executables are resolved deliberately and arguments are passed as distinct values.
- Stable porcelain formats and `-z` delimiters are used wherever Git provides them.
- Paths are canonicalized on their owning machine with platform-correct semantics.
- Mutating requests carry the expected repository identity and a fresh state fingerprint.
- Destructive operations have separate plan and execute phases. Execution repeats discovery and policy validation.
- Standard worktree removal never adds `--force`; branch deletion is an independent follow-up action.
- A command allowlist is implemented as typed operations. There is no generic remote shell endpoint.

## Persistence and migration

Settings use typed, defensively decoded keys with explicit legacy migrations. Machine profiles contain SSH configuration references (host alias, optional user/port), never key material or passphrases. The local store persists machine-scoped roots and the last machine/repository/worktree/view position; Git and the remote filesystem remain authoritative.

The product rename migrates the legacy application-data directory and browser storage keys once, without overwriting existing Repola settings. Migration is idempotent and covered by tests.

## Performance boundaries

- Repository discovery, status, size, provider lookup, and diff loading are independent cancellable operations.
- The UI publishes partial inventory results in batches and uses stable keyed records.
- Change lists and history are windowed or use `content-visibility`; selection state is stored by stable path/commit identity.
- Diff parsing and expensive syntax work stay outside urgent React updates and can move to workers if measurement shows main-thread pressure.
- Remote calls avoid waterfalls: independent capability, repository, and provider requests start together after the minimum context is known.
- Cached data is labeled with its observation time and never presented as current safety evidence.

## Error model

Errors are structured by domain (`transport`, `protocol`, `git`, `filesystem`, `authentication`, `provider`, `validation`, `cancelled`, `timeout`, `internal`) and include a safe summary, actionable detail, retryability, and redacted diagnostic context. Raw Git stderr can be displayed when safe, but exit codes and command intent remain structured for reliable UI handling.

## Testing strategy

- Pure parsers, policies, identifiers, framing, and migrations use deterministic unit and property tests.
- Git behavior uses temporary real repositories rather than mocks.
- Protocol compatibility uses golden frames plus malformed, oversized, truncated, reordered, cancelled, and version-skew cases.
- Local and SSH transports run the same conformance suite; CI uses a disposable SSH server fixture.
- UI tests cover keyboard and pointer paths, focus restoration, loading/empty/error states, optimistic-state rollback, and destructive confirmations.
- Performance fixtures include large status output, large diffs, deep history, many worktrees, and injected network latency.
