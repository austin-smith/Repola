import { describe, expect, it } from "vitest";
import { classifyGitError } from "./git-errors";

describe("classifyGitError", () => {
  it.each([
    ["fatal: detected dubious ownership; configure safe.directory", "safeDirectory"],
    ["git-lfs filter-process: command not found", "lfs"],
    ["pre-commit hook declined", "hook"],
    ["Permission denied (publickey).", "authentication"],
    ["fatal: unable to access: Could not resolve host", "transport"],
    ["fatal: another git process seems to be running", "generic"],
  ] as const)("classifies %s", (message, category) => {
    expect(classifyGitError(message).category).toBe(category);
  });
});
