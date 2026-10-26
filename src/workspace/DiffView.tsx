import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { processPatch, type FileDiffMetadata } from "@pierre/diffs";
import { useTheme } from "@/components/theme-provider";
import { unifiedDiffRows, type DiffRow } from "../domain/diff-rows";
import type { DiffToken, DiffTokens, TokenAppearance } from "../domain/diff-tokens";
import { highlightDiff } from "./diff-highlighter";

/**
 * Diffs with more lines than this on either side are shown without syntax
 * highlighting. At this size highlighting takes a few seconds on the worker
 * and handing the tokens back to the UI thread takes tens of milliseconds;
 * both grow with the diff.
 */
export const MAX_HIGHLIGHTED_LINES_PER_SIDE = 10_000;

const HUNK_ROW_HEIGHT = 32;
const LINE_ROW_HEIGHT = 20;
/** Combined width of the old-line and new-line gutter columns. */
const GUTTER_WIDTH_PX = 44 + 44;

type LineRow = Extract<DiffRow, { kind: "line" }>;
/**
 * A read-only, syntax-highlighted unified diff of one file. Rows are windowed
 * against the scrolling ancestor so only the visible part of a large diff is
 * mounted. Text shows at once; colors follow when the highlighting worker
 * finishes, and the diff stays plain text if it is too large to highlight or
 * highlighting fails.
 */
export function DiffView({ patch, path, scrollElement }: {
  patch: string;
  /** The file's real path, which decides the highlighting language. */
  path: string;
  /** The ancestor that scrolls this diff; rows are windowed against it. */
  scrollElement: HTMLElement | null;
}) {
  const file = useMemo(() => singleFile(patch), [patch]);
  // A patch that does not parse as exactly one file is shown verbatim, so
  // nothing in it is ever hidden. So is one with no hunks (a mode change, a
  // pure rename, an empty new file): its header lines are the whole change.
  if (!file || file.hunks.length === 0) return <pre className="overflow-x-auto p-3 font-mono text-xs whitespace-pre">{patch}</pre>;
  return <ParsedDiffView patch={patch} path={path} file={file} scrollElement={scrollElement} />;
}

