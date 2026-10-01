import { describe, expect, it } from "vitest";
import { matchesRepositoryQuery } from "./repository-picker";
import type { RepositorySummary } from "../ipc/types";

function repository(name: string, path: string): RepositorySummary {
  return {
    id: path,
    name,
    path,
    remoteUrl: null,
    provider: "none",
    worktreeCount: 1,
    attentionCount: 0,
    conflictedCount: 0,
    allocatedBytes: 0,
    allocationIncomplete: false,
  };
}

describe("matchesRepositoryQuery", () => {
  it("matches every repository for a blank query", () => {
    expect(matchesRepositoryQuery(repository("repola", "/repos/repola"), "   ")).toBe(true);
  });

  it("matches the name case-insensitively after trimming", () => {
    expect(matchesRepositoryQuery(repository("Repola", "/repos/repola"), "  POLA ")).toBe(true);
    expect(matchesRepositoryQuery(repository("repola", "/repos/repola"), "t3code")).toBe(false);
  });

  it("ignores the hidden path", () => {
    expect(matchesRepositoryQuery(repository("repola", "/Users/me/projects/repola"), "projects")).toBe(false);
  });
});
