import { Suspense, useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  AlertTriangleIcon,
  ClockIcon,
  Code2Icon,
  DatabaseIcon,
  FileClockIcon,
  FileDiffIcon,
  FolderOpenIcon,
  FolderPlusIcon,
  GitCommitIcon,
  RefreshCwIcon,
  SearchIcon,
  SearchXIcon,
  SettingsIcon,
  ShieldCheckIcon,
  Trash2Icon,
  WrenchIcon,
  XIcon,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from "@/components/ui/input-group";
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { toast } from "@/components/ui/toast";
import { TooltipButton } from "@/components/tooltip-button";
import { ErrorBoundary } from "@/components/error-boundary";
import { cn } from "@/lib/utils";
import { toMessage } from "@/lib/errors";
import { ActionDialog } from "../dialogs/ActionDialog";
import { CommandPalette, type CommandPaletteItem } from "../dialogs/CommandPalette";
import type { BulkItem, BulkStage } from "../dialogs/BulkActionDialog";
import { SettingsDialog } from "../dialogs/SettingsDialog";
import { RepositoryDialog } from "../dialogs/RepositoryDialog";
import { CreateWorktreeDialog } from "../dialogs/CreateWorktreeDialog";
import { inventoryGridClass, WorktreeRow } from "../workspace/WorktreeRow";
import { useShortPath } from "./environment";
import { formatAge, formatBytes, formatMeasuredBytes } from "../domain/format";
import { performFocusedSelectAll } from "../domain/select-all";
import { actionForWorktree, computeTotals, filterAndSortWorktrees, isRemovable } from "../domain/inventory";
import { batchDeletionRefusal, deletionOutcomeUnknown, settleFollowUps, type Removal } from "../domain/branch-deletion-outcomes";
import type { AgeFilter, StateFilter } from "../domain/inventory";
import type {
  ActionKind,
  ActionPlan,
  AgentInfo,
  BranchDeletionPlan,
  BranchDeletionResult,
  BranchInfo,
  FollowUpAction,
  MachineProfile,
  MachineProfileInput,
  RemoteProvider,
  RepositorySummary,
  ScanResult,
  WorktreeRecord,
  WorkspaceContext,
  WorkspaceFilters,
  WorkspaceLayout,
  WorkspaceView,
} from "../ipc/types";
import { deleteMachine, loadMachines, moveMachine, saveMachine, testMachine } from "../ipc/machines";
import { loadWorkspaceContext, saveWorkspaceContext } from "../ipc/preferences";
import { launchWorktreeTool } from "../ipc/app-preferences";
import { restoreAndTrackWindow } from "./window-state";
import {
  executeBranchDeletion,
  executeWorktreeAction,
  fetchPullRequests,
  loadBranches,
  loadRegisteredRepositories,
  pickRepositoryRoot,
  prepareBranchDeletionReview,
  prepareWorktreeAction,
  revealAuditLog,
  resolveDroppedRepository,
  registerRepository,
  scanWorktrees,
  unregisterRepository,
} from "../ipc/worktrees";
import { RepositoryProvider, type RepositoryContextValue } from "../workspace/context";
import { AppSidebar, type MachineConnection } from "../workspace/AppSidebar";
import { RepositoryToolbar } from "../workspace/RepositoryToolbar";
import { ChangesWorkbench } from "../workspace/ChangesWorkbench";
import { HistoryWorkbench } from "../workspace/lazy";
import { WorktreeDetails, type PullState } from "../workspace/WorktreeDetails";
import { PaneResizeHandle } from "../workspace/PaneResizeHandle";
import { LazyDialog } from "../workspace/LazyDialog";
import { BulkActionDialog, DeleteBranchDialog, DiffDialog } from "../workspace/lazy";
import { RepositoryDropZone } from "../components/RepositoryDropZone";
import { useRepositoryDrop } from "./use-repository-drop";

const stateOptions: { value: StateFilter; label: string }[] = [
  { value: "all", label: "Every state" },
  { value: "clean", label: "Clean" },
  { value: "changed", label: "Local changes" },
  { value: "attention", label: "Needs attention" },
];
const stateItems = Object.fromEntries(stateOptions.map((option) => [option.value, option.label]));

type BulkPlan = { kind: "remove"; plan: ActionPlan } | { kind: "deleteBranch"; plan: BranchDeletionPlan };

interface BulkEntry extends BulkItem {
  plan: BulkPlan | null;
}

interface BulkState {
  kind: BulkPlan["kind"];
  stage: BulkStage;
  items: BulkEntry[];
  followUps: FollowUpAction[];
}

const unreviewed = (error: string) => ({ plan: null, command: null, warnings: [], error });

/** A branch a removed worktree left behind, open in the branch deletion review. */
interface BranchReview {
  machineId: string;
  followUp: FollowUpAction;
  branches: BranchInfo[];
}

function App() {
  const shortPath = useShortPath();
  const [scan, setScan] = useState<ScanResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  const [repository, setRepository] = useState("all");
  const [workspaceView, setWorkspaceView] = useState<WorkspaceView>("changes");
  const [currentRepositoryPath, setCurrentRepositoryPath] = useState<string | null>(null);
  const [currentWorktreePath, setCurrentWorktreePath] = useState<string | null>(null);
  const [age, setAge] = useState<AgeFilter>(0);
  const [state, setState] = useState<StateFilter>("all");
  // null until persistence is hydrated; [] means the user has not added a repository yet.
  const [registeredRepositories, setRegisteredRepositories] = useState<string[] | null>(null);
  const [machines, setMachines] = useState<MachineProfile[] | null>(null);
  const [workspaceLocations, setWorkspaceLocations] = useState<WorkspaceContext["locations"]>({});
  const [workspaceFilters, setWorkspaceFilters] = useState<WorkspaceContext["filters"]>({});
  const [workspaceLayout, setWorkspaceLayout] = useState<WorkspaceLayout>({ inventorySidebarWidth: 230, detailsWidth: 360 });
  const [workspaceContextHydrated, setWorkspaceContextHydrated] = useState(false);
  const [selectedMachineId, setSelectedMachineId] = useState("local");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [repositoryDialogOpen, setRepositoryDialogOpen] = useState(false);
  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false);
  const [worktreeContext, setWorktreeContext] = useState<{ worktree: WorktreeRecord; x: number; y: number } | null>(null);
  const [createWorktreeOpen, setCreateWorktreeOpen] = useState(false);
  const [repositoriesBusy, setRepositoriesBusy] = useState(false);
  const [machinesBusy, setMachinesBusy] = useState(false);
  const [machineConnections, setMachineConnections] = useState<Record<string, MachineConnection>>({});
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [checked, setChecked] = useState<ReadonlySet<string>>(new Set());
  const [bulk, setBulk] = useState<BulkState | null>(null);
  const [bulkBusy, setBulkBusy] = useState(false);
  const [actionPlan, setActionPlan] = useState<ActionPlan | null>(null);
  const [branchReview, setBranchReview] = useState<BranchReview | null>(null);
  const [actionBusy, setActionBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pullEvidence, setPullEvidence] = useState<Record<string, PullState>>({});
  const [diffWorktree, setDiffWorktree] = useState<WorktreeRecord | null>(null);
  const [auditPath, setAuditPath] = useState<string | null>(null);
  const didStart = useRef(false);
  const refreshGeneration = useRef(0);
  const activeMachineId = useRef("local");
  const refreshController = useRef<AbortController | null>(null);
  const connectionGenerations = useRef<Record<string, number>>({});
  const workspaceSaveChain = useRef<Promise<unknown>>(Promise.resolve());
  const now = scan?.scannedAtMs ?? Date.now();
  const repositoryPaths = useMemo(() => registeredRepositories ?? [], [registeredRepositories]);
  const selectedMachine = machines?.find((machine) => machine.id === selectedMachineId) ?? null;
  const selectedConnection = machineConnections[selectedMachineId] ?? null;

  const probeMachineConnection = useCallback(async (
    machineId: string,
    notifyOnFailure = false,
  ): Promise<AgentInfo | null> => {
    const generation = (connectionGenerations.current[machineId] ?? 0) + 1;
    connectionGenerations.current[machineId] = generation;
    setMachineConnections((current) => ({
      ...current,
      [machineId]: {
        status: "checking",
        latencyMs: current[machineId]?.latencyMs ?? null,
        checkedAtMs: current[machineId]?.checkedAtMs ?? null,
        lastConnectedAtMs: current[machineId]?.lastConnectedAtMs ?? null,
        agent: current[machineId]?.agent ?? null,
        error: null,
      },
    }));
    const started = performance.now();
    try {
      const agent = await testMachine(machineId);
      if (connectionGenerations.current[machineId] !== generation) return agent;
      const now = Date.now();
      setMachineConnections((current) => ({
        ...current,
        [machineId]: {
          status: "online",
          latencyMs: Math.max(0, Math.round(performance.now() - started)),
          checkedAtMs: now,
          lastConnectedAtMs: now,
          agent,
          error: null,
        },
      }));
      return agent;
    } catch (cause) {
      const message = toMessage(cause);
      if (connectionGenerations.current[machineId] === generation) {
        setMachineConnections((current) => ({
          ...current,
          [machineId]: {
            status: "offline",
            latencyMs: current[machineId]?.latencyMs ?? null,
            checkedAtMs: Date.now(),
            lastConnectedAtMs: current[machineId]?.lastConnectedAtMs ?? null,
            agent: current[machineId]?.agent ?? null,
            error: message,
          },
        }));
      }
      if (notifyOnFailure) {
        toast.add({ type: "error", title: "Could not connect to machine", description: message });
      }
      return null;
    }
  }, []);

  const refresh = useCallback(async (machineId: string, paths: string[]) => {
    refreshController.current?.abort();
    const controller = new AbortController();
    refreshController.current = controller;
    const generation = ++refreshGeneration.current;
    if (paths.length === 0) {
      // Nothing is configured: there is nothing to scan and nothing to guess.
      if (generation === refreshGeneration.current) {
        setScan(null);
        setError(null);
        setLoading(false);
      }
      if (refreshController.current === controller) refreshController.current = null;
      return;
    }
    setLoading(true);
    setError(null);
    const records = new Map<string, WorktreeRecord>();
    let meta: { repositoryPaths: string[]; repositories: RepositorySummary[]; warnings: string[] } | null = null;
    let settled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const publishPartial = () => {
      timer = null;
      const partialMeta = meta;
      if (settled || !partialMeta || generation !== refreshGeneration.current) return;
      const worktrees = Array.from(records.values());
      setScan({
        scannedAtMs: Date.now(),
        repositoryPaths: partialMeta.repositoryPaths,
        repositories: partialMeta.repositories,
        worktrees,
        totals: computeTotals(worktrees, partialMeta.repositories.length),
        warnings: partialMeta.warnings,
      });
    };
    try {
      const next = await scanWorktrees(machineId, paths, (event) => {
        if (generation !== refreshGeneration.current) return;
        if (event.type === "repositories") {
          meta = { repositoryPaths: event.repositoryPaths, repositories: event.repositories, warnings: event.warnings };
        } else if (event.type === "worktree") {
          records.set(event.record.id, event.record);
        } else if (event.type === "size") {
          const record = records.get(event.id);
          if (record) {
            records.set(event.id, {
              ...record,
              sizeBytes: event.sizeBytes,
              sizeIncomplete: event.sizeIncomplete,
              lastActivityAtMs: event.lastActivityAtMs,
            });
          }
        } else {
          settled = true;
        }
        if (!timer && !settled) timer = setTimeout(publishPartial, 120);
      }, controller.signal);
      settled = true;
      if (generation !== refreshGeneration.current) return;
      setScan(next);
      setPullEvidence({});
      setSelectedId((current) => current && next.worktrees.some((item) => item.id === current)
        ? current
        : null);
      setChecked((current) => new Set(
        next.worktrees.filter((item) => current.has(item.id) && isRemovable(item)).map((item) => item.id),
      ));
    } catch (cause) {
      if (generation === refreshGeneration.current && !controller.signal.aborted) setError(toMessage(cause));
    } finally {
      settled = true;
      if (timer) clearTimeout(timer);
      if (generation === refreshGeneration.current) setLoading(false);
      if (refreshController.current === controller) refreshController.current = null;
    }
  }, []);

  const refreshWorkspace = useCallback(
    () => refresh(selectedMachineId, repositoryPaths),
    [refresh, repositoryPaths, selectedMachineId],
  );
  const showChanges = useCallback(() => setWorkspaceView("changes"), []);

  useEffect(() => () => refreshController.current?.abort(), []);

  useEffect(() => {
    let dispose: (() => void) | null = null;
    void restoreAndTrackWindow().then((cleanup) => { dispose = cleanup; }).catch(() => undefined);
    return () => { dispose?.(); };
  }, []);

  useEffect(() => {
    if (didStart.current) return;
    didStart.current = true;
    void (async () => {
      try {
        const [storedMachines, storedContext] = await Promise.all([loadMachines(), loadWorkspaceContext()]);
        const machineId = storedMachines.some((machine) => machine.id === storedContext.selectedMachineId && machine.enabled)
          ? storedContext.selectedMachineId
          : "local";
        const location = storedContext.locations[machineId];
        const filters = storedContext.filters[machineId];
        const stored = await loadRegisteredRepositories(machineId);
        activeMachineId.current = machineId;
        setSelectedMachineId(machineId);
        setWorkspaceLocations(storedContext.locations);
        setWorkspaceFilters(storedContext.filters);
        setWorkspaceLayout(storedContext.layout);
        setQuery(filters?.query ?? "");
        setRepository(filters?.repositoryPath ?? "all");
        setAge((filters?.ageDays ?? 0) as AgeFilter);
        setState(filters?.state ?? "all");
        setCurrentRepositoryPath(location?.repositoryPath ?? null);
        setCurrentWorktreePath(location?.worktreePath ?? null);
        setWorkspaceView(location?.view ?? "changes");
        setRegisteredRepositories(stored);
        setMachines(storedMachines);
        setWorkspaceContextHydrated(true);
        void probeMachineConnection(machineId);
        await refresh(machineId, stored);
      } catch (cause) {
        setRegisteredRepositories([]);
        setMachines([]);
        setError(toMessage(cause));
        setLoading(false);
      }
    })();
  }, [probeMachineConnection, refresh]);

  const switchMachine = async (machineId: string) => {
    if (machineId === selectedMachineId) return;
    const profile = machines?.find((machine) => machine.id === machineId && machine.enabled);
    if (!profile) return;
    const generation = ++refreshGeneration.current;
    refreshController.current?.abort();
    activeMachineId.current = machineId;
    setSelectedMachineId(machineId);
    setLoading(true);
    setError(null);
    setScan(null);
    setSelectedId(null);
    setChecked(new Set());
    setPullEvidence({});
    setActionPlan(null);
    setActionError(null);
    setBranchReview(null);
    setDiffWorktree(null);
    const location = workspaceLocations[machineId];
    const filters = workspaceFilters[machineId];
    setQuery(filters?.query ?? "");
    setRepository(filters?.repositoryPath ?? "all");
    setAge((filters?.ageDays ?? 0) as AgeFilter);
    setState(filters?.state ?? "all");
    setCurrentRepositoryPath(location?.repositoryPath ?? null);
    setCurrentWorktreePath(location?.worktreePath ?? null);
    setWorkspaceView(location?.view ?? "changes");
    void probeMachineConnection(machineId);
    try {
      const stored = await loadRegisteredRepositories(machineId);
      if (generation !== refreshGeneration.current || activeMachineId.current !== machineId) return;
      setRegisteredRepositories(stored);
      await refresh(machineId, stored);
    } catch (cause) {
      if (generation !== refreshGeneration.current || activeMachineId.current !== machineId) return;
      setRegisteredRepositories([]);
      setError(toMessage(cause));
      setLoading(false);
    }
  };

  const filtered = useMemo(() => {
    return filterAndSortWorktrees(scan?.worktrees ?? [], {
      age,
      query,
      repositoryPath: repository,
      state,
    }, now);
  }, [age, now, query, repository, scan?.worktrees, state]);

  const currentRepository = scan?.repositories.find((item) => item.path === currentRepositoryPath)
    ?? scan?.repositories[0]
    ?? null;
  const repositoryWorktrees = useMemo(
    () => (scan?.worktrees ?? []).filter((item) => item.repositoryPath === currentRepository?.path && item.exists),
    [currentRepository?.path, scan?.worktrees],
  );
  const currentWorktree = repositoryWorktrees.find((item) => item.path === currentWorktreePath)
    ?? repositoryWorktrees.find((item) => item.isPrimary)
    ?? repositoryWorktrees[0]
    ?? null;

  const selectedMachineOs = selectedMachine?.kind === "ssh" ? selectedConnection?.agent?.operatingSystem ?? null : null;
  const repositoryContext = useMemo<RepositoryContextValue>(() => ({
    machineId: selectedMachineId,
    machineKind: selectedMachine?.kind ?? "local",
    machineOs: selectedMachineOs,
    repository: currentRepository,
    worktree: currentWorktree,
    refreshWorkspace,
    showChanges,
    recordAuditPath: setAuditPath,
  }), [currentRepository, currentWorktree, refreshWorkspace, selectedMachine?.kind, selectedMachineId, selectedMachineOs, showChanges]);

  useEffect(() => {
    if (!workspaceContextHydrated) return;
    setWorkspaceLocations((current) => {
      const previous = current[selectedMachineId];
      if (
        previous?.repositoryPath === currentRepositoryPath
        && previous.worktreePath === currentWorktreePath
        && previous.view === workspaceView
      ) return current;
      return {
        ...current,
        [selectedMachineId]: {
          repositoryPath: currentRepositoryPath,
          worktreePath: currentWorktreePath,
          view: workspaceView,
        },
      };
    });
  }, [currentRepositoryPath, currentWorktreePath, selectedMachineId, workspaceContextHydrated, workspaceView]);

  useEffect(() => {
    if (!workspaceContextHydrated) return;
    const context: WorkspaceContext = { selectedMachineId, locations: workspaceLocations, filters: workspaceFilters, layout: workspaceLayout };
    const timeout = setTimeout(() => {
      workspaceSaveChain.current = workspaceSaveChain.current
        .catch(() => undefined)
        .then(() => saveWorkspaceContext(context))
        .catch((cause: unknown) => {
          toast.add({ type: "error", title: "Could not save workspace position", description: toMessage(cause) });
        });
    }, 250);
    return () => clearTimeout(timeout);
  }, [selectedMachineId, workspaceContextHydrated, workspaceFilters, workspaceLayout, workspaceLocations]);

  useEffect(() => {
    if (!workspaceContextHydrated) return;
    const filters: WorkspaceFilters = { query, repositoryPath: repository, ageDays: age, state };
    setWorkspaceFilters((current) => {
      const previous = current[selectedMachineId];
      if (previous?.query === filters.query
        && previous.repositoryPath === filters.repositoryPath
        && previous.ageDays === filters.ageDays
        && previous.state === filters.state) return current;
      return { ...current, [selectedMachineId]: filters };
    });
  }, [age, query, repository, selectedMachineId, state, workspaceContextHydrated]);

  useEffect(() => {
    if (loading || !scan) return;
    if (currentRepository && currentRepository.path !== currentRepositoryPath) {
      setCurrentRepositoryPath(currentRepository.path);
    }
    if (currentWorktree && currentWorktree.path !== currentWorktreePath) {
      setCurrentWorktreePath(currentWorktree.path);
    }
  }, [currentRepository, currentRepositoryPath, currentWorktree, currentWorktreePath, loading, scan]);

  const selected = filtered.find((item) => item.id === selectedId) ?? filtered[0] ?? null;
  const effectiveSelectedId = selected?.id ?? null;
  const filtersActive = query !== "" || repository !== "all" || age !== 0 || state !== "all";
  const clearFilters = () => {
    setQuery("");
    setRepository("all");
    setAge(0);
    setState("all");
  };

  const filteredRemovable = useMemo(() => filtered.filter(isRemovable), [filtered]);
  const checkedRecords = useMemo(
    () => (scan?.worktrees ?? []).filter((item) => checked.has(item.id) && isRemovable(item)),
    [checked, scan?.worktrees],
  );
  const checkedBytes = checkedRecords.reduce((sum, item) => sum + (item.sizeBytes ?? 0), 0);
  const allFilteredChecked = filteredRemovable.length > 0
    && filteredRemovable.every((item) => checked.has(item.id));

  const toggleChecked = useCallback((id: string) => {
    setChecked((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const toggleAllFiltered = () => {
    setChecked((current) => {
      const next = new Set(current);
      if (allFilteredChecked) filteredRemovable.forEach((item) => next.delete(item.id));
      else filteredRemovable.forEach((item) => next.add(item.id));
      return next;
    });
  };

  const reviewAction = async (kind: ActionKind, worktree: WorktreeRecord) => {
    const machineId = activeMachineId.current;
    setActionBusy(true);
    setActionError(null);
    try {
      const plan = await prepareWorktreeAction(machineId, kind, worktree.repositoryPath, worktree.path);
      if (activeMachineId.current === machineId) setActionPlan(plan);
    } catch (cause) {
      setActionError(toMessage(cause));
    } finally {
      setActionBusy(false);
    }
  };

  const executeAction = async () => {
    if (!actionPlan) return;
    setActionBusy(true);
    setActionError(null);
    try {
      const result = await executeWorktreeAction(selectedMachineId, actionPlan);
      setActionPlan(null);
      if (result.auditPath) setAuditPath(result.auditPath);
      const followUp = result.followUp;
      toast.add({
        type: "success",
        title: result.message,
        description: result.auditWarning ? `Audit warning: ${result.auditWarning}` : undefined,
        actionProps: followUp
          ? { children: "Review Branch Deletion…", onClick: () => void reviewFollowUp(followUp) }
          : undefined,
      });
      await refreshWorkspace();
    } catch (cause) {
      setActionError(toMessage(cause));
    } finally {
      setActionBusy(false);
    }
  };

  const reviewFollowUp = async (followUp: FollowUpAction) => {
    const machineId = activeMachineId.current;
    setActionBusy(true);
    try {
      const branches = await loadBranches(machineId, followUp.repositoryPath, followUp.worktreePath);
      if (activeMachineId.current !== machineId) return;
      setBranchReview({ machineId, followUp, branches });
    } catch (cause) {
      toast.add({
        type: "error",
        title: `Branch ${followUp.branch} was not prepared for deletion`,
        description: toMessage(cause),
      });
    } finally {
      setActionBusy(false);
    }
  };

  const followUpBranchDeleted = async (result: BranchDeletionResult) => {
    if (result.auditPath) setAuditPath(result.auditPath);
    await refreshWorkspace();
  };

  const reviewBulkRemoval = async () => {
    if (checkedRecords.length === 0) return;
    setBulkBusy(true);
    try {
      const items = await Promise.all(checkedRecords.map(async (worktree): Promise<BulkEntry> => {
        const base = {
          key: worktree.id,
          title: worktree.branch ?? worktree.path,
          subtitle: shortPath(worktree.path),
          sizeLabel: formatMeasuredBytes(worktree.sizeBytes, worktree.sizeIncomplete),
          done: false,
        };
        try {
          const plan = await prepareWorktreeAction(selectedMachineId, "remove", worktree.repositoryPath, worktree.path);
          return { ...base, plan: { kind: "remove", plan }, command: plan.commandDisplay, warnings: plan.warnings, error: null };
        } catch (cause) {
          return { ...base, ...unreviewed(toMessage(cause)) };
        }
      }));
      setBulk({ kind: "remove", stage: "review", items, followUps: [] });
    } finally {
      setBulkBusy(false);
    }
  };

  const reviewBulkBranchDeletion = async (followUps: FollowUpAction[]) => {
    setBulkBusy(true);
    try {
      const items = await Promise.all(followUps.map(async (followUp): Promise<BulkEntry> => {
        const base = {
          key: `${followUp.repositoryPath}::${followUp.branch}`,
          title: followUp.branch,
          subtitle: shortPath(followUp.repositoryPath),
          done: false,
        };
        try {
          const plan = await prepareBranchDeletionReview(selectedMachineId, {
            repositoryPath: followUp.repositoryPath,
            worktreePath: followUp.worktreePath,
            branchRef: `refs/heads/${followUp.branch}`,
            deleteLocal: true,
            deleteRemote: false,
          });
          const refusal = batchDeletionRefusal(plan);
          if (refusal) return { ...base, ...unreviewed(refusal) };
          return { ...base, plan: { kind: "deleteBranch", plan }, command: plan.commands.join("\n"), warnings: plan.warnings, error: null };
        } catch (cause) {
          return { ...base, ...unreviewed(toMessage(cause)) };
        }
      }));
      setBulk({ kind: "deleteBranch", stage: "review", items, followUps: [] });
    } finally {
      setBulkBusy(false);
    }
  };

  const executeBulk = async () => {
    if (!bulk || bulk.stage !== "review") return;
    const kind = bulk.kind;
    const items = bulk.items.map((item) => ({ ...item }));
    const removals: Removal[] = [];
    setBulk({ kind, stage: "running", items, followUps: [] });
    for (let index = 0; index < items.length; index += 1) {
      const item = items[index];
      if (!item.plan || item.error || item.done) continue;
      try {
        let deletion: BranchDeletionResult | undefined;
        if (item.plan.kind === "remove") {
          const result = await executeWorktreeAction(selectedMachineId, item.plan.plan);
          if (result.auditPath) setAuditPath(result.auditPath);
          removals.push({ repositoryPath: item.plan.plan.repositoryPath, followUp: result.followUp });
        } else {
          deletion = await executeBranchDeletion(selectedMachineId, item.plan.plan, null);
          if (deletion.auditPath) setAuditPath(deletion.auditPath);
        }
        // A deletion interrupted after it started may have happened.
        const unconfirmed = [deletion?.local, deletion?.remote].some((step) => step?.unconfirmed === true);
        items[index] = unconfirmed && deletion
          ? { ...item, deletion, unconfirmed, error: deletion.message }
          : { ...item, done: true, deletion };
      } catch (cause) {
        items[index] = { ...item, error: toMessage(cause), unconfirmed: item.plan.kind === "deleteBranch" && deletionOutcomeUnknown(cause) };
      }
      setBulk({ kind, stage: "running", items: items.map((entry) => ({ ...entry })), followUps: [] });
    }
    setBulk({ kind, stage: "done", items, followUps: settleFollowUps(removals) });
    setChecked(new Set());
    await refreshWorkspace();
  };

  const checkPullRequests = useCallback(async (worktree: WorktreeRecord) => {
    if (!worktree.branch) return;
    setPullEvidence((current) => ({ ...current, [worktree.id]: "loading" }));
    try {
      const evidence = await fetchPullRequests(selectedMachineId, worktree.repositoryPath, worktree.branch);
      setPullEvidence((current) => ({ ...current, [worktree.id]: evidence }));
    } catch (cause) {
      setPullEvidence((current) => ({
        ...current,
        [worktree.id]: {
          status: "cliError",
          provider: "other",
          branch: worktree.branch ?? "",
          pulls: [],
          detail: toMessage(cause),
        },
      }));
    }
  }, [selectedMachineId]);

  const registerAndSelectRepository = async (path: string): Promise<boolean> => {
    setRepositoriesBusy(true);
    try {
      const result = await registerRepository(selectedMachineId, path);
      setRegisteredRepositories(result.registeredRepositories);
      await refresh(selectedMachineId, result.registeredRepositories);
      setCurrentRepositoryPath(result.repositoryPath);
      setCurrentWorktreePath(null);
      setWorkspaceView("changes");
      return true;
    } catch (cause) {
      toast.add({ type: "error", title: "Could not add repository", description: toMessage(cause) });
      return false;
    } finally {
      setRepositoriesBusy(false);
    }
  };

  const addExistingRepository = async (remotePath?: string): Promise<boolean> => {
    try {
      const selectedPath = selectedMachine?.kind === "ssh"
        ? remotePath?.trim() ?? null
        : await pickRepositoryRoot();
      if (!selectedPath) return false;
      return registerAndSelectRepository(selectedPath);
    } catch (cause) {
      toast.add({ type: "error", title: "Could not choose a repository", description: toMessage(cause) });
      return false;
    }
  };

  const completeRepositoryOnboarding = async (repositoryPath: string): Promise<boolean> => {
    if (!await registerAndSelectRepository(repositoryPath)) return false;
    toast.add({ type: "success", title: "Repository ready", description: repositoryPath });
    return true;
  };

  const removeRepositoryFromList = async (repositoryPath: string): Promise<void> => {
    setRepositoriesBusy(true);
    try {
      const nextRepositories = await unregisterRepository(selectedMachineId, repositoryPath);
      setRegisteredRepositories(nextRepositories);
      if (currentRepositoryPath === repositoryPath) {
        setCurrentRepositoryPath(null);
        setCurrentWorktreePath(null);
      }
      await refresh(selectedMachineId, nextRepositories);
      toast.add({ type: "success", title: "Repository removed from Repola", description: "Files and Git data were not changed. Add the repository again at any time." });
    } catch (cause) {
      toast.add({ type: "error", title: "Could not remove repository from the list", description: toMessage(cause) });
    } finally {
      setRepositoriesBusy(false);
    }
  };

  const updateMachine = async (input: MachineProfileInput): Promise<boolean> => {
    setMachinesBusy(true);
    try {
      setMachines(await saveMachine(input));
      return true;
    } catch (cause) {
      toast.add({ type: "error", title: "Could not save machine", description: toMessage(cause) });
      return false;
    } finally {
      setMachinesBusy(false);
    }
  };

  const removeSshMachine = async (machineId: string): Promise<boolean> => {
    setMachinesBusy(true);
    try {
      const nextMachines = await deleteMachine(machineId);
      setMachines(nextMachines);
      setWorkspaceLocations((current) => {
        const remaining = { ...current };
        delete remaining[machineId];
        return remaining;
      });
      if (machineId === selectedMachineId) await switchMachine("local");
      return true;
    } catch (cause) {
      toast.add({ type: "error", title: "Could not remove machine", description: toMessage(cause) });
      return false;
    } finally {
      setMachinesBusy(false);
    }
  };

  const reorderMachine = async (machineId: string, delta: -1 | 1): Promise<void> => {
    setMachinesBusy(true);
    try {
      setMachines(await moveMachine(machineId, delta));
    } catch (cause) {
      toast.add({ type: "error", title: "Could not reorder machines", description: toMessage(cause) });
    } finally {
      setMachinesBusy(false);
    }
  };

  const testSshMachine = async (machineId: string): Promise<AgentInfo | null> => {
    return probeMachineConnection(machineId, true);
  };

  const performMenuAction = useCallback((action: string) => {
    if (action === "settings") setSettingsOpen(true);
    else if (action === "add-repository") setRepositoryDialogOpen(true);
    else if (action === "command-palette") setCommandPaletteOpen(true);
    else if (action === "refresh") void refreshWorkspace();
    else if (action === "view-changes") setWorkspaceView("changes");
    else if (action === "view-history") setWorkspaceView("history");
    else if (action === "view-worktrees") setWorkspaceView("worktrees");
    else if (action === "changes-select-all") performFocusedSelectAll();
  }, [refreshWorkspace]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.altKey || event.shiftKey) return;
      const key = event.key.toLowerCase();
      const action = event.key === "," ? "settings"
        : key === "k" ? "command-palette"
          : key === "a" ? "changes-select-all"
          : event.key === "1" ? "view-changes"
            : event.key === "2" ? "view-history"
              : event.key === "3" ? "view-worktrees"
                : null;
      if (!action) return;
      event.preventDefault();
      performMenuAction(action);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [performMenuAction]);

  useEffect(() => {
    let dispose: (() => void) | null = null;
    void listen<string>("repola://menu-action", (event) => performMenuAction(event.payload)).then((unlisten) => { dispose = unlisten; });
    return () => { dispose?.(); };
  }, [performMenuAction]);

  const repositoryDropsBlocked = loading || repositoriesBusy || machinesBusy || actionBusy || bulkBusy
    || settingsOpen || commandPaletteOpen || createWorktreeOpen || bulk !== null || actionPlan !== null || branchReview !== null || diffWorktree !== null;
  const addDroppedRepositories = async (paths: string[]): Promise<boolean> => {
    if (selectedMachine?.kind !== "local" || repositoryDropsBlocked) return false;
    setRepositoriesBusy(true);
    try {
      const resolved = await Promise.allSettled(paths.map((path) => resolveDroppedRepository(path)));
      const repositories = Array.from(new Set(resolved.flatMap((result) => result.status === "fulfilled" ? [result.value] : [])));
      const failures = resolved.filter((result): result is PromiseRejectedResult => result.status === "rejected");
      if (repositories.length === 0) {
        toast.add({ type: "error", title: "No Git repository was added", description: failures.length ? toMessage(failures[0].reason) : "Drop a repository folder or a file inside a working copy." });
        return false;
      }
      if (activeMachineId.current !== "local") return false;
      let nextRepositories = repositoryPaths;
      for (const repository of repositories) {
        const result = await registerRepository("local", repository);
        nextRepositories = result.registeredRepositories;
      }
      // A pending local drop must not replace another machine's inventory.
      if (activeMachineId.current !== "local") return false;
      setRegisteredRepositories(nextRepositories);
      setCurrentRepositoryPath(repositories[0]);
      setCurrentWorktreePath(null);
      setWorkspaceView("changes");
      await refresh("local", nextRepositories);
      toast.add({ type: "success", title: repositories.length === 1 ? "Repository added" : `${repositories.length} repositories added`, description: repositories.join("\n") });
      if (failures.length) toast.add({ type: "warning", title: "Some dropped items could not be added", description: failures.map((failure) => toMessage(failure.reason)).join("\n") });
      return true;
    } catch (cause) {
      toast.add({ type: "error", title: "Could not add dropped repositories", description: toMessage(cause) });
      return false;
    } finally {
      setRepositoriesBusy(false);
    }
  };
  const emptyRepositoryDropEnabled = selectedMachine?.kind === "local" && registeredRepositories?.length === 0
    && !scan && !error && !repositoryDialogOpen && !repositoryDropsBlocked;
  const { ref: emptyRepositoryDropRef, active: emptyRepositoryDropActive } = useRepositoryDrop(emptyRepositoryDropEnabled, addDroppedRepositories);

  useEffect(() => {
    if (!worktreeContext) return;
    const close = () => setWorktreeContext(null);
    window.addEventListener("pointerdown", close);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("pointerdown", close);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
    };
  }, [worktreeContext]);

  const settingsDialog = settingsOpen && machines !== null && (
    <SettingsDialog
      machines={machines}
      selectedMachineId={selectedMachineId}
      busy={machinesBusy}
      onClose={() => setSettingsOpen(false)}
      onRemoveMachine={removeSshMachine}
      onMoveMachine={reorderMachine}
      onSaveMachine={updateMachine}
      onTestMachine={testSshMachine}
    />
  );

  const repositoryDialog = repositoryDialogOpen && selectedMachine ? (
    <RepositoryDialog
      machine={selectedMachine}
      onAddExisting={addExistingRepository}
      onCompleted={completeRepositoryOnboarding}
      onDropRepositories={addDroppedRepositories}
      dropDisabled={repositoryDropsBlocked}
      onClose={() => setRepositoryDialogOpen(false)}
    />
  ) : null;

  const paletteCommands: CommandPaletteItem[] = [
    { id: "add", label: "Add Repository…", detail: `Add, clone, or create on ${selectedMachine?.name ?? "the selected machine"}.`, shortcut: "⌘O", run: () => setRepositoryDialogOpen(true) },
    { id: "refresh", label: "Refresh", detail: "Refresh repositories, worktrees, and Git state.", shortcut: "⌘R", run: () => void refreshWorkspace() },
    { id: "changes", label: "Show Changes", detail: "Review and commit the selected working copy.", shortcut: "⌘1", run: () => setWorkspaceView("changes") },
    { id: "history", label: "Show History", detail: "Browse commits and history actions.", shortcut: "⌘2", run: () => setWorkspaceView("history") },
    { id: "worktrees", label: "Show Worktrees", detail: "Review linked worktrees and cleanup evidence.", shortcut: "⌘3", run: () => setWorkspaceView("worktrees") },
    { id: "editor", label: "Open in Editor", detail: currentWorktree?.path ?? "No worktree selected.", disabled: !currentWorktree, run: () => { if (currentWorktree) void launchWorktreeTool(selectedMachineId, currentWorktree.path, "editor").catch((cause: unknown) => toast.add({ type: "error", title: "Could not open editor", description: toMessage(cause) })); } },
    { id: "terminal", label: "Open in Terminal", detail: selectedMachine?.kind === "ssh" ? "Interactive remote shells stay in your terminal SSH workflow." : currentWorktree?.path ?? "No worktree selected.", disabled: !currentWorktree || selectedMachine?.kind === "ssh", run: () => { if (currentWorktree) void launchWorktreeTool(selectedMachineId, currentWorktree.path, "terminal").catch((cause: unknown) => toast.add({ type: "error", title: "Could not open terminal", description: toMessage(cause) })); } },
    { id: "settings", label: "Settings…", detail: "Machines, tools, Git defaults, and appearance.", shortcut: "⌘,", run: () => setSettingsOpen(true) },
  ];
  const commandPalette = <CommandPalette open={commandPaletteOpen} commands={paletteCommands} onOpenChange={setCommandPaletteOpen} />;
  const worktreeContextAction = worktreeContext ? actionForWorktree(worktreeContext.worktree) : null;
  const overlays = <>
    {settingsDialog}
    {commandPalette}
    {createWorktreeOpen && currentRepository && currentWorktree ? (
      <CreateWorktreeDialog
        machineId={selectedMachineId}
        repository={currentRepository}
        sourceWorktree={currentWorktree}
        onClose={() => setCreateWorktreeOpen(false)}
        onCreated={async (result) => {
          setCurrentRepositoryPath(result.repositoryPath);
          setCurrentWorktreePath(result.worktreePath);
          await refreshWorkspace();
          toast.add({ type: "success", title: "Worktree created", description: `${result.branch} · ${result.worktreePath}` });
        }}
      />
    ) : null}
    {worktreeContext && (
      <div role="menu" aria-label={`Actions for ${worktreeContext.worktree.branch ?? "detached worktree"}`} className="fixed z-[110] min-w-52 border bg-popover p-1 text-popover-foreground shadow-lg" style={{ left: Math.min(worktreeContext.x, window.innerWidth - 225), top: Math.min(worktreeContext.y, window.innerHeight - 190) }} onPointerDown={(event) => event.stopPropagation()}>
        <button role="menuitem" className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm hover:bg-accent" onClick={() => { const item = worktreeContext.worktree; setWorktreeContext(null); void launchWorktreeTool(selectedMachineId, item.path, "editor").catch((cause: unknown) => toast.add({ type: "error", title: "Could not open editor", description: toMessage(cause) })); }}><Code2Icon className="size-4" aria-hidden="true" />Open in Editor</button>
        <button role="menuitem" className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm hover:bg-accent" onClick={() => { const item = worktreeContext.worktree; setCurrentRepositoryPath(item.repositoryPath); setCurrentWorktreePath(item.path); setWorkspaceView("changes"); setWorktreeContext(null); }}><FileDiffIcon className="size-4" aria-hidden="true" />View Changes</button>
        {isRemovable(worktreeContext.worktree) && <button role="menuitem" className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm hover:bg-accent" onClick={() => { toggleChecked(worktreeContext.worktree.id); setWorktreeContext(null); }}><Checkbox checked={checked.has(worktreeContext.worktree.id)} aria-hidden="true" />Select for Cleanup</button>}
        {worktreeContextAction ? <><div className="my-1 border-t" /><button role="menuitem" className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm text-destructive hover:bg-destructive/10" onClick={() => { reviewAction(worktreeContextAction.kind, worktreeContext.worktree); setWorktreeContext(null); }}><WrenchIcon className="size-4" aria-hidden="true" />Review Management Action…</button></> : null}
      </div>
    )}
  </>;

  const showAuditLog = async () => {
    if (!auditPath) return;
    try {
      await revealAuditLog(auditPath);
    } catch (cause) {
      toast.add({ type: "error", title: "Could not reveal the audit log", description: toMessage(cause) });
    }
  };

  if (registeredRepositories === null || machines === null) {
    return (
      <main className="flex h-full flex-col items-center justify-center gap-3 bg-background">
        <Spinner className="size-6" />
        <strong className="text-base font-medium">{registeredRepositories === null ? "Loading repositories" : "Mapping worktrees"}</strong>
        <span className="max-w-md text-center text-sm text-muted-foreground">
          {registeredRepositories === null ? "Loading repositories you added…" : "Inspecting Git registrations, local changes, and disk usage…"}
        </span>
      </main>
    );
  }

  const shell = (children: ReactNode) => (
    <RepositoryProvider value={repositoryContext}>
      <div className="flex h-full min-w-0 bg-background">
        <a href="#main-content" className="sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50 focus:bg-background focus:px-3 focus:py-2 focus:ring-2 focus:ring-ring">Skip to main content</a>
        <AppSidebar
          machines={machines}
          selectedMachine={selectedMachine ?? machines[0]}
          selectedMachineId={selectedMachineId}
          connection={selectedConnection}
          disabled={actionBusy || bulkBusy || repositoriesBusy}
          view={workspaceView}
          onMachineChange={switchMachine}
          onRetry={() => void probeMachineConnection(selectedMachineId, true)}
          onViewChange={setWorkspaceView}
          onOpenSettings={() => setSettingsOpen(true)}
        />
        <main id="main-content" className="flex min-w-0 flex-1 flex-col bg-background">{children}</main>
        {repositoryDialog}
      </div>
    </RepositoryProvider>
  );

  if (!scan && loading) {
    return shell(
      <>
        <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3">
          <Spinner className="size-6" />
          <strong className="text-base font-medium">Mapping worktrees on {selectedMachine?.name}</strong>
          <span className="max-w-md text-center text-sm text-muted-foreground">
            Inspecting Git registrations, local changes, and disk usage…
          </span>
        </div>
      </>,
    );
  }

  if (!scan && error) {
    return shell(
      <>
        <Empty className="min-h-0 flex-1">
          <EmptyHeader>
            <EmptyMedia variant="icon"><AlertTriangleIcon className="text-destructive" aria-hidden="true" /></EmptyMedia>
            <EmptyTitle>Couldn’t scan worktrees</EmptyTitle>
            <EmptyDescription>{error}</EmptyDescription>
          </EmptyHeader>
          <div className="flex gap-2">
            <Button variant="outline" onClick={() => setSettingsOpen(true)}>
              <SettingsIcon data-icon="inline-start" aria-hidden="true" />
              Settings
            </Button>
            <Button onClick={() => void refreshWorkspace()}>Try Again</Button>
          </div>
        </Empty>
        {overlays}
      </>,
    );
  }

  if (!scan) {
    // No repositories configured: show the explicit onboarding flow.
    return shell(
      <>
        <Empty className="min-h-0 flex-1">
          <EmptyHeader>
            <EmptyMedia variant="icon"><FolderOpenIcon aria-hidden="true" /></EmptyMedia>
            <EmptyTitle>No repositories yet</EmptyTitle>
            <EmptyDescription>
              {selectedMachine?.kind === "ssh"
                ? `Add an existing Git repository on ${selectedMachine.name}, or clone or create one there.`
                : "Add an existing Git repository, clone one, or create a new one."}
            </EmptyDescription>
          </EmptyHeader>
          {selectedMachine?.kind === "local" ? (
            <RepositoryDropZone ref={emptyRepositoryDropRef} active={emptyRepositoryDropActive} disabled={!emptyRepositoryDropEnabled} className="max-w-sm" />
          ) : null}
          <Button disabled={repositoriesBusy} onClick={() => setRepositoryDialogOpen(true)}>
            {repositoriesBusy ? <Spinner data-icon="inline-start" /> : <FolderPlusIcon data-icon="inline-start" aria-hidden="true" />}
            {repositoriesBusy ? "Working…" : "Add Repository…"}
          </Button>
        </Empty>
        {overlays}
      </>,
    );
  }

  const selectedProvider: RemoteProvider = scan.repositories
    .find((repo) => repo.path === selected?.repositoryPath)?.provider ?? "none";
  const repositoryFilterOptions = [
    { value: "all", label: "All repositories" },
    ...scan.repositories.map((item) => ({ value: item.path, label: item.name })),
  ];
  const repositoryFilterItems = Object.fromEntries(repositoryFilterOptions.map((item) => [item.value, item.label]));
  const ageOptions: { value: AgeFilter; label: string }[] = [
    { value: 0, label: "Any activity" },
    { value: 30, label: "Idle 30+ days" },
    { value: 90, label: "Idle 90+ days" },
    { value: 180, label: "Idle 180+ days" },
    { value: 365, label: "Idle 1+ year" },
  ];
  const ageItems = Object.fromEntries(ageOptions.map((item) => [String(item.value), item.label]));
  const bulkFollowUpLabel = bulk?.stage === "done" && bulk.kind === "remove" && bulk.followUps.length > 0
    ? `Review ${bulk.followUps.length} branch deletion${bulk.followUps.length === 1 ? "" : "s"}…`
    : null;

  const workspaceToolbar = (
    <RepositoryToolbar
      repositories={scan.repositories}
      worktrees={repositoryWorktrees}
      onRepositoryChange={(path) => {
        setCurrentRepositoryPath(path);
        setCurrentWorktreePath(null);
      }}
      onWorktreeChange={setCurrentWorktreePath}
      onAddRepository={() => setRepositoryDialogOpen(true)}
      onCreateWorktree={() => setCreateWorktreeOpen(true)}
      onRemoveRepository={removeRepositoryFromList}
    />
  );

  const noWorkingCopy = (title: string, description: string) => (
    <Empty className="min-h-0 flex-1">
      <EmptyHeader>
        <EmptyMedia variant="icon"><GitCommitIcon aria-hidden="true" /></EmptyMedia>
        <EmptyTitle>{title}</EmptyTitle>
        <EmptyDescription>{description}</EmptyDescription>
      </EmptyHeader>
    </Empty>
  );

  if (workspaceView === "changes") {
    return shell(
      <>
        {workspaceToolbar}
        {currentRepository && currentWorktree ? (
          <ErrorBoundary label="The changes view" resetKey={currentWorktree.id}>
            <ChangesWorkbench key={currentWorktree.id} />
          </ErrorBoundary>
        ) : noWorkingCopy("No working copy selected", "Add or select a repository with an available working copy.")}
        {overlays}
      </>,
    );
  }

  if (workspaceView === "history") {
    return shell(
      <>
        {workspaceToolbar}
        {currentRepository && currentWorktree ? (
          <ErrorBoundary label="The history view" resetKey={currentWorktree.id}>
            <Suspense fallback={<div className="grid min-h-0 flex-1 place-items-center"><Spinner className="size-6" /></div>}>
              <HistoryWorkbench key={currentWorktree.id} />
            </Suspense>
          </ErrorBoundary>
        ) : noWorkingCopy("No history available", "Select an available working copy.")}
        {overlays}
      </>,
    );
  }

  return shell(
    <>
      <section className="flex h-16 shrink-0 items-center border-b bg-card px-5" aria-labelledby="worktree-manager-title">
        <div className="min-w-0">
          <h2 id="worktree-manager-title" className="text-lg leading-tight font-semibold tracking-tight text-balance">Worktrees</h2>
          <p className="truncate text-xs text-muted-foreground">{selectedMachine?.name} · scanned {formatAge(scan.scannedAtMs, Date.now())}</p>
        </div>
        <div className="ml-auto flex items-center gap-1">
          <Button
            size="sm"
            disabled={!selected?.exists}
            onClick={() => {
              if (!selected) return;
              setCurrentRepositoryPath(selected.repositoryPath);
              setCurrentWorktreePath(selected.path);
              setCreateWorktreeOpen(true);
            }}
          >
            <FolderPlusIcon data-icon="inline-start" aria-hidden="true" />
            New Worktree…
          </Button>
          <TooltipButton variant="ghost" size="icon-sm" disabled={!auditPath} onClick={() => void showAuditLog()} aria-label="Show audit log" tooltip="Show audit log">
            <FileClockIcon aria-hidden="true" />
          </TooltipButton>
          <TooltipButton variant="ghost" size="icon-sm" disabled={loading} onClick={() => void refreshWorkspace()} aria-label="Refresh worktrees" tooltip="Refresh worktrees (⌘R)">
            <RefreshCwIcon className={loading ? "animate-spin" : ""} aria-hidden="true" />
          </TooltipButton>
        </div>
      </section>

      <section className="flex h-14 shrink-0 items-center gap-2 border-b bg-muted/25 px-4" aria-label="Worktree filters">
          <InputGroup className="w-72 shrink-0">
            <InputGroupAddon><SearchIcon aria-hidden="true" /></InputGroupAddon>
            <InputGroupInput
              name="worktree-search"
              autoComplete="off"
              spellCheck={false}
              value={query}
              placeholder="Branch, path, commit…"
              aria-label="Search worktrees"
              onChange={(event) => setQuery(event.currentTarget.value)}
            />
            {query && (
              <InputGroupAddon align="inline-end">
                <Tooltip>
                  <TooltipTrigger render={<InputGroupButton size="icon-xs" aria-label="Clear search" onClick={() => setQuery("")} />}>
                    <XIcon aria-hidden="true" />
                  </TooltipTrigger>
                  <TooltipContent>Clear search</TooltipContent>
                </Tooltip>
              </InputGroupAddon>
            )}
          </InputGroup>
          <Select items={repositoryFilterItems} value={repository} onValueChange={(value) => setRepository(value ?? "all")}>
            <SelectTrigger className="w-48" aria-label="Filter by repository"><DatabaseIcon aria-hidden="true" /><SelectValue /></SelectTrigger>
            <SelectContent align="start" alignItemWithTrigger={false}>
              <SelectGroup>
                {repositoryFilterOptions.map((option) => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}
              </SelectGroup>
            </SelectContent>
          </Select>
          <Select items={stateItems} value={state} onValueChange={(value) => setState((value ?? "all") as StateFilter)}>
            <SelectTrigger className="w-40" aria-label="Filter by worktree state"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectGroup>
                {stateOptions.map((option) => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}
              </SelectGroup>
            </SelectContent>
          </Select>
          <Select items={ageItems} value={String(age)} onValueChange={(value) => setAge(Number(value ?? 0) as AgeFilter)}>
            <SelectTrigger className="w-40" aria-label="Filter by last activity"><ClockIcon aria-hidden="true" /><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectGroup>
                {ageOptions.map((option) => <SelectItem key={option.value} value={String(option.value)}>{option.label}</SelectItem>)}
              </SelectGroup>
            </SelectContent>
          </Select>
          {filtersActive ? <Button variant="ghost" size="sm" onClick={clearFilters}>Reset</Button> : null}
          <span className="ml-auto text-xs tabular-nums text-muted-foreground">{filtered.length} of {scan.worktrees.length}</span>
          {(error || scan.warnings.length > 0) && (
            <Tooltip>
              <TooltipTrigger render={<Badge variant="destructive" tabIndex={0} />}>
                <AlertTriangleIcon aria-hidden="true" />
                {error ? "Refresh failed" : `${scan.warnings.length} warning${scan.warnings.length === 1 ? "" : "s"}`}
              </TooltipTrigger>
              <TooltipContent className="whitespace-pre-line">{error ?? scan.warnings.join("\n")}</TooltipContent>
            </Tooltip>
          )}
      </section>

      <div className="grid min-h-0 flex-1" style={{ gridTemplateColumns: `minmax(0, 1fr) 5px ${workspaceLayout.detailsWidth}px` }}>
        <section className="flex min-h-0 min-w-0 flex-col bg-background">
          {checkedRecords.length > 0 && (
            <div className="flex shrink-0 items-center gap-3 border-b bg-warning/10 px-4 py-2">
              <strong className="font-mono text-sm">{checkedRecords.length} selected</strong>
              <span className="text-xs text-muted-foreground">{formatBytes(checkedBytes)} allocated</span>
              <Button variant="outline" size="sm" className="ml-auto" onClick={() => setChecked(new Set())}>Clear</Button>
              <Button variant="destructive" size="sm" disabled={bulkBusy} onClick={() => void reviewBulkRemoval()}>
                {bulkBusy ? <Spinner data-icon="inline-start" /> : <Trash2Icon data-icon="inline-start" aria-hidden="true" />}
                {bulkBusy ? "Running preflight…" : "Review Removals…"}
              </Button>
            </div>
          )}

          <div className="min-h-0 flex-1 overflow-auto" role="table" aria-label="Worktrees">
            <div className={cn(inventoryGridClass, "sticky top-0 z-10 h-9 shrink-0 border-b bg-muted px-3 pl-4 text-xs font-medium tracking-wider text-muted-foreground uppercase")} role="row">
              <span role="columnheader" className="flex items-center justify-center">
                <Checkbox
                  checked={allFilteredChecked}
                  disabled={filteredRemovable.length === 0}
                  aria-label={allFilteredChecked ? "Deselect every removable worktree in view" : "Select every removable worktree in view"}
                  onCheckedChange={toggleAllFiltered}
                />
              </span>
              <span role="columnheader">Worktree</span><span role="columnheader">Last activity</span><span role="columnheader">Local state</span><span role="columnheader">Integration</span><span role="columnheader">Size</span><span role="columnheader" aria-label="Actions" />
            </div>
            {filtered.map((worktree) => (
              <WorktreeRow
                key={worktree.id}
                worktree={worktree}
                selected={effectiveSelectedId === worktree.id}
                checked={checked.has(worktree.id)}
                onSelect={(id) => {
                  setSelectedId(id);
                  setActionError(null);
                }}
                onToggleChecked={toggleChecked}
                onContextMenu={(event, item) => {
                  event.preventDefault();
                  setSelectedId(item.id);
                  setWorktreeContext({ worktree: item, x: event.clientX, y: event.clientY });
                }}
                now={now}
              />
            ))}
          </div>
          {filtered.length === 0 && (
            <Empty className="flex-1">
              <EmptyHeader>
                <EmptyMedia variant="icon"><SearchXIcon aria-hidden="true" /></EmptyMedia>
                <EmptyTitle>No worktrees match</EmptyTitle>
                <EmptyDescription>Adjust the scope or reset filters.</EmptyDescription>
              </EmptyHeader>
            </Empty>
          )}
        </section>

        <PaneResizeHandle side="right" value={workspaceLayout.detailsWidth} minimum={280} maximum={560} onChange={(value) => setWorkspaceLayout((current) => ({ ...current, detailsWidth: value }))} />

        <aside className="min-h-0 overflow-y-auto border-l bg-sidebar">
          {selected ? (
            <WorktreeDetails
              worktree={selected}
              provider={selectedProvider}
              pull={pullEvidence[selected.id] ?? null}
              now={now}
              busy={actionBusy}
              actionError={actionPlan ? null : actionError}
              onReviewAction={reviewAction}
              onCheckPulls={checkPullRequests}
              onPullChanged={async (evidence, checkedOut) => {
                setPullEvidence((current) => ({ ...current, [selected.id]: evidence }));
                if (checkedOut) await refreshWorkspace();
              }}
              onViewChanges={setDiffWorktree}
            />
          ) : (
            <Empty className="h-full">
              <EmptyHeader>
                <EmptyMedia variant="icon"><ShieldCheckIcon aria-hidden="true" /></EmptyMedia>
                <EmptyTitle>Select a worktree</EmptyTitle>
                <EmptyDescription>Review exact evidence before taking action.</EmptyDescription>
              </EmptyHeader>
            </Empty>
          )}
        </aside>
      </div>

      {actionPlan && <ActionDialog plan={actionPlan} busy={actionBusy} error={actionError} onCancel={() => { setActionPlan(null); setActionError(null); }} onConfirm={() => void executeAction()} />}
      {bulk && (
        <LazyDialog onClose={() => setBulk(null)}>
          <BulkActionDialog
            title={bulk.kind === "remove" ? `Remove ${bulk.items.length} clean worktree${bulk.items.length === 1 ? "" : "s"}?` : `Delete ${bulk.items.length} retained branch${bulk.items.length === 1 ? "" : "es"}?`}
            summary={bulk.kind === "remove"
              ? "Each worktree passed its own preflight moments ago and is revalidated again at execution. Branches are always retained."
              : "Each branch is reviewed again and deleted only if it is unchanged and still merged. Remote branches are untouched."}
            confirmationText={bulk.kind === "remove" ? "REMOVE" : "DELETE"}
            stage={bulk.stage}
            items={bulk.items}
            followUpLabel={bulkFollowUpLabel}
            onCancel={() => setBulk(null)}
            onConfirm={() => void executeBulk()}
            onFollowUp={() => void reviewBulkBranchDeletion(bulk.followUps)}
          />
        </LazyDialog>
      )}
      {branchReview ? (
        <LazyDialog onClose={() => setBranchReview(null)}>
          <DeleteBranchDialog
            machineId={branchReview.machineId}
            repositoryPath={branchReview.followUp.repositoryPath}
            worktreePath={branchReview.followUp.worktreePath}
            branches={branchReview.branches}
            initialBranchRef={`refs/heads/${branchReview.followUp.branch}`}
            onClose={() => setBranchReview(null)}
            onDeleted={followUpBranchDeleted}
          />
        </LazyDialog>
      ) : null}
      {diffWorktree ? (
        <LazyDialog onClose={() => setDiffWorktree(null)}>
          <DiffDialog machineId={selectedMachineId} worktree={diffWorktree} onClose={() => setDiffWorktree(null)} />
        </LazyDialog>
      ) : null}
      {overlays}
    </>,
  );
}

export default App;
