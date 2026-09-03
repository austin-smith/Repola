import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { RepositorySummary } from "../ipc/types";
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

describe("RepositoryToolbar", () => {
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
});
