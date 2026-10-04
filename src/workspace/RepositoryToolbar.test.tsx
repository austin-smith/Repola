import type { ComponentProps } from "react";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type * as WorktreeIpc from "../ipc/worktrees";
import type { BranchDeletionPlan, BranchDeletionRequest, BranchDeletionResult, BranchInfo, RepositorySummary, WorktreeRecord } from "../ipc/types";
import { fileManagerName } from "../domain/platform";
import { RepositoryProvider, type RepositoryContextValue } from "./context";
import { RepositoryToolbar } from "./RepositoryToolbar";

const ipc = vi.hoisted(() => ({
  loadBranches: vi.fn(),
  prepareBranchDeletionReview: vi.fn(),
  executeBranchDeletion: vi.fn(),
}));

vi.mock("../ipc/worktrees", async (importOriginal) => ({
  ...await importOriginal<typeof WorktreeIpc>(),
  ...ipc,
}));

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

function renderToolbar(
  overrides: Partial<ComponentProps<typeof RepositoryToolbar>> = {},
  selected: WorktreeRecord | null = null,
  context: Partial<RepositoryContextValue> = {},
) {
  return render(
    <RepositoryProvider value={{
      machineId: "local",
      machineKind: "local",
      machineOs: null,
      repository,
      worktree: selected,
      refreshWorkspace: vi.fn(),
      showChanges: vi.fn(),
      recordAuditPath: vi.fn(),
      ...context,
    }}>
      <RepositoryToolbar
        repositories={[repository]}
        worktrees={[]}
        onRepositoryChange={vi.fn()}
        onWorktreeChange={vi.fn()}
        onAddRepository={vi.fn()}
        onCreateWorktree={vi.fn()}
        onRemoveRepository={vi.fn()}
        {...overrides}
      />
    </RepositoryProvider>,
  );
}

