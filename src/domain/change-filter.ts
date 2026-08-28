import type { FileChange } from "../ipc/types";

/**
 * Narrows the changed-file list to entries whose path contains every
 * whitespace-separated term of the query, matched case-insensitively.
 * An empty query returns the list unchanged.
 */
export function filterChanges<T extends Pick<FileChange, "path">>(changes: readonly T[], query: string): T[] {
  const terms = query.toLowerCase().split(/\s+/).filter((term) => term !== "");
  if (terms.length === 0) return [...changes];
  return changes.filter((change) => {
    const haystack = change.path.display.toLowerCase();
    return terms.every((term) => haystack.includes(term));
  });
}
