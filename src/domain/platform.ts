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
