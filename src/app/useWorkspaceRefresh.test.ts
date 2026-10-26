import { act, cleanup, renderHook } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useWorkspaceRefresh } from "./useWorkspaceRefresh";
import { computeTotals } from "../domain/inventory";
import type { RefreshRequest, RefreshResult, RepositorySummary, ScanResult, WorkingCopySnapshot, WorkspaceChangedEvent, WorkspaceView, WorktreeRecord } from "../ipc/types";
import type { WorkspaceMutationEvent } from "../ipc/refresh-events";

const ipc = vi.hoisted(() => ({
  refreshWorkspace: vi.fn(), measureWorktree: vi.fn(), onWorkspaceChanged: vi.fn(),
  onWorkspaceMutation: vi.fn(), replaceWorkspaceWatch: vi.fn(), releaseWorkspaceWatch: vi.fn(),
}));
vi.mock("../ipc/worktrees", () => ipc);

const repository: RepositorySummary = { id: "repo", name: "repo", path: "/repo", remoteUrl: null, provider: "none", worktreeCount: 2, attentionCount: 0, conflictedCount: 0, allocatedBytes: 100, allocationIncomplete: false };
function tree(path: string, primary = false): WorktreeRecord {
  return { id: path, repositoryName: "repo", repositoryPath: "/repo", path, branch: primary ? "main" : "topic", head: "head", detached: false, isPrimary: primary, exists: true, createdAtMs: 1, headCommitAtMs: 2, lastActivityAtMs: 5, headSubject: "commit", unpushedCommitCount: 0, sizeBytes: primary ? 0 : 100, sizeIncomplete: false, origin: { kind: "unattributed", id: path, label: path }, status: { available: true, total: 0, staged: 0, unstaged: 0, untracked: 0, conflicted: 0 }, registration: { kind: "healthy", reason: null }, integration: { kind: "headContained", target: "main", summary: "Contained" }, safety: { level: "review", label: "Review", reasons: [] } };
}
const members = [tree("/repo", true), tree("/sibling")];
const snapshot: WorkingCopySnapshot = { repositoryPath: "/repo", worktreePath: "/repo", head: "head", branch: "main", upstream: null, upstreamHead: null, remote: null, ahead: 0, behind: 0, operation: null, changes: [] };
function initialScan(): ScanResult {
  return { scannedAtMs: 1, repositoryPaths: ["/repo"], repositories: [repository], worktrees: members, totals: computeTotals(members, 1), warnings: [] };
}
function result(request: RefreshRequest, overrides: Partial<RefreshResult> = {}): RefreshResult {
  return { refreshedAtMs: Date.now(), repositories: [repository], memberships: members, worktrees: members.filter((record) => request.worktreePaths === null || request.worktreePaths.includes(record.path)), successfulRepositoryPaths: ["/repo"], errors: [], gitSignatures: [{ repositoryPath: "/repo", signature: "unchanged" }], workingCopy: request.workingCopy ? { ...snapshot, changes: [] } : null, ...overrides };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}
