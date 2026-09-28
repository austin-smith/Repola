import type { MachineKind } from "../ipc/types";

/**
 * The path separator on the machine that owns a path: the desktop's own for
 * local machines, and the remote OS's for SSH machines.
 */
export function machinePathSeparator(machineKind: MachineKind, machineOs: string | null, hostSeparator: string): string {
  if (machineKind === "local") return hostSeparator;
  return machineOs === "windows" ? "\\" : "/";
}

/**
 * What to call the host's file browser in labels. Detected from the webview's
 * user agent, which reports the OS the app is running on; falls back to a
 * generic name elsewhere.
 */
export function fileManagerName(userAgent: string = typeof navigator === "undefined" ? "" : navigator.userAgent): string {
  if (/Macintosh|Mac OS X/.test(userAgent)) return "Finder";
  if (/Windows/.test(userAgent)) return "File Explorer";
  return "file manager";
}
