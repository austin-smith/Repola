import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import DiscardedChangesDialog from "./DiscardedChangesDialog";
import type { RecoveryPoint, RecoveryRestorePlan, RecoveryRestoreResult } from "../ipc/types";

const ipc = vi.hoisted(() => ({
  loadRecoveryPoints: vi.fn(),
  planRecoveryRestore: vi.fn(),
  restoreRecoveryPoint: vi.fn(),
  loadRecoveryFileDiff: vi.fn(),
  deleteRecoveryPoints: vi.fn(),
}));
vi.mock("../ipc/worktrees", () => ipc);
vi.mock("@pierre/diffs/react", () => ({ PatchDiff: ({ patch }: { patch: string }) => <pre data-testid="patch-diff">{patch}</pre> }));
vi.mock("@/components/theme-provider", () => ({ useTheme: () => ({ resolvedTheme: "light" }) }));

function point(overrides: Partial<RecoveryPoint>): RecoveryPoint {
  return {
    id: "refs/repola/discarded/20261003T142501Z-aaa",
    oid: "1".repeat(40),
    kind: "discardFile",
    summary: "Discarded changes to a.txt",
    createdAt: "2026-10-03T14:25:01.123Z",
    worktreePath: "/repo",
    head: "abc",
    pathCount: 1,
    paths: [{ display: "a.txt", token: "612e747874" }],
    storedBytes: 12,
    ...overrides,
  };
}

const newest = point({});
const sibling = point({ id: "refs/repola/discarded/20261002T000000Z-bbb", oid: "2".repeat(40), summary: "Discarded changes to b.txt", worktreePath: "/repo-feature" });
const older = point({ id: "refs/repola/discarded/20261001T000000Z-ccc", oid: "3".repeat(40), kind: "discardAll", summary: "Discarded all changes (30 files)", pathCount: 30 });

const restorePlan: RecoveryRestorePlan = {
  point: newest,
  entries: [
    { path: { display: "a.txt", token: "612e747874" }, worktree: "replace", indexChanges: false },
    { path: { display: "same.txt", token: "73616d652e747874" }, worktree: "unchanged", indexChanges: false },
  ],
  omitted: 0,
  fingerprint: "restore-fingerprint",
};

function renderDialog(initialPointId: string | null = null) {
  const props = { onBusyChange: vi.fn(), onClose: vi.fn() };
  render(
    <DiscardedChangesDialog
      machineId="build-box"
      machineKind="ssh"
      repositoryPath="/repo"
      worktreePath="/repo"
      initialPointId={initialPointId}
      {...props}
    />,
  );
  return props;
}

