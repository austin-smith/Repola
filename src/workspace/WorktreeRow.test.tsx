import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { WorktreeRecord } from "../ipc/types";
import { WorktreeRow } from "./WorktreeRow";

const worktree: WorktreeRecord = {
  id: "worktree-1",
  repositoryName: "repola",
  repositoryPath: "/repos/repola",
  path: "/repos/repola-worktree",
  branch: "feature/polish",
  head: "0123456789abcdef",
  detached: false,
  isPrimary: false,
  exists: true,
  createdAtMs: null,
  headCommitAtMs: null,
  lastActivityAtMs: null,
  headSubject: null,
  unpushedCommitCount: null,
  sizeBytes: 1024,
  sizeIncomplete: false,
  origin: { kind: "unattributed", id: "linked", label: "Linked" },
  status: { available: false, total: 0, staged: 0, unstaged: 0, untracked: 0, conflicted: 0 },
  registration: { kind: "healthy", reason: null },
  integration: { kind: "headContained", target: "origin/main", summary: "Contained in origin/main." },
  safety: { level: "review", label: "Safe to review", reasons: [] },
};

describe("WorktreeRow", () => {
  it("hides generic origin noise and uses plain-language state", () => {
    render(
      <TooltipProvider>
        <WorktreeRow
          now={Date.now()}
          selected={false}
          checked={false}
          worktree={worktree}
          onSelect={vi.fn()}
          onToggleChecked={vi.fn()}
          onContextMenu={vi.fn()}
        />
      </TooltipProvider>,
    );

    expect(screen.queryByText("Linked")).not.toBeInTheDocument();
    expect(screen.getByText("Unavailable")).toBeInTheDocument();
    expect(screen.getByText("Contained")).toBeInTheDocument();
  });
});
