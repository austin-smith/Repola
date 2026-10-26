import { beforeEach, describe, expect, it, vi } from "vitest";
import { invokeOperation } from "./operations";
import { onWorkspaceMutation, type WorkspaceMutationEvent } from "./refresh-events";
import { mutateBranch, mutateHistory, createRepository, deleteRecoveryPoints, discardChanges, planDiscard, planRecoveryRestore, registerRepository, restoreRecoveryPoint, setFileStaging, synchronizeWorkingCopy, unregisterRepository } from "./worktrees";
import { invoke } from "@tauri-apps/api/core";
import type { DiscardPlan, RecoveryRestorePlan, WorkingCopySnapshot } from "./types";

vi.mock("./operations", () => ({ invokeOperation: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(), revealItemInDir: vi.fn() }));

beforeEach(() => {
  vi.mocked(invokeOperation).mockReset().mockResolvedValue(undefined);
  vi.mocked(invoke).mockReset().mockResolvedValue(undefined);
});

describe("mutation IPC refresh scope", () => {
  it("captures exact file scope before invoke and requests no sibling Git refresh", async () => {
    const events: WorkspaceMutationEvent[] = [];
    const unsubscribe = onWorkspaceMutation((event) => { events.push(event); });
    vi.mocked(invokeOperation).mockImplementation(() => {
      expect(events).toEqual([{ phase: "start", machineId: "local", repositoryPath: "repo with spaces", worktreePath: "tree", impact: "files" }]);
      return Promise.resolve(undefined);
    });
    try {
      await setFileStaging("local", "repo with spaces", "tree", [], true);
      expect(events[1]).toEqual({ ...events[0], phase: "settled" });
    } finally {
      unsubscribe();
    }
  });

  it("classifies ref and inventory changes and emits settlement after failures", async () => {
    const events: WorkspaceMutationEvent[] = [];
    const unsubscribe = onWorkspaceMutation((event) => { events.push(event); });
    const failure = new Error("partially failed");
    try {
      await synchronizeWorkingCopy("remote", "repo", "tree", "fetch", "abc");
      await createRepository("remote", "destination", "main");
      await registerRepository("local", "exact path");
      await deleteRecoveryPoints("local", "repo", "tree", []);
      vi.mocked(invoke).mockRejectedValueOnce(failure);
      await expect(unregisterRepository("local", "repo")).rejects.toBe(failure);
      expect(events.filter((event) => event.phase === "settled")).toEqual([
        { phase: "settled", machineId: "remote", repositoryPath: "repo", worktreePath: "tree", impact: "git" },
        { phase: "settled", machineId: "remote", impact: "inventory" },
        { phase: "settled", machineId: "local", repositoryPath: "exact path", impact: "inventory" },
        { phase: "settled", machineId: "local", repositoryPath: "repo", worktreePath: "tree", impact: "git" },
        { phase: "settled", machineId: "local", repositoryPath: "repo", impact: "inventory" },
      ]);
    } finally {
      unsubscribe();
    }
  });

  it.each(["discard", "restore"] as const)("fences reads only while a reviewed %s executes and reconciles partial failures", async (kind) => {
    const events: WorkspaceMutationEvent[] = [];
    const unsubscribe = onWorkspaceMutation((event) => { events.push(event); });
    const discard: DiscardPlan = { target: { kind: "all" }, entries: [], omitted: 0, kept: [], keptOmitted: 0, backupBytes: 0, fingerprint: "reviewed" };
    const restore: RecoveryRestorePlan = {
      point: { id: "saved", oid: "abc", kind: "discardAll", summary: "Saved changes", createdAt: "2026-10-01T00:00:00Z", worktreePath: "tree", head: "abc", pathCount: 0, paths: [], storedBytes: 0 },
      entries: [], omitted: 0, fingerprint: "reviewed",
    };
    try {
      if (kind === "discard") await planDiscard("local", "repo", "tree", discard.target);
      else await planRecoveryRestore("local", "repo", "tree", restore.point);
      expect(events).toEqual([]);

      let reject!: (error: Error) => void;
      const pending = new Promise<never>((_resolve, fail) => { reject = fail; });
      vi.mocked(invokeOperation).mockImplementation(() => {
        expect(events).toEqual([{ phase: "start", machineId: "local", repositoryPath: "repo", worktreePath: "tree", impact: "files" }]);
        return pending;
      });
      const operation = kind === "discard"
        ? discardChanges("local", "repo", "tree", discard)
        : restoreRecoveryPoint("local", "repo", "tree", restore);
      const failure = new Error("files changed before the operation failed");
      const assertion = expect(operation).rejects.toBe(failure);
      reject(failure);
      await assertion;
      expect(events).toEqual([
        { phase: "start", machineId: "local", repositoryPath: "repo", worktreePath: "tree", impact: "files" },
        { phase: "settled", machineId: "local", repositoryPath: "repo", worktreePath: "tree", impact: "files" },
      ]);
    } finally {
      unsubscribe();
    }
  });

  it("sends the exact reviewed branch for attached and detached history and branch mutations", async () => {
    const snapshot: WorkingCopySnapshot = {
      repositoryPath: "repo", worktreePath: "tree", head: "abc", branch: "main",
      upstream: null, upstreamHead: null, remote: null, ahead: 0, behind: 0, changes: [], operation: null,
    };
    await mutateHistory("local", snapshot, "resetHard", "base");
    expect(invokeOperation).toHaveBeenLastCalledWith("mutate_history", {
      machineId: "local",
      request: { repositoryPath: "repo", worktreePath: "tree", kind: "resetHard", target: "base", expectedHead: "abc", expectedBranch: "main", expectedChanges: [] },
    }, {});
    await mutateHistory("local", { ...snapshot, branch: null }, "resetSoft", "base");
    expect(vi.mocked(invokeOperation).mock.lastCall?.[1]).toMatchObject({ request: { expectedBranch: null } });
    await mutateBranch("local", "repo", "tree", "rename", "renamed", null, "abc", "main");
    expect(vi.mocked(invokeOperation).mock.lastCall?.[1]).toMatchObject({ request: { expectedHead: "abc", expectedBranch: "main" } });
  });

});