describe("DiscardedChangesDialog", () => {
  afterEach(cleanup);
  beforeEach(() => {
    vi.clearAllMocks();
    ipc.loadRecoveryPoints.mockResolvedValue({ points: [newest, sibling, older], omitted: 0 });
    ipc.planRecoveryRestore.mockResolvedValue(restorePlan);
    ipc.loadRecoveryFileDiff.mockResolvedValue({ patch: "diff --git a/a.txt b/a.txt\n-now\n+saved\n", binary: false, truncated: false });
  });

  it("lists this worktree's recovery points first and says where they are stored", async () => {
    renderDialog(older.id);
    const list = await screen.findByRole("region", { name: "Recovery points" });
    const summaries = within(list).getAllByRole("button").map((button) => button.firstElementChild?.textContent);
    expect(summaries).toEqual([
      "Discarded changes to a.txt",
      "Discarded all changes (30 files)",
      "Discarded changes to b.txt",
    ]);
    expect(within(list).getByText("From other worktrees")).toBeInTheDocument();
    expect(screen.getByText(/in this repository on the remote machine/)).toBeInTheDocument();
    const details = screen.getByRole("region", { name: "Recovery point details" });
    expect(within(details).getByText(older.id)).toBeInTheDocument();
    expect(within(details).getByText("and 29 more")).toBeInTheDocument();
  });

  it("reviews a restore with a per-path preview and restores against the plan's fingerprint", async () => {
    const replaced = point({ id: "refs/repola/discarded/20261003T150000Z-ddd", oid: "4".repeat(40), kind: "restore", summary: "Replaced while restoring" });
    ipc.restoreRecoveryPoint.mockResolvedValue({ replaced } satisfies RecoveryRestoreResult);
    ipc.loadRecoveryPoints
      .mockResolvedValueOnce({ points: [newest, sibling, older], omitted: 0 })
      .mockResolvedValueOnce({ points: [replaced, newest, sibling, older], omitted: 0 });
    const { onBusyChange } = renderDialog();
    fireEvent.click(await screen.findByRole("button", { name: "Review Restore…" }));
    expect(await screen.findByText(/changes 1 listed path\. Whatever it replaces is saved as a new recovery point first\./)).toBeInTheDocument();
    expect(screen.getByText("Replace the current content")).toBeInTheDocument();
    expect(screen.queryByText("same.txt")).not.toBeInTheDocument();
    expect(await screen.findByTestId("patch-diff")).toHaveTextContent("+saved");
    expect(ipc.loadRecoveryFileDiff).toHaveBeenCalledWith("build-box", "/repo", "/repo", { id: newest.id, oid: newest.oid }, restorePlan.entries[0]?.path, expect.any(AbortSignal));

    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Restore 1 Path" })));
    expect(ipc.restoreRecoveryPoint).toHaveBeenCalledWith("build-box", "/repo", "/repo", restorePlan);
    expect(onBusyChange.mock.calls).toEqual([[true], [false]]);
    // The list loads again, now with the point holding what the restore replaced.
    expect(await screen.findByText("Replaced while restoring")).toBeInTheDocument();
    expect(ipc.loadRecoveryPoints).toHaveBeenCalledTimes(2);
  });

  it("says how many saved paths a restore covers beyond those it lists", async () => {
    ipc.planRecoveryRestore.mockResolvedValue({ ...restorePlan, omitted: 2 });
    renderDialog();
    fireEvent.click(await screen.findByRole("button", { name: "Review Restore…" }));
    expect(await screen.findByText(/2 more saved paths are too many to list here; the restore covers them too\./)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Restore" })).toBeEnabled();
  });

  it("reviews the restore again after a refusal, without the earlier preview", async () => {
    ipc.restoreRecoveryPoint.mockRejectedValue(new Error("The working copy changed after this restore was reviewed."));
    renderDialog();
    fireEvent.click(await screen.findByRole("button", { name: "Review Restore…" }));
    expect(await screen.findByTestId("patch-diff")).toHaveTextContent("+saved");
    ipc.loadRecoveryFileDiff.mockImplementation(() => new Promise(() => undefined));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Restore 1 Path" })));
    expect(await screen.findByText(/changed after this restore was reviewed/)).toBeInTheDocument();
    await waitFor(() => expect(ipc.planRecoveryRestore).toHaveBeenCalledTimes(2));
    await screen.findByRole("button", { name: "Restore 1 Path" });
    expect(screen.queryByTestId("patch-diff")).not.toBeInTheDocument();
  });

  it("restores only what the latest review planned and cancels the review it left", async () => {
    let resolveFirst!: (plan: RecoveryRestorePlan) => void;
    const olderPlan: RecoveryRestorePlan = { ...restorePlan, point: older, fingerprint: "older-fingerprint" };
    ipc.planRecoveryRestore
      .mockImplementationOnce(() => new Promise((resolve) => { resolveFirst = resolve; }))
      .mockResolvedValueOnce(olderPlan);
    ipc.restoreRecoveryPoint.mockResolvedValue({ replaced: null } satisfies RecoveryRestoreResult);
    renderDialog();
    fireEvent.click(await screen.findByRole("button", { name: "Review Restore…" }));
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    fireEvent.click(within(screen.getByRole("region", { name: "Recovery points" })).getByText("Discarded all changes (30 files)"));
    fireEvent.click(screen.getByRole("button", { name: "Review Restore…" }));
    const confirm = await screen.findByRole("button", { name: "Restore 1 Path" });
    expect(ipc.planRecoveryRestore.mock.calls[0]?.[4]?.aborted).toBe(true);

    await act(async () => resolveFirst(restorePlan));
    await act(async () => fireEvent.click(confirm));
    expect(ipc.restoreRecoveryPoint).toHaveBeenCalledWith("build-box", "/repo", "/repo", olderPlan);
  });

  it("deletes only the reviewed selection", async () => {
    ipc.deleteRecoveryPoints.mockResolvedValue(undefined);
    ipc.loadRecoveryPoints
      .mockResolvedValueOnce({ points: [newest, sibling, older], omitted: 0 })
      .mockResolvedValueOnce({ points: [sibling], omitted: 0 });
    renderDialog();
    const first = await screen.findByRole("checkbox", { name: "Select “Discarded changes to a.txt” for deletion" });
    expect(screen.getByRole("button", { name: "Delete…" })).toBeDisabled();
    fireEvent.click(first);
    fireEvent.click(screen.getByRole("checkbox", { name: "Select “Discarded all changes (30 files)” for deletion" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete 2…" }));
    const review = screen.getByRole("region", { name: "Recovery points to delete" });
    expect(within(review).getByText(newest.id)).toBeInTheDocument();
    expect(within(review).getByText(older.id)).toBeInTheDocument();
    expect(ipc.deleteRecoveryPoints).not.toHaveBeenCalled();

    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Delete Recovery Points" })));
    expect(ipc.deleteRecoveryPoints).toHaveBeenCalledWith("build-box", "/repo", "/repo", [
      { id: newest.id, oid: newest.oid },
      { id: older.id, oid: older.oid },
    ]);
    const list = screen.getByRole("region", { name: "Recovery points" });
    expect(within(list).getAllByRole("button")).toHaveLength(1);
    await waitFor(() => expect(ipc.loadRecoveryPoints).toHaveBeenCalledTimes(2));
    expect(within(list).getAllByRole("button")).toHaveLength(1);
  });

  it("says how many older points were left out and lists them once a deletion makes room", async () => {
    ipc.loadRecoveryPoints
      .mockResolvedValueOnce({ points: [newest], omitted: 1 })
      .mockResolvedValueOnce({ points: [older], omitted: 0 });
    ipc.deleteRecoveryPoints.mockResolvedValue(undefined);
    renderDialog();
    expect(await screen.findByText("1 older recovery point not shown. Delete recovery points you no longer need to see it.")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("checkbox", { name: "Select “Discarded changes to a.txt” for deletion" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete 1…" }));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Delete Recovery Point" })));
    const list = screen.getByRole("region", { name: "Recovery points" });
    expect(await within(list).findByText("Discarded all changes (30 files)")).toBeInTheDocument();
    expect(within(list).queryByText("Discarded changes to a.txt")).not.toBeInTheDocument();
    expect(screen.queryByText(/not shown/)).not.toBeInTheDocument();
  });
});
