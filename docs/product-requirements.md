# Repola product requirements

Repola is a desktop Git workbench for working copies on the local computer and on machines reached through SSH. A machine is part of repository identity: the same absolute path on two machines is never treated as the same working copy.

This document is the acceptance contract for the product transformation. A checked item must be backed by automated tests where practical and must work through the same host execution interface for local and SSH machines.

## Product principles

- The common path is as direct as GitHub Desktop: choose a repository and worktree, review changes, commit, and synchronize.
- Local and SSH-hosted working copies have the same information architecture and commands. Remote state is not copied into a shadow repository or mounted through a network filesystem.
- Repository-wide worktree awareness appears where it is useful: selection, branch occupancy, create/switch, conflict prevention, cleanup, and storage review. It is not a separate application hidden behind a mode switch.
- Git remains the source of truth. Repola executes the installed Git CLI with exact arguments and parses stable porcelain formats.
- Destructive actions show the exact scope, capture a state fingerprint, and revalidate immediately before execution.
- Provider metadata is evidence, never a substitute for local Git state. Pull-request completion is never inferred from ancestry.
- Keyboard navigation, screen-reader semantics, contrast, focus behavior, reduced motion, and responsive minimum-window behavior are release requirements.

## Daily working-copy workflow

- [x] Add, create, clone, publish, remove-from-list, and reveal a repository.
- [x] Select a machine, repository, worktree, and branch without losing the current context.
- [x] Show staged, unstaged, untracked, conflicted, ignored, submodule, rename, copy, binary, executable-bit, and symlink status correctly.
- [x] Render text, image, binary, large-file, and submodule diffs with explicit fallback states.
- [x] Select whole files and individual diff lines or hunks for staging and unstaging.
- [x] Discard selected lines, hunks, files, or all changes through a reviewed destructive action.
- [x] Commit with summary, description, author identity, co-authors, trailers, signing configuration, and hook output.
- [x] Amend the latest commit and undo the latest local commit without discarding its changes.
- [x] Fetch, pull, push, force-push-with-lease, and publish with visible ahead/behind/divergence state.
- [x] Detect in-progress merge, rebase, cherry-pick, revert, bisect, and sequencer operations and guide continue/skip/abort actions.
- [x] Show actionable authentication, hook, LFS, safe-directory, and transport failures without hiding Git output.

## History and branch workflow

- [x] Browse paginated commit history without loading an unbounded graph into memory.
- [x] Inspect commit metadata, signatures, parents, changed files, and per-file diffs.
- [x] Search commits and compare any local or remote branch.
- [x] Create, checkout, rename, publish, and delete branches with worktree occupancy checks.
- [x] Merge, rebase, squash, cherry-pick, revert, reset, and create/delete/push tags.
- [x] Preserve recoverability by exposing reflog-based undo points for destructive history operations.
- [x] Resolve text conflicts with ours/theirs/both/manual choices and explicit unresolved-file tracking.
- [x] Stash all or selected changes, include untracked files, list/apply/pop/drop stashes, and recover conflicts.

## Worktree workflow

- [x] Show only repositories explicitly added, cloned, created, or dropped by the user; never crawl parent folders or guess filesystem locations.
- [x] Inventory primary, linked, missing, locked, prunable, agent-created, and unattributed worktrees.
- [x] Inspect dirtiness, unpushed commits, disk allocation, recent activity, containment evidence, and provider pull-request evidence.
- [x] Preview and revalidate safe worktree removal, metadata repair, unlock, prune, and branch deletion.
- [x] Create a worktree from a new or existing branch with collision and branch-occupancy validation.
- [x] Switch worktrees from the repository toolbar while retaining repository context.
- [x] Explain branch occupancy inline wherever an implemented branch action would be affected.
- [x] Surface repository worktree health in the repository picker and contextual maintenance panel.
- [x] Support bulk review and cleanup across repositories on one selected machine.

## Machines and remote execution

- [x] Treat `local` as a built-in machine using the same protocol contract as remote machines.
- [x] Add, edit, test, reorder, disable, and remove SSH machine profiles without storing private keys or passphrases.
- [x] Use the system OpenSSH client and honor host aliases, `ProxyJump`, identity agents, hardware keys, and `known_hosts` policy.
- [x] Never enable agent forwarding implicitly.
- [x] Negotiate protocol and agent versions before repository commands.
- [x] Bootstrap or upgrade a matching signed/checksummed `repola-agent` for the remote OS and architecture.
- [x] Frame requests and responses so arbitrary Git output cannot corrupt the protocol stream.
- [x] Stream inventory progress, bound every captured output, support cancellation, apply timeouts, and recover from disconnects.
- [x] Keep machine, repository, worktree, and operation IDs stable and collision-resistant.
- [x] Make remote latency and connection state visible without turning the app into a remote-terminal UI.

## Provider integration

- [x] Authenticate to GitHub.com and GitHub Enterprise without putting tokens in settings files.
- [x] Show repository, issue, pull-request, review, check, and merge-queue context for the selected branch.
- [x] Create pull requests, open them in the browser, copy links, and check out pull-request branches.
- [x] Preserve generic Git hosting support when no provider integration exists.
- [x] Retain Azure DevOps pull-request evidence support and make provider adapters independently testable.
- [x] Keep SSH host credentials, Git-origin credentials, and provider API credentials as separate concerns.

## Desktop integration and quality

- [x] Open the selected worktree in a configured editor, terminal, shell, or file manager on the machine where that action makes sense.
- [x] Provide application menus, contextual menus, drag/drop repository addition, keyboard shortcuts, and command palette.
- [x] Persist window, pane, selection, machine, repository, worktree, filter, theme, editor, and Git preferences with versioned migrations.
- [x] Redact credentials and sensitive environment values from diagnostics and audit logs.
- [ ] Package and update Repola and its matching agent on macOS, Windows, and Linux.
- [ ] Pass frontend unit/integration tests, Rust unit/integration tests, protocol golden tests, real-repository fixtures, and cross-platform CI.
- [x] Remain responsive with large repositories, tens of thousands of changes, long histories, many worktrees, and high-latency SSH links.

## Explicit non-goals

- Remote desktop, screen sharing, or rendering an application running on the remote machine.
- Editing arbitrary remote files over SFTP.
- Maintaining a second hidden clone as a proxy for a remote working copy.
- Reimplementing Git object storage, transport, credentials, or merge semantics.
- Silently forcing destructive Git operations to make an action appear successful.
