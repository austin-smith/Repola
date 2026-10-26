import { processPatch } from "@pierre/diffs";
import { describe, expect, it } from "vitest";
import { unifiedDiffRows, type DiffRow } from "./diff-rows";

function rows(patch: string): DiffRow[] {
  const [file] = processPatch(patch).files;
  if (!file) throw new Error("the fixture did not parse");
  return unifiedDiffRows(file);
}

const header = "diff --git a/src/a.ts b/src/a.ts\nindex 1111111..2222222 100644\n--- a/src/a.ts\n+++ b/src/a.ts\n";

describe("unifiedDiffRows", () => {
  it("numbers each side's lines and points every line at its tokens", () => {
    expect(rows(`${header}@@ -4,3 +4,3 @@ function run()\n keep\n-old\n+new\n tail\n`)).toEqual([
      { kind: "hunk", hunkIndex: 0, header: "@@ -4,3 +4,3 @@ function run()" },
      { kind: "line", hunkIndex: 0, type: "context", oldLine: 4, newLine: 4, content: "keep", side: "addition", sideIndex: 0 },
      { kind: "line", hunkIndex: 0, type: "deletion", oldLine: 5, newLine: null, content: "old", side: "deletion", sideIndex: 1 },
      { kind: "line", hunkIndex: 0, type: "addition", oldLine: null, newLine: 5, content: "new", side: "addition", sideIndex: 1 },
      { kind: "line", hunkIndex: 0, type: "context", oldLine: 6, newLine: 6, content: "tail", side: "addition", sideIndex: 2 },
    ]);
  });

  it("keeps hunks in order and continues each side's indices across them", () => {
    const result = rows(`${header}@@ -1,2 +1,2 @@\n a\n-b\n+B\n@@ -10 +10,2 @@\n-q\n+r\n+s\n`);
    expect(result.filter((row) => row.kind === "hunk").map((row) => row.header)).toEqual(["@@ -1,2 +1,2 @@", "@@ -10 +10,2 @@"]);
    expect(result.slice(5)).toEqual([
      { kind: "line", hunkIndex: 1, type: "deletion", oldLine: 10, newLine: null, content: "q", side: "deletion", sideIndex: 2 },
      { kind: "line", hunkIndex: 1, type: "addition", oldLine: null, newLine: 10, content: "r", side: "addition", sideIndex: 2 },
      { kind: "line", hunkIndex: 1, type: "addition", oldLine: null, newLine: 11, content: "s", side: "addition", sideIndex: 3 },
    ]);
  });

  it("marks the side that ends without a newline, between the deletions and additions", () => {
    const result = rows(`${header}@@ -1,2 +1,2 @@\n x\n-y\n\\ No newline at end of file\n+y\n`);
    expect(result.map((row) => (row.kind === "line" ? row.type : row.kind))).toEqual(["hunk", "context", "deletion", "no-newline", "addition"]);
    expect(result[3]).toEqual({ kind: "no-newline", hunkIndex: 0, side: "deletion" });
  });

  it("marks both sides once when the file ends in unchanged lines without a newline", () => {
    const result = rows(`${header}@@ -1,2 +1,2 @@\n-a\n+b\n c\n\\ No newline at end of file\n`);
    expect(result[result.length - 1]).toEqual({ kind: "no-newline", hunkIndex: 0, side: "both" });
  });

  it("shows line text without its line ending", () => {
    const result = rows(`${header}@@ -1,2 +1,2 @@\n x\r\n-y\r\n+z\r\n`);
    expect(result.flatMap((row) => (row.kind === "line" ? [row.content] : []))).toEqual(["x", "y", "z"]);
  });

  it("numbers a new file's lines from one", () => {
    const result = rows("diff --git a/new.ts b/new.ts\nnew file mode 100644\n--- /dev/null\n+++ b/new.ts\n@@ -0,0 +1,2 @@\n+a\n+b\n");
    expect(result.flatMap((row) => (row.kind === "line" ? [[row.oldLine, row.newLine]] : []))).toEqual([[null, 1], [null, 2]]);
  });
});
