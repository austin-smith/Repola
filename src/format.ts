const byteFormatter = new Intl.NumberFormat("en", { maximumFractionDigits: 1 });
const dateFormatter = new Intl.DateTimeFormat("en", { month: "short", day: "numeric", year: "numeric" });

export function formatBytes(bytes: number | null): string {
  if (bytes === null) return "—";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  return `${byteFormatter.format(value)} ${units[unitIndex]}`;
}

export function ageInDays(timestamp: number | null, now = Date.now()): number | null {
  return timestamp === null ? null : Math.max(0, Math.floor((now - timestamp) / 86_400_000));
}

export function formatAge(timestamp: number | null, now = Date.now()): string {
  const days = ageInDays(timestamp, now);
  if (days === null) return "unknown";
  if (days === 0) return "today";
  if (days === 1) return "1 day";
  if (days < 60) return `${days} days`;
  if (days < 730) return `${Math.floor(days / 30)} mo`;
  return `${Math.floor(days / 365)} yr`;
}

export function formatDate(timestamp: number | null): string {
  return timestamp === null ? "Unavailable" : dateFormatter.format(timestamp);
}

/** What the UI needs to know about the host OS to abbreviate paths. Supplied by the backend, never guessed. */
export interface PathDisplay {
  /** The current user's home directory, or null when it could not be resolved. */
  homeDir: string | null;
  /** The platform path separator ("\\" on Windows, "/" elsewhere). */
  separator: string;
}

function trimTrailingSeparators(value: string, separator: string): string {
  let end = value.length;
  while (end > 0 && (value[end - 1] === separator || value[end - 1] === "/")) end -= 1;
  return value.slice(0, end);
}

/**
 * Replace the user's home directory with `~` for display. On Windows the
 * comparison is case-insensitive and tolerant of the forward slashes Git
 * prints, because `C:/Users/me/code` and `C:\Users\me\code` are the same folder.
 */
export function shortPath(path: string, display: PathDisplay): string {
  if (!display.homeDir) return path;
  const windows = display.separator === "\\";
  const home = trimTrailingSeparators(display.homeDir, display.separator);
  if (home.length === 0) return path;
  const comparable = (value: string) => (windows ? value.replace(/\//g, "\\").toLowerCase() : value);
  const trimmed = trimTrailingSeparators(path, display.separator);
  const candidate = comparable(trimmed);
  const prefix = comparable(home);
  if (candidate === prefix) return "~";
  if (!candidate.startsWith(prefix + display.separator)) return path;
  const remainder = trimmed.slice(home.length);
  return `~${windows ? remainder.replace(/\//g, "\\") : remainder}`;
}

export function formatMeasuredBytes(bytes: number | null, incomplete: boolean): string {
  const formatted = formatBytes(bytes);
  return incomplete && bytes !== null ? `≥ ${formatted}` : formatted;
}

export function shortSha(head: string | null): string {
  return head?.slice(0, 8) ?? "unknown";
}
