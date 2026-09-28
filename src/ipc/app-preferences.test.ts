import { beforeEach, describe, expect, it, vi } from "vitest";
import { loadAppPreferences, updateAppPreferences } from "./app-preferences";
import type { AppPreferences } from "./types";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => mocks);

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

describe("preference updates", () => {
  let stored: AppPreferences;
  beforeEach(() => {
    stored = { version: 5, editorId: null, terminalId: null, defaultSignCommits: false, textGenerationSelections: {} };
    mocks.invoke.mockReset().mockImplementation(async (command, args) => {
      if (command === "save_app_preferences") stored = args.preferences;
      return structuredClone(stored);
    });
  });

  it("orders updates and reads, merging each update with the last saved preferences", async () => {
    const firstSave = deferred<void>();
    mocks.invoke.mockImplementation(async (command, args) => {
      if (command === "save_app_preferences") {
        if (!stored.textGenerationSelections.local) await firstSave.promise;
        stored = args.preferences;
      }
      return structuredClone(stored);
    });
    const ai = updateAppPreferences((current) => ({ ...current, textGenerationSelections: {
      local: { provider: "claude", selections: {} },
    } }));
    const tools = updateAppPreferences((current) => ({ ...current, defaultSignCommits: true }));
    const read = loadAppPreferences();
    await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalledTimes(2));
    firstSave.resolve();
    await Promise.all([ai, tools]);
    expect(await read).toMatchObject({ defaultSignCommits: true, textGenerationSelections: { local: { provider: "claude" } } });
    expect(mocks.invoke.mock.calls.map(([command]) => command)).toEqual([
      "load_app_preferences", "save_app_preferences", "load_app_preferences", "save_app_preferences", "load_app_preferences",
    ]);
  });

  it("continues after a failed save using the last durable state", async () => {
    const firstSave = deferred<AppPreferences>();
    mocks.invoke.mockResolvedValueOnce(structuredClone(stored)).mockImplementationOnce(() => firstSave.promise);
    const failed = updateAppPreferences((current) => ({ ...current, editorId: "cursor" }));
    const rejection = expect(failed).rejects.toThrow("Disk full");
    const next = updateAppPreferences((current) => ({ ...current, terminalId: "ghostty" }));
    firstSave.reject(new Error("Disk full"));
    await rejection;
    expect(await next).toMatchObject({ editorId: null, terminalId: "ghostty" });
    expect(await loadAppPreferences()).toMatchObject({ editorId: null, terminalId: "ghostty" });
  });
});
