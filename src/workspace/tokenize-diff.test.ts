// @vitest-environment node
// Runs the real highlighter with grammars resolved as the UI thread resolves
// them, so a change in either fails here rather than silently dropping colors.
import { describe, expect, it } from "vitest";
import type { DiffToken } from "../domain/diff-tokens";
import { resolveGrammar } from "./highlight-grammar";
import { tokenizeDiff } from "./tokenize-diff";

const text = (tokens: readonly DiffToken[]) => tokens.map((token) => token.text).join("");
const tokenize = async (patch: string, path: string) => tokenizeDiff(patch, await resolveGrammar(path));

describe("tokenizeDiff", () => {
  it("colors each side's lines for both themes and marks the words that changed", async () => {
    const tokens = await tokenize(
      "diff --git a/src/a.ts b/src/a.ts\n--- a/src/a.ts\n+++ b/src/a.ts\n@@ -1,2 +1,2 @@\n const a = 1;\n-let b = \"x\";\n+let b = \"y\";\n",
      "src/a.ts",
    );
    expect(tokens.deletions.map(text)).toEqual(["const a = 1;", "let b = \"x\";"]);
    expect(tokens.additions.map(text)).toEqual(["const a = 1;", "let b = \"y\";"]);

    const keyword = tokens.additions[1].find((token) => token.text === "let");
    expect(keyword?.light?.color).toMatch(/^#[0-9a-f]{6}$/i);
    expect(keyword?.dark?.color).toMatch(/^#[0-9a-f]{6}$/i);
    expect(keyword?.light?.color).not.toBe(keyword?.dark?.color);
    expect(tokens.additions[1].filter((token) => token.changed).map((token) => token.text)).toEqual(["y"]);
    expect(tokens.deletions[1].filter((token) => token.changed).map((token) => token.text)).toEqual(["x"]);
  });

  it("chooses the language from the real path and attaches each language as it is needed", async () => {
    const python = await tokenize("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-def a(): pass\n+def b(): pass\n", "tool.py");
    expect(python.additions[0].find((token) => token.text === "def")?.light?.color).toBeDefined();
  });

  it("keeps files in unknown languages as uncolored text", async () => {
    const plain = await tokenize("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old words\n+new words\n", "notes.unknown-extension");
    expect(text(plain.additions[0])).toBe("new words");
    // Only the theme's default text color, with no syntax colors.
    expect(new Set(plain.additions[0].map((token) => token.light?.color)).size).toBe(1);
  });

  it("refuses patches that are not exactly one file", async () => {
    await expect(tokenize("@@ -1 +1 @@\n-a\n+b\n", "a.ts")).rejects.toThrow("Expected exactly one file in the patch.");
  });
});
