import { useCallback, useEffect, useRef, useState, type Dispatch, type SetStateAction } from "react";
import type { MachineProfile, ScanResult, WorkingCopySnapshot, WorkspaceView } from "../ipc/types";
import {
  measureWorktree, onWorkspaceChanged, onWorkspaceMutation, refreshWorkspace as inspectWorkspace,
  releaseWorkspaceWatch, replaceWorkspaceWatch,
} from "../ipc/worktrees";
import { mergeMeasurement, mergeRefresh } from "../domain/refresh-merge";
import { RefreshQueue, refreshIntent } from "../domain/refresh-queue";
import { toMessage } from "../lib/errors";
import { workingCopySnapshotsEqual } from "../domain/diff-cache";

interface Options {
  machineId: string;
  machineKind: MachineProfile["kind"];
  repositoryPaths: string[];
  scan: ScanResult | null;
  setScan: Dispatch<SetStateAction<ScanResult | null>>;
  repositoryPath: string | null;
  worktreePath: string | null;
  view: WorkspaceView;
  enabled: boolean;
  getEpoch: () => number;
}

interface Publication {
  version: number;
  statuses: Record<string, number>;
  git: Record<string, number>;
  snapshot: WorkingCopySnapshot | null;
  pending: boolean;
  error: string | null;
}

const LOCAL_RECOVERY_MS = 30_000;
const REMOTE_RECOVERY_MS = 10_000;
const MAX_RECOVERY_MS = 5 * 60_000;
const MEASUREMENT_COOLDOWN_MS = 60_000;