let watched: ((event: WorkspaceChangedEvent) => void) | undefined;
let mutation: ((event: WorkspaceMutationEvent) => void) | undefined;
let hidden = false;
const flush = async () => { await act(async () => { await Promise.resolve(); }); };
async function advance(ms: number) { await act(async () => { await vi.advanceTimersByTimeAsync(ms); }); await flush(); }
function fileEvent(path: string) {
  const calls = ipc.replaceWorkspaceWatch.mock.calls;
  const subscriptionId = calls[calls.length - 1]?.[1] as string;
  watched?.({ machineId: "local", subscriptionId, repositoryPaths: ["/repo"], worktreePaths: [path], kind: "files" });
}
function manyRepositories(count = 3): ScanResult {
  const paths = ["/repo", "/second", "/third", "/fourth"].slice(0, count);
  const worktrees = paths.flatMap((path) => [
    { ...tree(path, true), repositoryPath: path, repositoryName: path },
    { ...tree(`${path}-topic`), repositoryPath: path, repositoryName: path },
  ]);
  return { scannedAtMs: 1, repositoryPaths: paths, repositories: paths.map((path) => ({ ...repository, id: path, name: path, path })), worktrees, totals: computeTotals(worktrees, paths.length), warnings: [] };
}
function mockInventory(fixture: ScanResult) {
  ipc.refreshWorkspace.mockImplementation(async (_machine: string, request: RefreshRequest) => {
    const scoped = fixture.worktrees.filter((record) => request.repositoryPaths.includes(record.repositoryPath));
    return result(request, {
      repositories: fixture.repositories.filter((item) => request.repositoryPaths.includes(item.path)),
      memberships: scoped,
      worktrees: scoped.filter((record) => request.worktreePaths === null || request.worktreePaths.includes(record.path)),
      successfulRepositoryPaths: request.repositoryPaths,
      gitSignatures: request.repositoryPaths.map((repositoryPath) => ({ repositoryPath, signature: "unchanged" })),
    });
  });
}
function mount(kind: "local" | "ssh" = "local", view: WorkspaceView = "changes", fixture = initialScan()) {
  return renderHook(() => {
    const [scan, setScan] = useState<ScanResult | null>(fixture);
    const refresh = useWorkspaceRefresh({ machineId: kind === "local" ? "local" : "remote", machineKind: kind, repositoryPaths: fixture.repositoryPaths, scan, setScan, repositoryPath: "/repo", worktreePath: "/repo", view, enabled: true, getEpoch: () => 0 });
    return { ...refresh, scan };
  });
}

