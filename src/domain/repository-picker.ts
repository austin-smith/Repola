import type { RepositorySummary } from "../ipc/types";

/** Matches the query against the repository name, the only value the picker shows. */
export function matchesRepositoryQuery(repository: RepositorySummary, query: string): boolean {
  const normalized = query.trim().toLowerCase();
  if (!normalized) return true;
  return repository.name.toLowerCase().includes(normalized);
}