/** App-owned reads, subscriptions and recovery, independent of the mounted view. */
export function useWorkspaceRefresh(options: Options) {
  const latest = useRef(options);
  useEffect(() => { latest.current = options; });
  const queue = useRef<RefreshQueue | null>(null);
  const consumers = useRef(new Set<() => Promise<void>>());
  const acknowledged = useRef<Array<{ version: number; resolve: () => void }>>([]);
  const failure = useRef<string | null>(null);
  const validationFailures = useRef(new Map<string, { repositoryPath: string; message: string }>());
  const [publication, setPublication] = useState<Publication>({ version: 0, statuses: {}, git: {}, snapshot: null, pending: false, error: null });
  const [watchError, setWatchError] = useState<string | null>(null);
  const [watchTopology, setWatchTopology] = useState(0);
  const [measurementPending, setMeasurementPending] = useState<ReadonlySet<string>>(new Set());

  useEffect(() => {
    const ready = acknowledged.current.filter((waiter) => waiter.version <= publication.version);
    acknowledged.current = acknowledged.current.filter((waiter) => waiter.version > publication.version);
    for (const waiter of ready) waiter.resolve();
  }, [publication.version]);

  useEffect(() => {
    const machineId = options.machineId;
    let disposed = false;
    let version = 0;
    let readController: AbortController | null = null;
    let measureController: AbortController | null = null;
    let recoveryTimer: ReturnType<typeof setTimeout> | null = null;
    let measurementTimer: ReturnType<typeof setTimeout> | null = null;
    let failures = 0;
    let measuring = false;
    let measurementNotBefore = 0;
    const measured = new Map<string, number>();
    const dirtyMeasurements = new Set<string>();
    const signatures = new Map<string, string>();
    let recoveryCursor = 0;
    const treeCursors = new Map<string, number>();
    const readOnlyRecovery = (all = false) => {
      const current = latest.current;
      const repository = current.repositoryPaths.length ? current.repositoryPaths[recoveryCursor++ % current.repositoryPaths.length] : null;
      const background = current.scan?.worktrees.filter((record) => record.exists && record.repositoryPath === repository && record.path !== current.worktreePath) ?? [];
      const cursor = repository ? treeCursors.get(repository) ?? 0 : 0;
      const target = background.length ? background[cursor % background.length] : null;
      const selected = current.scan?.worktrees.find((record) => record.exists && record.repositoryPath === current.repositoryPath && record.path === current.worktreePath);
      if (repository) treeCursors.set(repository, cursor + 1);
      return refreshIntent({
        all, metadata: true, visible: true,
        repositories: new Set([...(current.repositoryPath ? [current.repositoryPath] : []), ...(repository ? [repository] : [])]),
        // Inventory status stays current in every view; only the Changes view
        // requests the heavier selected working-copy snapshot below.
        worktrees: new Set([...(selected ? [selected.path] : []), ...(target ? [target.path] : [])]),
      });
    };

    const publish = (update: (current: Publication) => Publication): Promise<void> => {
      const acceptedVersion = ++version;
      const committed = new Promise<void>((resolve) => acknowledged.current.push({ version: acceptedVersion, resolve }));
      setPublication((current) => ({ ...update(current), version: acceptedVersion }));
      return committed;
    };

    const scheduleMeasurement = () => {
      if (disposed) return;
      const pending = new Set(latest.current.scan?.worktrees.filter((record) => record.exists && !record.isPrimary && (record.sizeBytes === null || dirtyMeasurements.has(record.id))).map((record) => record.id) ?? []);
      setMeasurementPending((previous) => previous.size === pending.size && [...pending].every((id) => previous.has(id)) ? previous : pending);
      if (disposed || measuring || measurementTimer || document.hidden || !latest.current.enabled) return;
      measurementTimer = setTimeout(() => {
        measurementTimer = null;
        void runMeasurement();
      }, Math.max(2_000, measurementNotBefore - Date.now()));
    };

    const runMeasurement = async () => {
      const current = latest.current;
      if (disposed || measuring || document.hidden || !current.enabled || readController) return;
      const candidates = current.scan?.worktrees.filter((record) => record.exists && !record.isPrimary
        && (record.sizeBytes === null || dirtyMeasurements.has(record.id))
        && Date.now() - (measured.get(record.id) ?? 0) >= MEASUREMENT_COOLDOWN_MS) ?? [];
      // Visible data first; insertion order provides aging for other dirty trees.
      const target = candidates.find((record) => record.path === current.worktreePath) ?? candidates[0];
      if (!target) return;
      measuring = true;
      const started = Date.now();
      const epoch = current.getEpoch();
      const controller = new AbortController();
      measureController = controller;
      try {
        const result = await measureWorktree(machineId, target.repositoryPath, target.path, controller.signal);
        if (!disposed && !controller.signal.aborted && latest.current.getEpoch() === epoch) {
          measured.set(target.id, Date.now());
          dirtyMeasurements.delete(target.id);
          current.setScan((scan) => scan && !disposed && !controller.signal.aborted && latest.current.getEpoch() === epoch ? mergeMeasurement(scan, result, target.id) : scan);
        }
      } catch {
        if (!controller.signal.aborted) measured.set(target.id, Date.now());
      } finally {
        if (measureController === controller) measureController = null;
        measuring = false;
        // Aggregate duty budget: many worktrees cannot turn per-tree cooldowns
        // into continuous traversal. Interrupted walks also consume the budget.
        measurementNotBefore = Date.now() + Math.max(2_000, (Date.now() - started) * 19);
        scheduleMeasurement();
      }
    };

    const scheduleRecovery = () => {
      if (recoveryTimer) clearTimeout(recoveryTimer);
      recoveryTimer = null;
      if (disposed || document.hidden || !latest.current.enabled) return;
      const interval = latest.current.machineKind === "ssh" ? REMOTE_RECOVERY_MS : LOCAL_RECOVERY_MS;
      recoveryTimer = setTimeout(() => {
        recoveryTimer = null;
        scheduler.request(readOnlyRecovery());
      }, Math.min(MAX_RECOVERY_MS, interval * 2 ** Math.min(failures, 5)));
    };

    const scheduler = new RefreshQueue(async (intent, isCurrent) => {
      const current = latest.current;
      if (disposed || current.machineId !== machineId || !current.enabled || current.repositoryPaths.length === 0) return;
      const epoch = current.getEpoch();
      measureController?.abort();
      if (recoveryTimer) clearTimeout(recoveryTimer);
      recoveryTimer = null;
      const requestedPaths = intent.all || intent.repositories.size === 0
        ? current.repositoryPaths
        : [...intent.repositories];
      const paths = current.repositoryPath && requestedPaths.includes(current.repositoryPath)
        ? [current.repositoryPath, ...requestedPaths.filter((path) => path !== current.repositoryPath)]
        : requestedPaths;
      const selected = intent.visible && current.view === "changes" && current.repositoryPath && current.worktreePath
        && (intent.all || paths.includes(current.repositoryPath))
        && (intent.all || intent.metadata || intent.git || intent.manual || intent.worktrees.size === 0 || intent.worktrees.has(current.worktreePath));
      const controller = new AbortController();
      readController = controller;
      try {
        const result = await inspectWorkspace(machineId, {
          repositoryPaths: paths,
          refreshMetadata: intent.all || intent.metadata || intent.git || intent.manual,
          worktreePaths: intent.git || intent.manual ? null : [...new Set([
            ...intent.worktrees,
            ...(selected && current.worktreePath ? [current.worktreePath] : []),
          ])],
          workingCopy: selected ? { repositoryPath: current.repositoryPath as string, worktreePath: current.worktreePath as string } : null,
        }, controller.signal);
        if (!isCurrent() || controller.signal.aborted || latest.current.getEpoch() !== epoch) return;
        const changedGit = new Set<string>();
        const previousRecords = new Map(current.scan?.worktrees.map((record) => [record.id, record]) ?? []);
        for (const member of result.memberships) {
          const previous = previousRecords.get(member.id);
          if (previous && (previous.head !== member.head || previous.branch !== member.branch || previous.exists !== member.exists || previous.registration.kind !== member.registration.kind)) changedGit.add(member.repositoryPath);
        }
        for (const entry of result.gitSignatures) {
          if (signatures.has(entry.repositoryPath) && signatures.get(entry.repositoryPath) !== entry.signature) changedGit.add(entry.repositoryPath);
          signatures.set(entry.repositoryPath, entry.signature);
        }
        // Ref changes detected by recovery need full affected-tree inspection.
        if (changedGit.size > 0 && !intent.git && !intent.manual) {
          scheduler.request(refreshIntent({ repositories: changedGit, git: true, visible: true }));
        }
        const inspectedIds = new Set(result.worktrees.map((record) => record.id));
        const enumerated = new Set(result.successfulRepositoryPaths);
        const liveIds = new Set(result.memberships.map((record) => record.id));
        for (const record of current.scan?.worktrees ?? []) if (!enumerated.has(record.repositoryPath)) liveIds.add(record.id);
        for (const id of dirtyMeasurements) if (!liveIds.has(id)) dirtyMeasurements.delete(id);
        for (const id of measured.keys()) if (!liveIds.has(id)) measured.delete(id);
        const previousIds = new Set(current.scan?.worktrees.map((record) => record.id) ?? []);
        const newTrees = result.memberships.filter((record) => {
          if (inspectedIds.has(record.id)) return false;
          const previous = previousRecords.get(record.id);
          return !previousIds.has(record.id) || previous?.exists !== record.exists || previous?.head !== record.head
            || previous?.branch !== record.branch || previous?.registration.kind !== record.registration.kind;
        });
        if (newTrees.length > 0) scheduler.request(refreshIntent({
          repositories: new Set(newTrees.map((record) => record.repositoryPath)),
          worktrees: new Set(newTrees.map((record) => record.path)),
        }));
        for (const record of result.worktrees) {
          if (!previousRecords.has(record.id) || previousRecords.get(record.id)?.sizeBytes === null || intent.measurements.has(record.path)) dirtyMeasurements.add(record.id);
        }
        const registered = new Set(current.repositoryPaths);
        const memberPaths = new Set(result.memberships.map((record) => record.path));
        for (const [key, error] of validationFailures.current) {
          if (!registered.has(error.repositoryPath)
            || (key.startsWith("tree:") && enumerated.has(error.repositoryPath) && !memberPaths.has(key.slice(5)))) validationFailures.current.delete(key);
        }
        for (const path of result.successfulRepositoryPaths) validationFailures.current.delete(`repo:${path}`);
        for (const record of result.worktrees) validationFailures.current.delete(`tree:${record.path}`);
        for (const error of result.errors) validationFailures.current.set(`${error.worktreePath ? "tree" : "repo"}:${error.worktreePath ?? error.repositoryPath}`, { repositoryPath: error.repositoryPath, message: `${error.worktreePath ?? error.repositoryPath}: ${error.message}` });
        const message = [...validationFailures.current.values()].map((error) => error.message).join("\n") || null;
        failure.current = message;
        failures = result.errors.length ? failures + 1 : 0;
        current.setScan((scan) => isCurrent() && latest.current.getEpoch() === epoch ? mergeRefresh(scan, result, current.repositoryPaths) : scan);
        await publish((state) => {
          const statuses = { ...state.statuses };
          const livePaths = new Set(result.memberships.map((record) => record.path));
          for (const record of current.scan?.worktrees ?? []) if (!enumerated.has(record.repositoryPath)) livePaths.add(record.path);
          for (const path of Object.keys(statuses)) if (!livePaths.has(path)) delete statuses[path];
          const snapshotChanged = result.workingCopy && (!state.snapshot || !workingCopySnapshotsEqual(state.snapshot, result.workingCopy));
          for (const record of result.worktrees) {
            const previous = previousRecords.get(record.id);
            if (!previous || JSON.stringify([previous.head, previous.branch, previous.status, previous.registration, previous.integration, previous.unpushedCommitCount]) !== JSON.stringify([record.head, record.branch, record.status, record.registration, record.integration, record.unpushedCommitCount]) || (snapshotChanged && record.path === result.workingCopy?.worktreePath)) statuses[record.path] = (statuses[record.path] ?? 0) + 1;
          }
          const git = { ...state.git };
          for (const entry of result.gitSignatures) {
            if (changedGit.has(entry.repositoryPath) || intent.git || intent.manual) git[entry.repositoryPath] = (git[entry.repositoryPath] ?? 0) + 1;
          }
          return isCurrent() && latest.current.getEpoch() === epoch
            ? { ...state, statuses, git, snapshot: snapshotChanged ? result.workingCopy : state.snapshot, error: message }
            : state;
        });
      } catch (cause) {
        if (isCurrent() && !controller.signal.aborted && latest.current.getEpoch() === epoch) {
          failure.current = toMessage(cause);
          for (const path of paths) validationFailures.current.set(`repo:${path}`, { repositoryPath: path, message: failure.current });
          failures += 1;
          await publish((state) => ({ ...state, error: failure.current }));
        }
      } finally {
        if (readController === controller) readController = null;
        if (!disposed) {
          scheduleRecovery();
          scheduleMeasurement();
        }
      }
    }, (pending) => { if (!disposed) setPublication((state) => ({ ...state, pending })); });
    queue.current = scheduler;
    failure.current = null;
    validationFailures.current.clear();
    setPublication({ version: 0, statuses: {}, git: {}, snapshot: null, pending: false, error: null });
    setWatchError(null);
    setMeasurementPending(new Set());

    const stopMutation = onWorkspaceMutation((event) => {
      if (event.machineId !== machineId) return;
      if (event.phase === "start") {
        scheduler.mutationStarted();
        measureController?.abort();
        readController?.abort();
        setPublication((state) => ({ ...state, pending: true }));
      } else {
        scheduler.mutationSettled(refreshIntent({
          all: !event.repositoryPath,
          repositories: new Set(event.repositoryPath ? [event.repositoryPath] : []),
          worktrees: new Set(event.worktreePath ? [event.worktreePath] : []),
          measurements: new Set(event.worktreePath ? [event.worktreePath] : []),
          git: event.impact !== "files",
          visible: true,
        }));
      }
    });
    const visibilityChanged = () => {
      if (document.hidden) {
        scheduler.setPaused(true);
        measureController?.abort();
        if (recoveryTimer) clearTimeout(recoveryTimer);
        recoveryTimer = null;
      } else {
        scheduler.request(readOnlyRecovery(true));
        scheduler.setPaused(!latest.current.enabled);
        scheduleMeasurement();
      }
    };
    const focused = () => { if (!document.hidden) scheduler.request(readOnlyRecovery(true)); };
    document.addEventListener("visibilitychange", visibilityChanged);
    window.addEventListener("focus", focused);
    scheduler.setPaused(document.hidden || !latest.current.enabled);
    scheduleRecovery();
    scheduleMeasurement();
    return () => {
      disposed = true;
      scheduler.dispose();
      if (queue.current === scheduler) queue.current = null;
      readController?.abort();
      measureController?.abort();
      if (recoveryTimer) clearTimeout(recoveryTimer);
      if (measurementTimer) clearTimeout(measurementTimer);
      stopMutation();
      document.removeEventListener("visibilitychange", visibilityChanged);
      window.removeEventListener("focus", focused);
      for (const waiter of acknowledged.current.splice(0)) waiter.resolve();
    };
  }, [options.machineId]);

  useEffect(() => {
    const registered = new Set(options.repositoryPaths);
    let removed = false;
    for (const [key, error] of validationFailures.current) {
      if (!registered.has(error.repositoryPath)) {
        validationFailures.current.delete(key);
        removed = true;
      }
    }
    if (removed) {
      failure.current = [...validationFailures.current.values()].map((error) => error.message).join("\n") || null;
      setPublication((state) => ({ ...state, error: failure.current }));
    }
  }, [options.repositoryPaths]);

  const targetKey = JSON.stringify(options.scan?.worktrees.filter((record) => record.exists).map((record) => ({ repositoryPath: record.repositoryPath, worktreePath: record.path })) ?? []);
  useEffect(() => {
    if (!options.enabled || options.machineKind !== "local") return;
    const machineId = options.machineId;
    const subscriptionId = crypto.randomUUID();
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void (async () => {
      try {
        unlisten = await onWorkspaceChanged((event) => {
          if (disposed || event.machineId !== machineId || event.subscriptionId !== subscriptionId) return;
          if (event.kind === "inventory" || event.kind === "overflow") setWatchTopology((revision) => revision + 1);
          queue.current?.request(refreshIntent({
            all: event.kind === "overflow",
            repositories: new Set(event.repositoryPaths),
            worktrees: new Set(event.worktreePaths),
            measurements: event.kind === "git" ? new Set() : new Set(event.worktreePaths),
            git: event.kind !== "files",
            visible: true,
          }));
        });
        if (disposed) { unlisten(); return; }
        const result = await replaceWorkspaceWatch(machineId, subscriptionId, JSON.parse(targetKey));
        if (!disposed && result.active) setWatchError(result.coverageErrors.length ? result.coverageErrors.map((error) => `${error.path}: ${error.message}`).join("\n") : null);
        if (disposed) await releaseWorkspaceWatch(subscriptionId);
      } catch (cause) {
        if (!disposed) setWatchError(toMessage(cause));
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
      void releaseWorkspaceWatch(subscriptionId).catch(() => undefined);
    };
  }, [options.enabled, options.machineId, options.machineKind, targetKey, watchTopology]);

  useEffect(() => {
    queue.current?.setPaused(document.hidden || !options.enabled);
    if (!options.enabled) return;
    // A newly mounted view gets its scoped data immediately, including after
    // bootstrapping or changing the selected machine/repository/worktree.
    queue.current?.request(refreshIntent({
      repositories: new Set(options.repositoryPath ? [options.repositoryPath] : []),
      worktrees: new Set(options.worktreePath ? [options.worktreePath] : []),
      visible: true,
    }));
  }, [options.enabled, options.machineId, options.repositoryPath, options.worktreePath, options.view]);

  const waitForRefresh = useCallback(async () => {
    await queue.current?.waitForIdle();
    const current = latest.current;
    const selectedFailure = validationFailures.current.get(`tree:${current.worktreePath}`)
      ?? validationFailures.current.get(`repo:${current.repositoryPath}`);
    if (selectedFailure) throw new Error(selectedFailure.message);
  }, []);
  const refreshWorkspace = useCallback(async () => {
    if (watchError) setWatchTopology((revision) => revision + 1);
    queue.current?.request(refreshIntent({ all: true, manual: true, visible: true, measurements: new Set(latest.current.scan?.worktrees.filter((record) => !record.isPrimary).map((record) => record.path) ?? []) }));
    try {
      await waitForRefresh();
      await Promise.all([...consumers.current].map((wait) => wait()));
    } catch {
      // The persistent error indicator explains the retained, stale data.
    }
  }, [waitForRefresh, watchError]);
  const reconcileMutation = useCallback(async () => {
    try {
      // Mutation IPC has already queued the exact affected scopes. A view's
      // success callback must not escalate that work into a scan of all repos.
      await waitForRefresh();
      await Promise.all([...consumers.current].map((wait) => wait()));
    } catch {
      // Keep the operation result and expose refresh failure independently.
    }
  }, [waitForRefresh]);
  const registerRefreshConsumer = useCallback((wait: () => Promise<void>) => {
    consumers.current.add(wait);
    return () => { consumers.current.delete(wait); };
  }, []);

  return {
    refreshWorkspace,
    reconcileMutation,
    waitForRefresh,
    registerRefreshConsumer,
    statusRevision: options.worktreePath ? publication.statuses[options.worktreePath] ?? 0 : 0,
    gitRevision: options.repositoryPath ? publication.git[options.repositoryPath] ?? 0 : 0,
    workingCopySnapshot: publication.snapshot?.repositoryPath === options.repositoryPath && publication.snapshot.worktreePath === options.worktreePath ? publication.snapshot : null,
    refreshPending: publication.pending,
    refreshError: publication.error ?? watchError,
    measurementPending,
  };
}
