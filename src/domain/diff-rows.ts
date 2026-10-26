import type { FileDiffMetadata } from "@pierre/diffs";

/** Which side of a diff a line's text, and its syntax tokens, come from. */
export type DiffSide = "deletion" | "addition";

export type DiffLineType = "context" | "deletion" | "addition";

export type DiffRow =
  | { readonly kind: "hunk"; readonly hunkIndex: number; readonly header: string }
  | {
    readonly kind: "line";
    readonly hunkIndex: number;
    readonly type: DiffLineType;
    readonly oldLine: number | null;
    readonly newLine: number | null;
    readonly content: string;
    /**
     * Where the line sits in the parsed diff's `deletionLines` or
     * `additionLines`. Syntax tokens are produced per side with the same
     * indices, so this is how a row finds its tokens.
     */
    readonly side: DiffSide;
    readonly sideIndex: number;
  }
  | { readonly kind: "no-newline"; readonly hunkIndex: number; readonly side: DiffSide | "both" };

/**
 * Flattens a parsed single-file diff into the rows of a unified view: each
 * hunk's header, then its lines in patch order, with the old and new line
 * numbers each line carries and an end-of-file marker where a side lacks a
 * trailing newline.
 */
export function unifiedDiffRows(diff: FileDiffMetadata): DiffRow[] {
  const rows: DiffRow[] = [];
  diff.hunks.forEach((hunk, hunkIndex) => {
    rows.push({ kind: "hunk", hunkIndex, header: hunkHeader(hunk.hunkSpecs, hunk.deletionStart, hunk.deletionCount, hunk.additionStart, hunk.additionCount) });
    let oldLine = hunk.deletionStart;
    let newLine = hunk.additionStart;
    hunk.hunkContent.forEach((content, contentIndex) => {
      const last = contentIndex === hunk.hunkContent.length - 1;
      if (content.type === "context") {
        for (let offset = 0; offset < content.lines; offset += 1) {
          rows.push({
            kind: "line",
            hunkIndex,
            type: "context",
            oldLine: oldLine++,
            newLine: newLine++,
            content: lineText(diff.additionLines[content.additionLineIndex + offset]),
            side: "addition",
            sideIndex: content.additionLineIndex + offset,
          });
        }
        // A trailing context block is the end of both versions at once.
        if (last && hunk.noEOFCRDeletions && hunk.noEOFCRAdditions) rows.push({ kind: "no-newline", hunkIndex, side: "both" });
        else if (last && hunk.noEOFCRDeletions) rows.push({ kind: "no-newline", hunkIndex, side: "deletion" });
        else if (last && hunk.noEOFCRAdditions) rows.push({ kind: "no-newline", hunkIndex, side: "addition" });
        return;
      }
      for (let offset = 0; offset < content.deletions; offset += 1) {
        rows.push({
          kind: "line",
          hunkIndex,
          type: "deletion",
          oldLine: oldLine++,
          newLine: null,
          content: lineText(diff.deletionLines[content.deletionLineIndex + offset]),
          side: "deletion",
          sideIndex: content.deletionLineIndex + offset,
        });
      }
      if (last && hunk.noEOFCRDeletions && content.deletions > 0) rows.push({ kind: "no-newline", hunkIndex, side: "deletion" });
      for (let offset = 0; offset < content.additions; offset += 1) {
        rows.push({
          kind: "line",
          hunkIndex,
          type: "addition",
          oldLine: null,
          newLine: newLine++,
          content: lineText(diff.additionLines[content.additionLineIndex + offset]),
          side: "addition",
          sideIndex: content.additionLineIndex + offset,
        });
      }
      if (last && hunk.noEOFCRAdditions && content.additions > 0) rows.push({ kind: "no-newline", hunkIndex, side: "addition" });
    });
  });
  return rows;
}

/** The hunk's `@@` line as it appeared in the patch, without its line ending. */
function hunkHeader(specs: string | undefined, deletionStart: number, deletionCount: number, additionStart: number, additionCount: number): string {
  return specs?.replace(/\r?\n$/, "") ?? `@@ -${deletionStart},${deletionCount} +${additionStart},${additionCount} @@`;
}

/** A parsed line keeps its line ending; rows show the text alone. */
function lineText(line: string | undefined): string {
  return line?.replace(/\r?\n$/, "") ?? "";
}
