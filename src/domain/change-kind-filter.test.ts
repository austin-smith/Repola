import { describe, expect, it } from "vitest";
import type { FileChangeKind } from "../ipc/types";
import {
  activeChangeKinds,
  changeKindFilterKind,
  countChangeKinds,
  filterByChangeKind,
  isChangeKindFilterKind,
} from "./change-kind-filter";

const file = (id: string, kind: FileChangeKind) => ({ id, kind });
const files = [
  file("a", "modified"),
  file("b", "untracked"),
  file("c", "deleted"),
  file("d", "added"),
  file("e", "modified"),
];

describe("change kind filter", () => {
  it("treats untracked files as additions", () => {
    expect(changeKindFilterKind("untracked")).toBe("added");
    expect(changeKindFilterKind("renamed")).toBe("renamed");
  });

  it("recognizes only filterable kinds", () => {
    expect(isChangeKindFilterKind("typeChanged")).toBe(true);
    expect(isChangeKindFilterKind("untracked")).toBe(false);
    expect(isChangeKindFilterKind("other")).toBe(false);
  });

  it("counts the present kinds in display order", () => {
    expect(countChangeKinds(files)).toEqual([
      { kind: "added", count: 2 },
      { kind: "modified", count: 2 },
      { kind: "deleted", count: 1 },
    ]);
    expect(countChangeKinds([])).toEqual([]);
  });

  it("keeps every file when no kind is active", () => {
    expect(filterByChangeKind(files, [])).toBe(files);
  });

  it("keeps the files of every active kind", () => {
    expect(filterByChangeKind(files, ["added"]).map((entry) => entry.id)).toEqual(["b", "d"]);
    expect(filterByChangeKind(files, ["deleted", "modified"]).map((entry) => entry.id)).toEqual(["a", "c", "e"]);
  });

  it("stops filtering by a kind whose files are gone", () => {
    const counts = countChangeKinds(files);
    expect(activeChangeKinds(["renamed", "deleted"], counts)).toEqual(["deleted"]);
    expect(activeChangeKinds(["renamed"], counts)).toEqual([]);
  });
});
