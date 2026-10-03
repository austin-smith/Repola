import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PhysicalPosition } from "@tauri-apps/api/dpi";
import type { EventCallback } from "@tauri-apps/api/event";
import type { DragDropEvent } from "@tauri-apps/api/webview";
import { TooltipProvider } from "../components/ui/tooltip";
import { computeTotals } from "../domain/inventory";
import type { MachineProfile, ScanResult, WorkspaceContext } from "../ipc/types";
import type * as WorktreeIpc from "../ipc/worktrees";
import type * as MachineIpc from "../ipc/machines";
import App from "./App";

const mocks = vi.hoisted(() => ({
  subscribe: vi.fn(), loadRepositories: vi.fn(), scan: vi.fn(), resolve: vi.fn(), register: vi.fn(),
  loadMachines: vi.fn(), loadContext: vi.fn(), toast: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: () => ({ onDragDropEvent: mocks.subscribe }) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => undefined }));
vi.mock("../components/ui/toast", () => ({ toast: { add: mocks.toast } }));
vi.mock("./window-state", () => ({ restoreAndTrackWindow: async () => () => undefined }));
vi.mock("./environment", () => ({ useShortPath: () => (path: string) => path, usePathSeparator: () => "/" }));
vi.mock("../ipc/worktrees", async (importOriginal) => ({
  ...await importOriginal<typeof WorktreeIpc>(),
  loadRegisteredRepositories: mocks.loadRepositories, scanWorktrees: mocks.scan,
  resolveDroppedRepository: mocks.resolve, registerRepository: mocks.register,
}));
vi.mock("../ipc/machines", async (importOriginal) => ({
  ...await importOriginal<typeof MachineIpc>(),
  loadMachines: mocks.loadMachines, testMachine: async () => ({ version: "test" }),
}));
vi.mock("../ipc/preferences", () => ({ loadWorkspaceContext: mocks.loadContext, saveWorkspaceContext: async () => undefined }));
vi.mock("../dialogs/SettingsDialog", () => ({ SettingsDialog: ({ onClose }: { onClose: () => void }) => (
  <div role="dialog" aria-label="Settings"><button onClick={onClose}>Close Settings</button></div>
) }));

const local: MachineProfile = { id: "local", name: "This computer", kind: "local", enabled: true, ssh: null };
const remote: MachineProfile = { ...local, id: "remote", name: "Remote", kind: "ssh" };
const context: WorkspaceContext = {
  selectedMachineId: "local", locations: {}, filters: {}, layout: { inventorySidebarWidth: 230, detailsWidth: 360 },
};
const scan: ScanResult = {
  scannedAtMs: 1, repositoryPaths: ["repository"], repositories: [], worktrees: [], totals: computeTotals([], 0), warnings: [],
};
let listeners: Set<EventCallback<DragDropEvent>>;
function dropOn(target: HTMLElement, paths = ["dropped folder"]) {
  vi.spyOn(target, "getBoundingClientRect").mockReturnValue({
    left: 10, right: 110, top: 10, bottom: 110, x: 10, y: 10, width: 100, height: 100, toJSON: () => ({}),
  });
  Object.defineProperty(document, "elementFromPoint", { configurable: true, value: () => target });
  const payload: DragDropEvent = { type: "drop", position: new PhysicalPosition(50, 50), paths };
  for (const listener of listeners) listener({ event: "tauri://drag-drop", id: 1, payload });
}
function renderApp() {
  return render(<TooltipProvider><App /></TooltipProvider>);
}
async function findReadyDropTarget() {
  return waitFor(() => {
    const target = screen.getByRole("region", { name: "Drop repositories" });
    expect(target).toHaveAttribute("aria-disabled", "false");
    expect(listeners.size).toBe(1);
    return target;
  });
}

