import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { RepositorySummary, WorktreeRecord } from "../ipc/types";
import { RepositoryProvider } from "./context";
import { RepositoryToolbar } from "./RepositoryToolbar";

const repository: RepositorySummary = {
  id: "repo-1",
  name: "repola",
  path: "/repos/repola",
  remoteUrl: null,
  provider: "none",
  worktreeCount: 1,
  attentionCount: 3,
  conflictedCount: 0,
  allocatedBytes: 1024,
  allocationIncomplete: false,
};

function worktree(path: string, branch: string, isPrimary = false): WorktreeRecord {
  return {
    id: path,
    repositoryName: "repola",
    repositoryPath: "/repos/repola",
    path,
    branch,
    head: "1234567890abcdef",
    detached: false,
    isPrimary,
    exists: true,
    createdAtMs: null,
    headCommitAtMs: null,
    lastActivityAtMs: null,
    headSubject: null,
    unpushedCommitCount: null,
    sizeBytes: null,
    sizeIncomplete: false,
    origin: { kind: "unattributed", id: "linked", label: "Linked" },
    status: { available: true, total: 0, staged: 0, unstaged: 0, untracked: 0, conflicted: 0 },
    registration: { kind: "healthy", reason: null },
    integration: { kind: "headContained", target: "main", summary: "" },
    safety: { level: "review", label: "", reasons: [] },
  };
}

describe("RepositoryToolbar", () => {
  afterEach(cleanup);

  it("puts repository creation in the repository dropdown", () => {
    const onAddRepository = vi.fn();

    render(
      <RepositoryProvider value={{
        machineId: "local",
        machineKind: "local",
        machineOs: null,
        repository,
        worktree: null,
        refreshWorkspace: vi.fn(),
        showChanges: vi.fn(),
      }}>
        <RepositoryToolbar
          repositories={[repository]}
          worktrees={[]}
          onRepositoryChange={vi.fn()}
          onWorktreeChange={vi.fn()}
          onAddRepository={onAddRepository}
          onCreateWorktree={vi.fn()}
          onRemoveRepository={vi.fn()}
        />
      </RepositoryProvider>,
    );

    expect(screen.queryByRole("button", { name: "Repository actions" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Current repository" }));
    const menu = screen.getByRole("menu", { name: "Current repository" });
    expect(within(menu).getByRole("menuitem", { name: "repola" })).toBeInTheDocument();
    expect(within(menu).queryByText(/worktree|attention|healthy|conflicted|KB|GB/i)).not.toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: "Remove from Repola…" })).toBeInTheDocument();
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Add Repository…" }));

    expect(onAddRepository).toHaveBeenCalledOnce();
  });

  it("filters worktrees by folder name or branch and switches on selection", () => {
    const onWorktreeChange = vi.fn();
    const main = worktree("/repos/repola", "ai-commit-messages", true);
    const worktrees = [
      main,
      worktree("/worktrees/t3code-30dacb81", "t3code/fix-contents-pane-reload"),
      worktree("/worktrees/t3code-64844281", "t3code/investigate-repo-disk-usage"),
    ];

    render(
      <RepositoryProvider value={{
        machineId: "local",
        machineKind: "local",
        machineOs: null,
        repository,
        worktree: null,
        refreshWorkspace: vi.fn(),
        showChanges: vi.fn(),
      }}>
        <RepositoryToolbar
          repositories={[repository]}
          worktrees={worktrees}
          onRepositoryChange={vi.fn()}
          onWorktreeChange={onWorktreeChange}
          onAddRepository={vi.fn()}
          onCreateWorktree={vi.fn()}
          onRemoveRepository={vi.fn()}
        />
      </RepositoryProvider>,
    );

    fireEvent.click(screen.getByRole("combobox", { name: "Current worktree" }));
    const list = screen.getByRole("listbox");
    expect(within(list).getByText("Main Worktree")).toBeInTheDocument();
    expect(within(list).getByText("Linked Worktrees")).toBeInTheDocument();
    expect(within(list).getByRole("option", { name: /t3code-30dacb81.*t3code\/fix-contents-pane-reload/ })).toBeInTheDocument();

    const filter = screen.getByRole("combobox", { name: "Filter worktrees" });
    fireEvent.change(filter, { target: { value: "DISK-USAGE" } });
    expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual(["t3code-64844281t3code/investigate-repo-disk-usage"]);

    fireEvent.change(filter, { target: { value: "30dacb" } });
    const match = screen.getByRole("option", { name: /t3code-30dacb81/ });
    expect(screen.getAllByRole("option")).toHaveLength(1);
    fireEvent.click(match);

    expect(onWorktreeChange).toHaveBeenCalledWith("/worktrees/t3code-30dacb81");
  });
});