describe("app-owned workspace refresh", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.resetAllMocks();
    hidden = false;
    vi.spyOn(document, "hidden", "get").mockImplementation(() => hidden);
    watched = undefined;
    mutation = undefined;
    ipc.refreshWorkspace.mockImplementation(async (_machine: string, request: RefreshRequest) => result(request));
    ipc.measureWorktree.mockImplementation(async (_machine: string, repositoryPath: string, worktreePath: string) => ({ repositoryPath, worktreePath, sizeBytes: 100, sizeIncomplete: false, lastActivityAtMs: 5 }));
    ipc.onWorkspaceChanged.mockImplementation(async (listener) => { watched = listener; return () => undefined; });
    ipc.onWorkspaceMutation.mockImplementation((listener) => { mutation = listener; return () => undefined; });
    ipc.replaceWorkspaceWatch.mockImplementation(async (_machine, subscriptionId) => ({ supported: true, active: true, subscriptionId, coverageErrors: [] }));
    ipc.releaseWorkspaceWatch.mockResolvedValue(undefined);
  });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.useRealTimers(); });

  it("refreshes only the file-event target and does not invalidate History or unrelated selected status", async () => {
    const hook = mount();
    await flush();
    expect(ipc.refreshWorkspace).toHaveBeenCalledTimes(1);
    const revision = hook.result.current.statusRevision;
    await act(async () => fileEvent("/sibling"));
    await flush();
    expect(ipc.refreshWorkspace.mock.calls[1][1]).toEqual({ repositoryPaths: ["/repo"], worktreePaths: ["/sibling"], workingCopy: null, refreshMetadata: false });
    expect(hook.result.current.statusRevision).toBe(revision);
    expect(hook.result.current.gitRevision).toBe(0);
    await act(async () => fileEvent("/repo"));
    await flush();
    expect(ipc.refreshWorkspace.mock.calls[2][1]).toEqual({ repositoryPaths: ["/repo"], worktreePaths: ["/repo"], workingCopy: { repositoryPath: "/repo", worktreePath: "/repo" }, refreshMetadata: false });
    expect(hook.result.current.gitRevision).toBe(0);
    expect(hook.result.current.workingCopySnapshot).toEqual(snapshot);
    expect(ipc.measureWorktree).not.toHaveBeenCalled();
  });

  it("coalesces events during a read and fences its result across a mutation", async () => {
    const hook = mount();
    await flush();
    const old = deferred<RefreshResult>();
    ipc.refreshWorkspace.mockImplementationOnce(() => old.promise);
    await act(async () => fileEvent("/repo"));
    expect(ipc.refreshWorkspace).toHaveBeenCalledTimes(2);
    const signal = ipc.refreshWorkspace.mock.calls[1][2] as AbortSignal;
    await act(async () => { for (let i = 0; i < 1000; i++) fileEvent("/sibling"); });
    expect(ipc.refreshWorkspace).toHaveBeenCalledTimes(2);
    expect(signal.aborted).toBe(false);
    await act(async () => mutation?.({ machineId: "local", repositoryPath: "/repo", worktreePath: "/repo", impact: "files", phase: "start" }));
    expect(signal.aborted).toBe(true);
    const revision = hook.result.current.statusRevision;
    await act(async () => old.resolve(result(ipc.refreshWorkspace.mock.calls[1][1] as RefreshRequest, { workingCopy: { ...snapshot, head: "obsolete" } })));
    expect(hook.result.current.statusRevision).toBe(revision);
    expect(hook.result.current.workingCopySnapshot?.head).toBe("head");
    expect(hook.result.current.refreshPending).toBe(true);
    expect(ipc.refreshWorkspace).toHaveBeenCalledTimes(2);
    await act(async () => mutation?.({ machineId: "local", repositoryPath: "/repo", worktreePath: "/repo", impact: "files", phase: "settled" }));
    await flush();
    expect(ipc.refreshWorkspace).toHaveBeenCalledTimes(3);
    expect(ipc.refreshWorkspace.mock.calls[2][1].worktreePaths).toEqual(["/sibling", "/repo"]);
    expect(hook.result.current.gitRevision).toBe(0);
    expect(hook.result.current.refreshPending).toBe(false);
  });

  it("waits for manual visible-data publication and mounted consumers", async () => {
    const hook = mount();
    await flush();
    const consumer = deferred<void>();
    const wait = vi.fn(() => consumer.promise);
    const unregister = hook.result.current.registerRefreshConsumer(wait);
    let completed = false;
    let refreshing!: Promise<void>;
    await act(async () => { refreshing = hook.result.current.refreshWorkspace().then(() => { completed = true; }); });
    await flush();
    expect(ipc.refreshWorkspace.mock.calls[1][1]).toEqual({ repositoryPaths: ["/repo"], worktreePaths: null, workingCopy: { repositoryPath: "/repo", worktreePath: "/repo" }, refreshMetadata: true });
    expect(hook.result.current.workingCopySnapshot).toEqual(snapshot);
    expect(wait).toHaveBeenCalledTimes(1);
    expect(completed).toBe(false);
    await act(async () => consumer.resolve());
    await refreshing;
    expect(completed).toBe(true);
    unregister();
  });

  it("retains hidden invalidations and runs one reconciliation on resume", async () => {
    mount();
    await flush();
    hidden = true;
    await act(async () => document.dispatchEvent(new Event("visibilitychange")));
    await act(async () => { fileEvent("/sibling"); window.dispatchEvent(new Event("focus")); });
    await advance(60_000);
    expect(ipc.refreshWorkspace).toHaveBeenCalledTimes(1);
    expect(ipc.measureWorktree).not.toHaveBeenCalled();
    hidden = false;
    await act(async () => document.dispatchEvent(new Event("visibilitychange")));
    await flush();
    expect(ipc.refreshWorkspace).toHaveBeenCalledTimes(2);
    expect(new Set(ipc.refreshWorkspace.mock.calls[1][1].worktreePaths)).toEqual(new Set(["/repo", "/sibling"]));
  });

  it("schedules SSH recovery after completion, backs off failures, and recovers without overlapping requests", async () => {
    const first = deferred<RefreshResult>();
    ipc.refreshWorkspace.mockImplementationOnce(() => first.promise);
    const hook = mount("ssh");
    await flush();
    await advance(60_000);
    expect(ipc.refreshWorkspace).toHaveBeenCalledTimes(1);
    await act(async () => first.reject(new Error("offline")));
    await flush();
    expect(hook.result.current.refreshError).toBe("offline");
    expect(hook.result.current.scan?.worktrees).toHaveLength(2);
    await advance(10_000);
    expect(ipc.refreshWorkspace).toHaveBeenCalledTimes(1);
    await advance(10_000);
    expect(ipc.refreshWorkspace).toHaveBeenCalledTimes(2);
    expect(hook.result.current.refreshError).toBeNull();
    await advance(10_000);
    expect(ipc.refreshWorkspace).toHaveBeenCalledTimes(3);
  });

  it("does not recursively measure already measured quiet trees during recovery", async () => {
    mount();
    await flush();
    await advance(30_000);
    expect(ipc.refreshWorkspace).toHaveBeenCalledTimes(2);
    await advance(2_000);
    expect(ipc.measureWorktree).not.toHaveBeenCalled();
  });

  it("keeps selected status and snapshot identity through quiet reconciliation", async () => {
    const hook = mount();
    await flush();
    const revision = hook.result.current.statusRevision;
    const accepted = hook.result.current.workingCopySnapshot;
    await advance(30_000);
    expect(ipc.refreshWorkspace).toHaveBeenCalledTimes(2);
    expect(hook.result.current.statusRevision).toBe(revision);
    expect(hook.result.current.workingCopySnapshot).toBe(accepted);
    expect(hook.result.current.gitRevision).toBe(0);
  });

  it("does not read Changes status again when History receives a file invalidation", async () => {
    const hook = mount("local", "history");
    await flush();
    await act(async () => fileEvent("/repo"));
    await flush();
    expect(ipc.refreshWorkspace.mock.calls.every((call) => call[1].workingCopy === null)).toBe(true);
    expect(hook.result.current.gitRevision).toBe(0);
  });

  it("recovers selected inventory status over SSH while History is open without loading a Changes snapshot", async () => {
    const fixture = manyRepositories();
    mockInventory(fixture);
    const hook = mount("ssh", "history", fixture);
    await flush();
    const revision = hook.result.current.statusRevision;
    mockInventory({ ...fixture, worktrees: fixture.worktrees.map((record) => record.path === "/repo"
      ? { ...record, status: { ...record.status, total: 1, unstaged: 1 } }
      : record) });
    for (let round = 0; round < 2; round++) {
      await advance(10_000);
      const request = ipc.refreshWorkspace.mock.calls[round + 1][1] as RefreshRequest;
      expect(request.repositoryPaths).toContain("/repo");
      expect(request.repositoryPaths.length).toBeLessThanOrEqual(2);
      expect(request.worktreePaths).toContain("/repo");
      expect(request.worktreePaths?.length).toBeLessThanOrEqual(2);
      expect(request.workingCopy).toBeNull();
      expect(request.refreshMetadata).toBe(true);
    }
    expect(hook.result.current.scan?.worktrees.find((record) => record.path === "/repo")?.status.unstaged).toBe(1);
    expect(hook.result.current.statusRevision).toBe(revision + 1);
    expect(hook.result.current.gitRevision).toBe(0);
    expect(hook.result.current.workingCopySnapshot).toBeNull();
    expect(ipc.replaceWorkspaceWatch).not.toHaveBeenCalled();
  });

  it("reconciles selected inventory status on local focus in History when no watcher invalidation arrives", async () => {
    const fixture = manyRepositories();
    mockInventory(fixture);
    const hook = mount("local", "history", fixture);
    await flush();
    const revision = hook.result.current.statusRevision;
    mockInventory({ ...fixture, worktrees: fixture.worktrees.map((record) => record.path === "/repo"
      ? { ...record, status: { ...record.status, total: 1, untracked: 1 } }
      : record) });
    await act(async () => window.dispatchEvent(new Event("focus")));
    await flush();
    expect(ipc.refreshWorkspace).toHaveBeenCalledTimes(2);
    const request = ipc.refreshWorkspace.mock.calls[1][1] as RefreshRequest;
    expect(request.repositoryPaths).toEqual(fixture.repositoryPaths);
    expect(new Set(request.worktreePaths)).toEqual(new Set(["/repo", "/repo-topic"]));
    expect(request.workingCopy).toBeNull();
    expect(request.refreshMetadata).toBe(true);
    expect(hook.result.current.scan?.worktrees.find((record) => record.path === "/repo")?.status.untracked).toBe(1);
    expect(hook.result.current.statusRevision).toBe(revision + 1);
    expect(hook.result.current.gitRevision).toBe(0);
    expect(hook.result.current.workingCopySnapshot).toBeNull();
  });

  it.each([3, 4])("bounds periodic recovery across %i repositories and visits every background tree", async (count) => {
    const fixture = manyRepositories(count);
    mockInventory(fixture);
    mount("local", "changes", fixture);
    await flush();
    const visitedRepositories = new Set<string>();
    const visitedTrees = new Set<string>();
    for (let round = 0; round < count * 2; round++) {
      await advance(30_000);
      expect(ipc.refreshWorkspace).toHaveBeenCalledTimes(round + 2);
      const request = ipc.refreshWorkspace.mock.calls[round + 1][1] as RefreshRequest;
      expect(request.repositoryPaths.length).toBeLessThanOrEqual(2);
      expect(request.repositoryPaths).toContain("/repo");
      expect(request.worktreePaths).not.toBeNull();
      expect(request.worktreePaths?.length).toBeLessThanOrEqual(2);
      expect(request.worktreePaths).toContain("/repo");
      expect(request.refreshMetadata).toBe(true);
      for (const path of request.repositoryPaths) visitedRepositories.add(path);
      for (const path of request.worktreePaths ?? []) visitedTrees.add(path);
    }
    expect(visitedRepositories).toEqual(new Set(fixture.repositoryPaths));
    expect(visitedTrees).toEqual(new Set(fixture.worktrees.map((record) => record.path)));
    expect(ipc.measureWorktree).not.toHaveBeenCalled();
  });

  it("lets a mutation callback await its scoped reconciliation and consumers without escalating to all repositories", async () => {
    const fixture = manyRepositories();
    mockInventory(fixture);
    const hook = mount("local", "changes", fixture);
    await flush();
    const inspected = deferred<RefreshResult>();
    const consumer = deferred<void>();
    const wait = vi.fn(() => consumer.promise);
    const unregister = hook.result.current.registerRefreshConsumer(wait);
    ipc.refreshWorkspace.mockImplementationOnce(() => inspected.promise);
    let completed = false;
    let reconciling!: Promise<void>;
    await act(async () => {
      mutation?.({ machineId: "local", repositoryPath: "/repo", worktreePath: "/repo", impact: "files", phase: "start" });
      mutation?.({ machineId: "local", repositoryPath: "/repo", worktreePath: "/repo", impact: "files", phase: "settled" });
      reconciling = hook.result.current.reconcileMutation().then(() => { completed = true; });
    });
    expect(ipc.refreshWorkspace).toHaveBeenCalledTimes(2);
    const request = ipc.refreshWorkspace.mock.calls[1][1] as RefreshRequest;
    expect(request).toEqual({ repositoryPaths: ["/repo"], worktreePaths: ["/repo"], workingCopy: { repositoryPath: "/repo", worktreePath: "/repo" }, refreshMetadata: false });
    expect(completed).toBe(false);
    expect(wait).not.toHaveBeenCalled();
    await act(async () => inspected.resolve(result(request, {
      memberships: fixture.worktrees.filter((record) => record.repositoryPath === "/repo"),
      worktrees: fixture.worktrees.filter((record) => record.path === "/repo"),
    })));
    await flush();
    expect(wait).toHaveBeenCalledTimes(1);
    expect(completed).toBe(false);
    expect(ipc.refreshWorkspace).toHaveBeenCalledTimes(2);
    await act(async () => consumer.resolve());
    await reconciling;
    expect(completed).toBe(true);
    expect(ipc.refreshWorkspace).toHaveBeenCalledTimes(2);
    unregister();
  });
});