describe("repository drop entry points", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    listeners = new Set();
    mocks.subscribe.mockImplementation(async (callback: EventCallback<DragDropEvent>) => {
      const subscribedListeners = listeners;
      await new Promise<void>((resolve) => setTimeout(resolve, 0));
      subscribedListeners.add(callback);
      return () => subscribedListeners.delete(callback);
    });
    mocks.loadMachines.mockResolvedValue([local, remote]);
    mocks.loadContext.mockResolvedValue(context);
    mocks.loadRepositories.mockResolvedValue([]);
    mocks.scan.mockResolvedValue(scan);
    mocks.resolve.mockResolvedValue("repository");
    mocks.register.mockResolvedValue({ repositoryPath: "repository", registeredRepositories: ["repository"] });
    vi.stubGlobal("devicePixelRatio", 1);
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("has no drop target or native listener in an existing workspace", async () => {
    mocks.loadRepositories.mockResolvedValue(["repository"]);
    renderApp();
    await screen.findByText("No working copy selected");
    expect(screen.queryByRole("region", { name: "Drop repositories" })).not.toBeInTheDocument();
    expect(mocks.subscribe).not.toHaveBeenCalled();
    fireEvent.keyDown(window, { key: "2", ctrlKey: true });
    expect(screen.getByText("No history available")).toBeInTheDocument();
    expect(mocks.subscribe).not.toHaveBeenCalled();
    fireEvent.keyDown(window, { key: "3", ctrlKey: true });
    expect(screen.getByRole("heading", { name: "Worktrees" })).toBeInTheDocument();
    expect(mocks.subscribe).not.toHaveBeenCalled();
  });

  it("adds a repository from the empty-screen target and then removes that target", async () => {
    renderApp();
    const target = await findReadyDropTarget();
    await act(async () => dropOn(target));
    expect(mocks.resolve).toHaveBeenCalledExactlyOnceWith("dropped folder");
    expect(mocks.register).toHaveBeenCalledExactlyOnceWith("local", "repository");
    await screen.findByText("No working copy selected");
    expect(listeners.size).toBe(0);
  });

  it("disables the background target while the repository dialog is open and only accepts Add Existing drops", async () => {
    renderApp();
    const background = await findReadyDropTarget();
    fireEvent.click(screen.getByRole("button", { name: "Add Repository…" }));
    await screen.findByRole("heading", { name: "Add Repository" });
    await waitFor(() => expect(listeners.size).toBe(1));
    expect(background).toHaveAttribute("aria-disabled", "true");
    act(() => dropOn(background));
    expect(mocks.resolve).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Clone" }));
    await waitFor(() => expect(listeners.size).toBe(0));
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(listeners.size).toBe(0);
    fireEvent.click(screen.getByRole("button", { name: "Add Existing" }));
    const target = await findReadyDropTarget();
    expect(target).not.toBe(background);
    await act(async () => dropOn(target));
    expect(mocks.register).toHaveBeenCalledExactlyOnceWith("local", "repository");
    expect(screen.queryByRole("heading", { name: "Add Repository" })).not.toBeInTheDocument();
  });

  it("disables onboarding drops while Settings is open", async () => {
    renderApp();
    const target = await findReadyDropTarget();
    fireEvent.keyDown(window, { key: ",", ctrlKey: true });
    await waitFor(() => expect(listeners.size).toBe(0));
    expect(target).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(screen.getByRole("button", { name: "Close Settings" }));
    await waitFor(() => expect(listeners.size).toBe(1));
  });

  it("locks the target and machine selection while a dropped repository is being resolved", async () => {
    let complete!: (repository: string) => void;
    mocks.resolve.mockImplementationOnce(() => new Promise<string>((resolve) => { complete = resolve; }));
    renderApp();
    const target = await findReadyDropTarget();
    act(() => dropOn(target));
    expect(target).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("combobox", { name: "Current machine" })).toBeDisabled();
    act(() => dropOn(target));
    expect(mocks.resolve).toHaveBeenCalledTimes(1);
    await act(async () => complete("repository"));
    expect(mocks.register).toHaveBeenCalledOnce();
  });

  it("deduplicates repositories and reports invalid items in a mixed drop", async () => {
    mocks.resolve.mockImplementation(async (path: string) => {
      if (path === "invalid item") throw new Error("Not a Git working copy");
      return "repository";
    });
    renderApp();
    const target = await findReadyDropTarget();
    await act(async () => dropOn(target, ["folder", "file in that folder", "invalid item"]));
    expect(mocks.register).toHaveBeenCalledExactlyOnceWith("local", "repository");
    expect(mocks.toast).toHaveBeenCalledWith(expect.objectContaining({ title: "Repository added" }));
    expect(mocks.toast).toHaveBeenCalledWith(expect.objectContaining({ type: "warning", description: "Not a Git working copy" }));
  });

  it.each(["footer", "escape", "close button"])("dismisses a pending drop with %s without closing a reopened dialog", async (control) => {
    let complete!: (repository: string) => void;
    mocks.resolve.mockImplementationOnce(() => new Promise<string>((resolve) => { complete = resolve; }));
    renderApp();
    await screen.findByText("No repositories yet");
    fireEvent.click(screen.getByRole("button", { name: "Add Repository…" }));
    const target = await findReadyDropTarget();
    act(() => dropOn(target));
    expect(screen.queryByRole("button", { name: "Cancel operation" })).not.toBeInTheDocument();
    expect(screen.getByText("Repository addition continues in the workspace after you close this dialog.")).toHaveAttribute("role", "status");
    if (control === "escape") fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    else fireEvent.click(screen.getByRole("button", { name: control === "footer" ? "Close dialog" : "Close" }));
    expect(screen.queryByRole("heading", { name: "Add Repository" })).not.toBeInTheDocument();

    // The menu can reopen onboarding while App finishes the accepted drop.
    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    fireEvent.click(screen.getByRole("option", { name: /Add Repository/ }));
    await screen.findByRole("heading", { name: "Add Repository" });
    await act(async () => complete("repository"));
    expect(mocks.register).toHaveBeenCalledExactlyOnceWith("local", "repository");
    expect(screen.getByRole("heading", { name: "Add Repository" })).toBeInTheDocument();
  });

  it("never enables local file drops for a remote machine", async () => {
    mocks.loadContext.mockResolvedValue({ ...context, selectedMachineId: "remote" });
    renderApp();
    await screen.findByText("No repositories yet");
    expect(mocks.subscribe).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Add Repository…" }));
    await screen.findByRole("heading", { name: "Add Repository on Remote" });
    expect(screen.getByLabelText("Repository path")).toBeInTheDocument();
    expect(mocks.subscribe).not.toHaveBeenCalled();
  });

  it("keeps the dialog open and permits retry when a dropped item is not a repository", async () => {
    mocks.resolve.mockRejectedValueOnce(new Error("Not a Git working copy"));
    renderApp();
    await screen.findByText("No repositories yet");
    fireEvent.click(screen.getByRole("button", { name: "Add Repository…" }));
    const target = await findReadyDropTarget();
    await act(async () => dropOn(target));
    expect(mocks.register).not.toHaveBeenCalled();
    expect(mocks.toast).toHaveBeenCalledWith(expect.objectContaining({ title: "No Git repository was added" }));
    expect(screen.getByRole("heading", { name: "Add Repository" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Choose Repository…" })).toBeEnabled();
    const retryTarget = await findReadyDropTarget();
    await act(async () => dropOn(retryTarget));
    expect(mocks.resolve).toHaveBeenCalledTimes(2);
    expect(mocks.register).toHaveBeenCalledExactlyOnceWith("local", "repository");
    await screen.findByText("No working copy selected");
  });
});
