import { isTauri } from "@tauri-apps/api/core";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { useSyncExternalStore } from "react";

export type UpdaterState =
  | { status: "idle" }
  | { status: "checking" }
  | { status: "upToDate" }
  | { status: "available"; version: string; notes: string | null }
  | { status: "installing"; version: string; downloadedBytes: number; totalBytes: number | null }
  | { status: "restarting"; version: string }
  | { status: "error"; message: string; context: "check" | "install"; version: string | null };

let state: UpdaterState = { status: "idle" };
let activeUpdate: Update | null = null;
let checkPromise: Promise<void> | null = null;
const listeners = new Set<() => void>();

function publish(next: UpdaterState): void {
  state = next;
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function getUpdaterState(): UpdaterState {
  return state;
}

export function useUpdaterState(): UpdaterState {
  return useSyncExternalStore(subscribe, () => state, () => state);
}

export function checkForUpdates(reportErrors = true): Promise<void> {
  if (checkPromise) return checkPromise;
  checkPromise = (async () => {
    if (!isTauri()) {
      if (reportErrors) publish({ status: "error", context: "check", version: null, message: "Update checks are available in the installed Repola desktop app." });
      return;
    }
    publish({ status: "checking" });
    try {
      if (activeUpdate) {
        await activeUpdate.close();
        activeUpdate = null;
      }
      const update = await check({ timeout: 20_000 });
      activeUpdate = update;
      if (update) {
        publish({ status: "available", version: update.version, notes: update.body ?? null });
      } else {
        publish({ status: "upToDate" });
      }
    } catch (cause) {
      if (reportErrors) {
        publish({
          status: "error",
          context: "check",
          version: null,
          message: cause instanceof Error ? cause.message : String(cause),
        });
      } else {
        publish({ status: "idle" });
      }
    }
  })().finally(() => {
    checkPromise = null;
  });
  return checkPromise;
}

export async function installAvailableUpdate(): Promise<void> {
  const update = activeUpdate;
  if (!update) {
    await checkForUpdates(true);
    if (!activeUpdate) return;
    return installAvailableUpdate();
  }
  let downloadedBytes = 0;
  let totalBytes: number | null = null;
  publish({ status: "installing", version: update.version, downloadedBytes, totalBytes });
  try {
    await update.downloadAndInstall((event) => {
      if (event.event === "Started") {
        totalBytes = event.data.contentLength ?? null;
      } else if (event.event === "Progress") {
        downloadedBytes += event.data.chunkLength;
      }
      publish({ status: "installing", version: update.version, downloadedBytes, totalBytes });
    }, { timeout: 10 * 60_000 });
    publish({ status: "restarting", version: update.version });
    await relaunch();
  } catch (cause) {
    if (activeUpdate === update) activeUpdate = null;
    await update.close().catch(() => undefined);
    publish({
      status: "error",
      context: "install",
      version: update.version,
      message: cause instanceof Error ? cause.message : String(cause),
    });
  }
}
