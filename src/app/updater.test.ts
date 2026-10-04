import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  check: vi.fn(),
  isTauri: vi.fn(() => true),
  relaunch: vi.fn(async () => undefined),
}));

vi.mock("@tauri-apps/api/core", () => ({ isTauri: mocks.isTauri }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: mocks.relaunch }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: mocks.check }));

describe("updater coordinator", () => {
  afterEach(() => vi.unstubAllEnvs());
  beforeEach(() => {
    vi.stubEnv("VITE_REPOLA_RELEASE_CHANNEL", "stable");
    vi.resetModules();
    mocks.check.mockReset();
    mocks.isTauri.mockReset().mockReturnValue(true);
    mocks.relaunch.mockReset().mockResolvedValue(undefined);
  });

  it("reports an up-to-date signed channel", async () => {
    mocks.check.mockResolvedValue(null);
    const updater = await import("./updater");
    await updater.checkForUpdates();
    expect(updater.getUpdaterState()).toEqual({ status: "upToDate" });
    expect(mocks.check).toHaveBeenCalledWith({ timeout: 20_000 });
  });

  it("tracks verified download progress and relaunches after installation", async () => {
    const update = {
      version: "1.2.3",
      body: "Release notes",
      close: vi.fn(async () => undefined),
      downloadAndInstall: vi.fn(async (onEvent: (event: unknown) => void) => {
        onEvent({ event: "Started", data: { contentLength: 100 } });
        onEvent({ event: "Progress", data: { chunkLength: 40 } });
        onEvent({ event: "Progress", data: { chunkLength: 60 } });
        onEvent({ event: "Finished" });
      }),
    };
    mocks.check.mockResolvedValue(update);
    const updater = await import("./updater");
    await updater.checkForUpdates();
    expect(updater.getUpdaterState()).toEqual({ status: "available", version: "1.2.3", notes: "Release notes" });
    await updater.installAvailableUpdate();
    expect(update.downloadAndInstall).toHaveBeenCalledWith(expect.any(Function), { timeout: 600_000 });
    expect(updater.getUpdaterState()).toEqual({ status: "restarting", version: "1.2.3" });
    expect(mocks.relaunch).toHaveBeenCalledOnce();
  });

  it("closes a failed update resource so retry starts from a fresh signed check", async () => {
    const failed = {
      version: "2.0.0",
      body: null,
      close: vi.fn(async () => undefined),
      downloadAndInstall: vi.fn(async () => { throw new Error("download interrupted"); }),
    };
    mocks.check.mockResolvedValueOnce(failed).mockResolvedValueOnce(null);
    const updater = await import("./updater");
    await updater.checkForUpdates();
    await updater.installAvailableUpdate();
    expect(updater.getUpdaterState()).toEqual({ status: "error", context: "install", version: "2.0.0", message: "download interrupted" });
    expect(failed.close).toHaveBeenCalledOnce();
    await updater.installAvailableUpdate();
    expect(mocks.check).toHaveBeenCalledTimes(2);
    expect(updater.getUpdaterState()).toEqual({ status: "upToDate" });
  });

  it("keeps automatic-check failures quiet", async () => {
    mocks.check.mockRejectedValue(new Error("no release channel"));
    const updater = await import("./updater");
    await updater.checkForUpdates(false);
    expect(updater.getUpdaterState()).toEqual({ status: "idle" });
  });

  it("closes a cross-channel offer before it can become installable", async () => {
    vi.stubEnv("VITE_REPOLA_RELEASE_CHANNEL", "stable");
    const update = { version: "0.2.0-nightly.123", close: vi.fn(async () => undefined) };
    mocks.check.mockResolvedValue(update);
    const updater = await import("./updater");
    await updater.checkForUpdates();
    expect(update.close).toHaveBeenCalledOnce();
    expect(updater.getUpdaterState()).toMatchObject({ status: "error", context: "check" });
    expect(updater.getUpdaterState()).not.toHaveProperty("version", update.version);
  });

  it("never contacts an update feed from a development build", async () => {
    vi.stubEnv("VITE_REPOLA_RELEASE_CHANNEL", "development");
    const updater = await import("./updater");
    await updater.checkForUpdates(false);
    expect(updater.getUpdaterState()).toEqual({ status: "idle" });
    await updater.checkForUpdates(true);
    expect(updater.getUpdaterState()).toMatchObject({ status: "error", context: "check", message: "Development builds are updated by installing a new build." });
    expect(mocks.check).not.toHaveBeenCalled();
  });
});