describe("RepositoryToolbar", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("puts repository actions in the repository picker", () => {
    const onAddRepository = vi.fn();
    renderToolbar({ onAddRepository });

    expect(screen.queryByRole("button", { name: "Repository actions" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("combobox", { name: "Current repository" }));
    const list = screen.getByRole("listbox");
    expect(within(list).getByRole("option", { name: "repola" })).toBeInTheDocument();
    expect(within(list).queryByText(/worktree|attention|healthy|conflicted|KB|GB/i)).not.toBeInTheDocument();
    expect(within(list).getByRole("option", { name: "Remove from Repola…" })).toBeInTheDocument();
    fireEvent.click(within(list).getByRole("option", { name: "Add Repository…" }));

    expect(onAddRepository).toHaveBeenCalledOnce();
  });

  it("filters repositories by name and switches on selection", () => {
    const onRepositoryChange = vi.fn();
    const other = { ...repository, id: "repo-2", name: "t3code", path: "/repos/t3code" };
    renderToolbar({ repositories: [repository, other], onRepositoryChange });

    fireEvent.click(screen.getByRole("combobox", { name: "Current repository" }));
    const filter = screen.getByRole("combobox", { name: "Filter repositories" });
    fireEvent.change(filter, { target: { value: "T3" } });
    expect(screen.queryByRole("option", { name: "repola" })).not.toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Add Repository…" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("option", { name: "t3code" }));

    expect(onRepositoryChange).toHaveBeenCalledWith("/repos/t3code");

    fireEvent.click(screen.getByRole("combobox", { name: "Current repository" }));
    expect(screen.getByRole("combobox", { name: "Filter repositories" })).toHaveValue("");
    expect(screen.getByRole("option", { name: "repola" })).toBeInTheDocument();
  });

  it("explains when no repository matches the filter", () => {
    renderToolbar();

    fireEvent.click(screen.getByRole("combobox", { name: "Current repository" }));
    fireEvent.change(screen.getByRole("combobox", { name: "Filter repositories" }), { target: { value: "missing" } });

    expect(screen.getByText("No repositories match.")).toBeInTheDocument();
    expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual(["Add Repository…", `Show in ${fileManagerName()}`, "Remove from Repola…"]);
  });

  it("filters worktrees by folder name or branch and switches on selection", () => {
    const onWorktreeChange = vi.fn();
    const main = worktree("/repos/repola", "ai-commit-messages", true);
    const worktrees = [
      main,
      worktree("/worktrees/t3code-30dacb81", "t3code/fix-contents-pane-reload"),
      worktree("/worktrees/t3code-64844281", "t3code/investigate-repo-disk-usage"),
    ];

    renderToolbar({ worktrees, onWorktreeChange });

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

  it("opens a reviewed branch deletion from the branch actions menu", async () => {
    const main = worktree("/repos/repola", "main", true);
    const branches: BranchInfo[] = [
      { name: "main", fullName: "refs/heads/main", head: "1234567890abcdef", remote: false, current: true, upstream: null, ahead: 0, behind: 0, occupiedWorktreePath: "/repos/repola" },
      { name: "old-work", fullName: "refs/heads/old-work", head: "abcdef1234567890", remote: false, current: false, upstream: null, ahead: 0, behind: 0, occupiedWorktreePath: null },
    ];
    ipc.loadBranches.mockResolvedValue(branches);
    ipc.prepareBranchDeletionReview.mockImplementation(() => new Promise<never>(() => undefined));
    renderToolbar({ worktrees: [main] }, main);

    const actions = screen.getByRole("button", { name: "Branch actions" });
    await vi.waitFor(() => expect(actions).toBeEnabled());
    fireEvent.click(actions);
    fireEvent.click(await screen.findByRole("menuitem", { name: "Delete branch…" }));

    expect(await screen.findByRole("dialog", { name: "Delete a branch" })).toBeInTheDocument();
    const request: BranchDeletionRequest = {
      repositoryPath: "/repos/repola",
      worktreePath: "/repos/repola",
      branchRef: "refs/heads/old-work",
      deleteLocal: true,
      deleteRemote: false,
    };
    expect(ipc.prepareBranchDeletionReview).toHaveBeenCalledWith("local", request, expect.any(AbortSignal));
  });

  it("points the audit log at a deletion made from the branch actions menu", async () => {
    const main = worktree("/repos/repola", "main", true);
    ipc.loadBranches.mockResolvedValue([
      { name: "main", fullName: "refs/heads/main", head: "1234567890abcdef", remote: false, current: true, upstream: null, ahead: 0, behind: 0, occupiedWorktreePath: "/repos/repola" },
      { name: "old-work", fullName: "refs/heads/old-work", head: "abcdef1234567890", remote: false, current: false, upstream: null, ahead: 0, behind: 0, occupiedWorktreePath: null },
    ] satisfies BranchInfo[]);
    const plan: BranchDeletionPlan = {
      repositoryPath: "/repos/repola",
      worktreePath: "/repos/repola",
      branchRef: "refs/heads/old-work",
      branchName: "old-work",
      deleteLocal: true,
      deleteRemote: false,
      local: null,
      remote: null,
      remoteUnavailableReason: null,
      requiresForce: false,
      confirmation: "confirm",
      commands: ["git -C /repos/repola update-ref --no-deref -d refs/heads/old-work abcdef1234567890"],
      warnings: [],
      blockers: [],
      fingerprint: {
        localTip: "abcdef1234567890",
        mergeReferenceOid: null,
        requiresForce: false,
        remote: null,
        remoteRef: null,
        remoteOid: null,
        pullRequests: null,
        pushDestination: null,
        localReachability: null,
        remoteReachability: null,
        commands: [],
        warnings: [],
        confirmation: "confirm",
      },
    };
    const result: BranchDeletionResult = {
      message: "Deleted local branch old-work.",
      local: { target: "old-work", deletedOid: "abcdef1234567890", succeeded: true, output: "", warning: null, finishCommands: [], recoveryCommands: ["git -C /repos/repola branch -- old-work abcdef1234567890"] },
      remote: null,
      auditPath: "/logs/actions.jsonl",
      auditWarning: null,
    };
    ipc.prepareBranchDeletionReview.mockResolvedValue(plan);
    ipc.executeBranchDeletion.mockResolvedValue(result);
    const recordAuditPath = vi.fn();
    renderToolbar({ worktrees: [main] }, main, { recordAuditPath });

    const actions = screen.getByRole("button", { name: "Branch actions" });
    await vi.waitFor(() => expect(actions).toBeEnabled());
    fireEvent.click(actions);
    fireEvent.click(await screen.findByRole("menuitem", { name: "Delete branch…" }));
    await screen.findByText(/update-ref --no-deref -d refs\/heads\/old-work/);
    fireEvent.click(screen.getByRole("button", { name: "Delete Branch" }));

    await waitFor(() => expect(recordAuditPath).toHaveBeenCalledWith("/logs/actions.jsonl"));
  });
});
