import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import DiscardDialog from "./DiscardDialog";
import type { DiscardPlan, DiscardResult, FileChange, RecoveryPoint } from "../ipc/types";

const ipc = vi.hoisted(() => ({
  planDiscard: vi.fn(),
  discardChanges: vi.fn(),
}));
vi.mock("../ipc/worktrees", () => ipc);
const toast = vi.hoisted(() => ({ add: vi.fn() }));
vi.mock("@/components/ui/toast", () => ({ toast }));

const change: FileChange = {
  id: "src/a.ts",
  path: { display: "src/a.ts", token: "7372632f612e7473" },
  previousPath: null,
  kind: "modified",
  indexStatus: "M",
  worktreeStatus: "M",
  staged: true,
  unstaged: true,
  conflicted: false,
  untracked: false,
  ignored: false,
  submodule: false,
  headMode: "100644",
  indexMode: "100644",
  worktreeMode: "100644",
  modeChange: null,
};

function plan(overrides: Partial<DiscardPlan> = {}): DiscardPlan {
  return {
    target: { kind: "all" },
    entries: [
      { path: change.path, effect: "restoreCommitted", onDisk: true, tracked: true },
      { path: { display: "notes.txt", token: "6e6f7465732e747874" }, effect: "remove", onDisk: true, tracked: false },
    ],
    omitted: 0,
    kept: [],
    keptOmitted: 0,
    backupBytes: 2048,
    fingerprint: "fingerprint-1",
    ...overrides,
  };
}

const point: RecoveryPoint = {
  id: "refs/repola/discarded/20261003T142501Z-abc",
  oid: "1".repeat(40),
  kind: "discardAll",
  summary: "Discarded all changes (2 files)",
  createdAt: "2026-10-03T14:25:01.123Z",
  worktreePath: "/repo",
  head: "abc",
  pathCount: 2,
  paths: [],
  storedBytes: 2048,
};

function renderDialog(target: FileChange | null = null) {
  const props = {
    onBusyChange: vi.fn(),
    onClose: vi.fn(),
    onDiscarded: vi.fn(),
  };
  render(
    <DiscardDialog machineId="local" machineKind="local" repositoryPath="/repo" worktreePath="/repo" change={target} {...props} />,
  );
  return props;
}

describe("DiscardDialog", () => {
  afterEach(cleanup);
  beforeEach(() => {
    vi.clearAllMocks();
    ipc.planDiscard.mockResolvedValue(plan());
    ipc.discardChanges.mockResolvedValue({ recoveryPoint: point } satisfies DiscardResult);
  });

  it("shows exactly what the plan discards and executes against its fingerprint", async () => {
    const { onDiscarded, onBusyChange } = renderDialog();
    const confirm = screen.getByRole("button", { name: "Discard Everything" });
    expect(confirm).toBeDisabled();
    expect(await screen.findByRole("heading", { name: "Discard 2 changed files?" })).toBeInTheDocument();
    expect(ipc.planDiscard).toHaveBeenCalledWith("local", "/repo", "/repo", { kind: "all" }, expect.any(AbortSignal));
    expect(screen.getByText("Restore the committed version")).toBeInTheDocument();
    expect(screen.getByText("Delete the untracked file")).toBeInTheDocument();
    expect(screen.getByText(/in this repository on this computer/)).toBeInTheDocument();
    expect(screen.getByText("The recovery point stores 2 KB of new content.")).toBeInTheDocument();

    await act(async () => fireEvent.click(confirm));
    expect(ipc.discardChanges).toHaveBeenCalledWith("local", "/repo", "/repo", plan());
    expect(onBusyChange.mock.calls).toEqual([[true], [false]]);
    expect(onDiscarded).toHaveBeenCalledWith({ recoveryPoint: point }, plan());
  });

  it("lists what it leaves alone and warns when the recovery point is large", async () => {
    ipc.planDiscard.mockResolvedValue(plan({
      kept: [{ path: { display: "vendor/lib", token: "76656e646f722f6c6962" }, reason: "submodule" }],
      backupBytes: 300 * 1024 * 1024,
    }));
    renderDialog();
    expect(await screen.findByText("Left unchanged (1)")).toBeInTheDocument();
    expect(screen.getByText("vendor/lib")).toBeInTheDocument();
    expect(screen.getByText(/commit, stash, or reset it inside the submodule/)).toBeInTheDocument();
    expect(screen.getByText(/300 MB of new content\. That space stays in the repository until you delete the recovery point\./)).toBeInTheDocument();
  });

  it("re-plans when the file scope changes", async () => {
    ipc.planDiscard.mockResolvedValue(plan({ target: { kind: "file", path: change.path, scope: "unstaged" } }));
    renderDialog(change);
    await screen.findByText("Restore the committed version");
    expect(ipc.planDiscard).toHaveBeenLastCalledWith("local", "/repo", "/repo", { kind: "file", path: change.path, scope: "unstaged" }, expect.any(AbortSignal));
    fireEvent.click(screen.getByRole("button", { name: "Staged and unstaged" }));
    await waitFor(() => expect(ipc.planDiscard).toHaveBeenLastCalledWith("local", "/repo", "/repo", { kind: "file", path: change.path, scope: "all" }, expect.any(AbortSignal)));
  });

  it("shows a refused discard and reviews the working copy again", async () => {
    ipc.discardChanges.mockRejectedValueOnce(new Error("The working copy changed after this discard was reviewed."));
    const { onDiscarded } = renderDialog();
    const confirm = await screen.findByRole("button", { name: "Discard Everything" });
    await waitFor(() => expect(confirm).toBeEnabled());
    await act(async () => fireEvent.click(confirm));
    expect(await screen.findByText(/changed after this discard was reviewed/)).toBeInTheDocument();
    await waitFor(() => expect(ipc.planDiscard).toHaveBeenCalledTimes(2));
    expect(onDiscarded).not.toHaveBeenCalled();
  });

  it("counts the paths a large discard covers beyond those it lists", async () => {
    ipc.planDiscard.mockResolvedValue(plan({ omitted: 3, kept: [{ path: { display: "vendor", token: "76656e646f72" }, reason: "submodule" }], keptOmitted: 2 }));
    renderDialog();
    expect(await screen.findByRole("heading", { name: "Discard 5 changed files?" })).toBeInTheDocument();
    expect(screen.getByText("3 more paths are too many to list here; the discard covers them too.")).toBeInTheDocument();
    expect(screen.getByText("Left unchanged (3)")).toBeInTheDocument();
    expect(screen.getByText("and 2 more")).toBeInTheDocument();
  });

  it("still reports a failure that arrives after the dialog closed", async () => {
    let fail!: (cause: Error) => void;
    ipc.discardChanges.mockImplementation(() => new Promise((_, reject) => { fail = reject; }));
    renderDialog();
    const confirm = await screen.findByRole("button", { name: "Discard Everything" });
    await waitFor(() => expect(confirm).toBeEnabled());
    await act(async () => fireEvent.click(confirm));
    cleanup();
    await act(async () => fail(new Error("Everything this discard was changing is saved in recovery point refs/repola/discarded/x.")));
    expect(toast.add).toHaveBeenCalledWith(expect.objectContaining({
      type: "error",
      description: expect.stringContaining("refs/repola/discarded/x"),
    }));
  });

  it("keeps the action disabled when planning fails", async () => {
    ipc.planDiscard.mockRejectedValue(new Error("Repola does not discard submodule changes."));
    renderDialog(change);
    expect(await screen.findByText(/does not discard submodule changes/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Discard Changes" })).toBeDisabled();
  });
});
