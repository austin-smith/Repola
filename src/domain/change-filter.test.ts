import { describe, expect, it } from "vitest";
import { filterChanges } from "./change-filter";

const changes = [
  { path: { display: "src/components/PileOfCrap.astro", token: "a" } },
  { path: { display: "src/lib/pile-of-crap.ts", token: "b" } },
  { path: { display: "CLAUDE.md", token: "c" } },
];

describe("filterChanges", () => {
  it("returns every change for an empty or whitespace query", () => {
    expect(filterChanges(changes, "")).toEqual(changes);
    expect(filterChanges(changes, "   ")).toEqual(changes);
  });

  it("matches case-insensitively anywhere in the path", () => {
    expect(filterChanges(changes, "PILE").map((change) => change.path.token)).toEqual(["a", "b"]);
    expect(filterChanges(changes, "claude").map((change) => change.path.token)).toEqual(["c"]);
  });

  it("requires every term to match", () => {
    expect(filterChanges(changes, "pile lib").map((change) => change.path.token)).toEqual(["b"]);
    expect(filterChanges(changes, "pile nowhere")).toEqual([]);
  });
});