function ParsedDiffView({ patch, path, file, scrollElement }: {
  patch: string;
  path: string;
  file: FileDiffMetadata;
  scrollElement: HTMLElement | null;
}) {
  const rows = useMemo(() => unifiedDiffRows(file), [file]);
  const tooLarge = Math.max(file.deletionLines.length, file.additionLines.length) > MAX_HIGHLIGHTED_LINES_PER_SIDE;
  const highlight = useHighlight(patch, path, !tooLarge);
  const tokens = highlight?.tokens ?? null;
  const { resolvedTheme } = useTheme();
  const note = tooLarge
    ? "Syntax highlighting is off for diffs this large."
    : highlight?.failed ? "Syntax highlighting is unavailable for this diff." : null;
  const longestLine = useMemo(() => rows.reduce((longest, row) => (row.kind === "line" ? Math.max(longest, row.content.length) : longest), 0), [rows]);

  // Distance from the top of the scroll container's content to the rows, so
  // virtual offsets can be expressed in the container's scroll coordinates.
  // Only the note above the rows can move them once mounted.
  const listRef = useRef<HTMLDivElement>(null);
  const [scrollMargin, setScrollMargin] = useState(0);
  useLayoutEffect(() => {
    const list = listRef.current;
    if (!list || !scrollElement) return;
    setScrollMargin(list.getBoundingClientRect().top - scrollElement.getBoundingClientRect().top + scrollElement.scrollTop);
  }, [scrollElement, note]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollElement,
    estimateSize: (index) => (rows[index]?.kind === "hunk" ? HUNK_ROW_HEIGHT : LINE_ROW_HEIGHT),
    scrollMargin,
    overscan: 12,
  });
  const items = virtualizer.getVirtualItems();
  const firstItem = items[0];

  return (
    <div>
      {note ? <p className="border-b px-3 py-1.5 text-xs text-muted-foreground">{note}</p> : null}
      <div ref={listRef} className="overflow-x-auto">
        <div
          className="relative w-full"
          style={{ height: virtualizer.getTotalSize(), minWidth: `calc(${GUTTER_WIDTH_PX}px + ${longestLine + 2}ch)` }}
        >
          {firstItem ? (
            <div className="absolute inset-x-0 top-0" style={{ transform: `translateY(${firstItem.start - scrollMargin}px)` }}>
              {items.map((item) => {
                const row = rows[item.index];
                if (!row) return null;
                if (row.kind === "hunk") return <HunkRow key={item.key} header={row.header} />;
                if (row.kind === "no-newline") return <NoNewlineRow key={item.key} />;
                return <LineRow key={item.key} row={row} tokens={lineTokensFor(tokens, row)} theme={resolvedTheme} />;
              })}
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
}

const HunkRow = memo(function HunkRow({ header }: { header: string }) {
  return (
    <div className="flex h-8 items-center border-t bg-muted/40 px-3 first:border-t-0">
      <code className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground">{header}</code>
    </div>
  );
});

const NoNewlineRow = memo(function NoNewlineRow() {
  return (
    <div className="grid h-5 grid-cols-[88px_1fr] font-mono text-xs leading-5 text-muted-foreground">
      <span />
      <span className="pl-4 italic">No newline at end of file</span>
    </div>
  );
});

const LineRow = memo(function LineRow({ row, tokens, theme }: { row: LineRow; tokens: readonly DiffToken[] | null; theme: "light" | "dark" }) {
  const tone = row.type === "addition" ? "bg-success/15 dark:bg-success/20" : row.type === "deletion" ? "bg-destructive/15 dark:bg-destructive/20" : "";
  const prefixTone = row.type === "addition" ? "text-success" : row.type === "deletion" ? "text-destructive" : "text-muted-foreground";
  const changedTone = row.type === "addition" ? "rounded-[2px] bg-success/30" : "rounded-[2px] bg-destructive/30";
  return (
    <div className={`grid h-5 grid-cols-[44px_44px_1fr] font-mono text-xs leading-5 ${tone}`}>
      <span className="pr-2 text-right text-muted-foreground/70 tabular-nums select-none">{row.oldLine}</span>
      <span className="pr-2 text-right text-muted-foreground/70 tabular-nums select-none">{row.newLine}</span>
      <code className="whitespace-pre pl-1">
        <span className={`inline-block w-3 select-none ${prefixTone}`} aria-hidden="true">{row.type === "addition" ? "+" : row.type === "deletion" ? "-" : " "}</span>
        {row.type === "context" ? null : <span className="sr-only">{row.type === "addition" ? "Added: " : "Removed: "}</span>}
        {tokens
          ? tokens.map((token, index) => (
            <span key={index} className={token.changed ? changedTone : undefined} style={tokenStyle(token[theme])}>{token.text}</span>
          ))
          : row.content}
      </code>
    </div>
  );
});

function tokenStyle(appearance: TokenAppearance | undefined): CSSProperties | undefined {
  if (!appearance) return undefined;
  return {
    color: appearance.color,
    fontStyle: appearance.italic ? "italic" : undefined,
    fontWeight: appearance.bold ? "bold" : undefined,
    textDecoration: appearance.underline ? "underline" : undefined,
  };
}

function lineTokensFor(tokens: DiffTokens | null, row: LineRow): readonly DiffToken[] | null {
  if (!tokens) return null;
  return (row.side === "deletion" ? tokens.deletions : tokens.additions)[row.sideIndex] ?? null;
}

function singleFile(patch: string): FileDiffMetadata | null {
  const files = processPatch(patch).files;
  return files.length === 1 ? files[0] ?? null : null;
}

/**
 * Highlights `patch` on the worker while `enabled`. Null while highlighting
 * is pending or skipped; a result records failure (including a timeout) so
 * the view can say why the diff is plain.
 */
function useHighlight(patch: string, path: string, enabled: boolean): { tokens: DiffTokens | null; failed: boolean } | null {
  const [result, setResult] = useState<{ patch: string; path: string; tokens: DiffTokens | null; failed: boolean } | null>(null);
  useEffect(() => {
    if (!enabled) return;
    const controller = new AbortController();
    highlightDiff(patch, path, controller.signal).then(
      (tokens) => { if (!controller.signal.aborted) setResult({ patch, path, tokens, failed: false }); },
      () => { if (!controller.signal.aborted) setResult({ patch, path, tokens: null, failed: true }); },
    );
    return () => controller.abort();
  }, [enabled, patch, path]);
  return enabled && result?.patch === patch && result.path === path ? result : null;
}
