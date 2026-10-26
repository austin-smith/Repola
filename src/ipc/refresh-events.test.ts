import { describe, expect, it } from "vitest";
import { onWorkspaceMutation, runWorkspaceMutation, type WorkspaceMutationEvent } from "./refresh-events";

describe("workspace mutation lifecycle", () => {
  it("signals start synchronously and retains the original identity through settlement", async () => {
    const events: WorkspaceMutationEvent[] = [];
    const unsubscribe = onWorkspaceMutation((event) => { events.push(event); });
    const scope = { machineId: "local", repositoryPath: "repo", worktreePath: "tree" };
    let resolve!: (value: number) => void;
    try {
      const operation = runWorkspaceMutation(scope, () => {
        expect(events.map((event) => event.phase)).toEqual(["start"]);
        return new Promise<number>((done) => { resolve = done; });
      });
      scope.machineId = "remote";
      scope.worktreePath = "other";
      resolve(7);
      await expect(operation).resolves.toBe(7);
      expect(events).toEqual([
        { phase: "start", machineId: "local", repositoryPath: "repo", worktreePath: "tree" },
        { phase: "settled", machineId: "local", repositoryPath: "repo", worktreePath: "tree" },
      ]);
    } finally {
      unsubscribe();
    }
  });

  it("signals settlement on rejected and synchronously thrown mutations", async () => {
    const phases: string[] = [];
    const unsubscribe = onWorkspaceMutation((event) => { phases.push(event.phase); });
    const failure = new Error("Git failed after changing files");
    try {
      await expect(runWorkspaceMutation({ machineId: "local" }, () => Promise.reject(failure))).rejects.toBe(failure);
      await expect(runWorkspaceMutation({ machineId: "local" }, () => { throw failure; })).rejects.toBe(failure);
      expect(phases).toEqual(["start", "settled", "start", "settled"]);
    } finally {
      unsubscribe();
    }
  });

  it("does not let subscriber errors alter mutations or suppress other subscribers", async () => {
    const broken = onWorkspaceMutation(() => { throw new Error("subscriber failed"); });
    const phases: string[] = [];
    const healthy = onWorkspaceMutation((event) => { phases.push(event.phase); });
    try {
      await expect(runWorkspaceMutation({ machineId: "local" }, () => Promise.resolve("done"))).resolves.toBe("done");
      expect(phases).toEqual(["start", "settled"]);
    } finally {
      broken();
      healthy();
    }
  });
});
