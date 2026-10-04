import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AppPreferences } from "../ipc/types";

const mocks = vi.hoisted(() => ({
  loadAppPreferences: vi.fn(),
  updateAppPreferences: vi.fn(),
  toast: vi.fn(),
}));
vi.mock("../ipc/app-preferences", () => ({
  loadAppPreferences: mocks.loadAppPreferences,
  updateAppPreferences: mocks.updateAppPreferences,
}));
vi.mock("@/components/ui/toast", () => ({ toast: { add: mocks.toast } }));

const stored: AppPreferences = {
  version: 5,
  editorId: "zed",
  terminalId: null,
  defaultSignCommits: true,
  textGenerationSelections: {},
  diff: { hideWhitespaceInChanges: false, hideWhitespaceInHistory: true },
};

// The store is shared by every diff view for the session, so each test gets
// a fresh copy of the module.
async function loadHook() {
  vi.resetModules();
  return (await import("./diff-preferences")).useHideWhitespace;
}

describe("useHideWhitespace", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("is unknown until loaded, then reports each view's own choice from one shared load", async () => {
    mocks.loadAppPreferences.mockResolvedValue(stored);
    const useHideWhitespace = await loadHook();
    const changes = renderHook(() => useHideWhitespace("changes"));
    const history = renderHook(() => useHideWhitespace("history"));
    expect(changes.result.current[0]).toBeNull();
    await waitFor(() => expect(changes.result.current[0]).toBe(false));
    expect(history.result.current[0]).toBe(true);
    expect(mocks.loadAppPreferences).toHaveBeenCalledTimes(1);
  });

  it("applies a change everywhere at once and saves only that view's key", async () => {
    mocks.loadAppPreferences.mockResolvedValue(stored);
    mocks.updateAppPreferences.mockImplementation(async (update: (current: AppPreferences) => AppPreferences) => update(stored));
    const useHideWhitespace = await loadHook();
    const first = renderHook(() => useHideWhitespace("changes"));
    const second = renderHook(() => useHideWhitespace("changes"));
    await waitFor(() => expect(first.result.current[0]).toBe(false));

    act(() => first.result.current[1](true));
    expect(first.result.current[0]).toBe(true);
    expect(second.result.current[0]).toBe(true);
    const update = mocks.updateAppPreferences.mock.calls[0][0] as (current: AppPreferences) => AppPreferences;
    expect(update(stored)).toEqual({ ...stored, diff: { hideWhitespaceInChanges: true, hideWhitespaceInHistory: true } });
  });

  it("rolls back and reports a choice that could not be saved", async () => {
    mocks.loadAppPreferences.mockResolvedValue(stored);
    mocks.updateAppPreferences.mockRejectedValue(new Error("disk full"));
    const useHideWhitespace = await loadHook();
    const hook = renderHook(() => useHideWhitespace("history"));
    await waitFor(() => expect(hook.result.current[0]).toBe(true));

    act(() => hook.result.current[1](false));
    expect(hook.result.current[0]).toBe(false);
    await waitFor(() => expect(hook.result.current[0]).toBe(true));
    expect(mocks.toast).toHaveBeenCalledWith(expect.objectContaining({ type: "error", description: "disk full" }));
  });

  it("falls back to showing whitespace when the stored choice cannot be read", async () => {
    mocks.loadAppPreferences.mockRejectedValue(new Error("unreadable"));
    const useHideWhitespace = await loadHook();
    const hook = renderHook(() => useHideWhitespace("changes"));
    await waitFor(() => expect(hook.result.current[0]).toBe(false));
  });
});
