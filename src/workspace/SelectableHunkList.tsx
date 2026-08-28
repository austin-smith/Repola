import { memo, useCallback, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { useVirtualizer, type VirtualItem } from "@tanstack/react-virtual";
import { Checkbox } from "@/components/ui/checkbox";
import type { PatchHunk } from "../ipc/types";
import type { FileCommitSelection } from "../domain/commit-selection";
import { selectableLineIndices, selectedLinesForHunk } from "../domain/commit-selection";

export interface SelectableLine {
  index: number;
  oldLine: number | null;
  newLine: number | null;
  prefix: string;
  content: string;
  selectable: boolean;
}

interface HunkRows {
  hunk: PatchHunk;
  selected: readonly number[];
  selectable: readonly number[];
  lines: readonly SelectableLine[];
}

type Row =
  | { kind: "header"; hunk: HunkRows }
  | { kind: "line"; hunk: HunkRows; line: SelectableLine; checked: boolean };

const HEADER_ROW_ESTIMATE = 41;
const LINE_ROW_ESTIMATE = 25;
/** Combined width of the checkbox, old-line, and new-line gutter columns. */
const GUTTER_WIDTH_PX = 36 + 44 + 44;

interface SelectableHunkListProps {
  hunks: readonly PatchHunk[];
  selection: FileCommitSelection;
  /** The ancestor that scrolls the diff pane. Rows are windowed against it. */
  scrollElement: HTMLElement | null;
  onHunkSelectionChange: (hunk: PatchHunk, lineIndices: readonly number[]) => void;
}

/**
 * Renders the hunks of a text diff as one windowed list so that large files
 * only mount the rows currently visible in the scroll container. Each hunk's
 * visible rows are wrapped in their own labelled group.
 */
export function SelectableHunkList({ hunks, selection, scrollElement, onHunkSelectionChange }: SelectableHunkListProps) {
  const rows = useMemo(() => buildRows(hunks, selection), [hunks, selection]);
  const longestLine = useMemo(
    () => rows.reduce((longest, row) => (row.kind === "line" ? Math.max(longest, row.line.content.length) : longest), 0),
    [rows],
  );

  // Distance from the top of the scroll container's content to this list, so
  // virtual offsets can be expressed in the container's scroll coordinates.
  // The list is the first child of the pane, so this only depends on the
  // pane's padding and is measured once per mount.
  const listRef = useRef<HTMLDivElement>(null);
  const [scrollMargin, setScrollMargin] = useState(0);
  useLayoutEffect(() => {
    const list = listRef.current;
    if (!list || !scrollElement) return;
    setScrollMargin(list.getBoundingClientRect().top - scrollElement.getBoundingClientRect().top + scrollElement.scrollTop);
  }, [scrollElement]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollElement,
    estimateSize: (index) => (rows[index]?.kind === "header" ? HEADER_ROW_ESTIMATE : LINE_ROW_ESTIMATE),
    getItemKey: (index) => {
      const row = rows[index];
      return row?.kind === "line" ? `${row.hunk.hunk.index}:${row.line.index}` : `hunk:${row?.hunk.hunk.index ?? index}`;
    },
    scrollMargin,
    overscan: 12,
  });

  // Row components are memoized, so give them callbacks whose identity is
  // stable across selection changes; the latest data is read through refs.
  const latestChange = useRef(onHunkSelectionChange);
  latestChange.current = onHunkSelectionChange;
  const latestRows = useRef(rows);
  latestRows.current = rows;

  const findHunk = (hunkIndex: number) => latestRows.current.find((row) => row.hunk.hunk.index === hunkIndex)?.hunk;

  const toggleLine = useCallback((hunkIndex: number, lineIndex: number) => {
    const target = findHunk(hunkIndex);
    if (!target) return;
    const next = new Set(target.selected);
    if (next.has(lineIndex)) next.delete(lineIndex);
    else next.add(lineIndex);
    latestChange.current(target.hunk, [...next].sort((left, right) => left - right));
  }, []);

  const toggleHunk = useCallback((hunkIndex: number, checked: boolean) => {
    const target = findHunk(hunkIndex);
    if (!target) return;
    latestChange.current(target.hunk, checked ? target.selectable : []);
  }, []);

  const groups = groupByHunk(virtualizer.getVirtualItems(), rows);

  return (
    <div ref={listRef} className="overflow-x-auto rounded-md border bg-card">
      <div
        className="relative w-full"
        style={{ height: virtualizer.getTotalSize(), minWidth: `calc(${GUTTER_WIDTH_PX}px + ${longestLine + 2}ch)` }}
      >
        {groups.map(({ hunk, items }) => (
          <div
            key={hunk.hunk.index}
            role="group"
            aria-label="Select changed lines"
            className="absolute inset-x-0 top-0"
            style={{ transform: `translateY(${items[0].start - scrollMargin}px)` }}
          >
            {items.map((item) => {
              const row = rows[item.index];
              if (!row) return null;
              return row.kind === "header"
                ? <HunkHeaderRow key={item.key} index={item.index} measure={virtualizer.measureElement} hunk={row.hunk.hunk} selectedCount={row.hunk.selected.length} selectableCount={row.hunk.selectable.length} onToggle={toggleHunk} />
                : <HunkLineRow key={item.key} index={item.index} measure={virtualizer.measureElement} hunkIndex={row.hunk.hunk.index} line={row.line} checked={row.checked} onToggle={toggleLine} />;
            })}
          </div>
        ))}
      </div>
    </div>
  );
}

interface RowChromeProps {
  index: number;
  measure: (element: HTMLElement | null) => void;
  style?: CSSProperties;
}

const HunkHeaderRow = memo(function HunkHeaderRow({ index, measure, hunk, selectedCount, selectableCount, onToggle }: RowChromeProps & {
  hunk: PatchHunk;
  selectedCount: number;
  selectableCount: number;
  onToggle: (hunkIndex: number, checked: boolean) => void;
}) {
  const allSelected = selectedCount === selectableCount;
  return (
    <div ref={measure} data-index={index} className="flex items-center gap-3 border-y bg-muted/50 px-3 py-2 first:border-t-0">
      <Checkbox
        checked={allSelected}
        indeterminate={selectedCount > 0 && !allSelected}
        onCheckedChange={(checked) => onToggle(hunk.index, checked === true)}
        aria-label={`${allSelected ? "Exclude" : "Include"} hunk from commit`}
      />
      <code className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground">{hunk.header}</code>
    </div>
  );
});

const HunkLineRow = memo(function HunkLineRow({ index, measure, hunkIndex, line, checked, onToggle }: RowChromeProps & {
  hunkIndex: number;
  line: SelectableLine;
  checked: boolean;
  onToggle: (hunkIndex: number, lineIndex: number) => void;
}) {
  const tone = line.prefix === "+" ? "bg-success/10" : line.prefix === "-" ? "bg-destructive/10" : "";
  return (
    <label
      ref={measure}
      data-index={index}
      className={`grid grid-cols-[36px_44px_44px_1fr] border-b font-mono text-xs ${tone} ${line.selectable ? "cursor-pointer hover:bg-accent" : ""}`}
    >
      <span className="grid place-items-center border-r bg-muted/40">
        {line.selectable ? <Checkbox checked={checked} onCheckedChange={() => onToggle(hunkIndex, line.index)} aria-label={`Select ${line.prefix === "+" ? "added" : "deleted"} line ${line.newLine ?? line.oldLine}`} /> : null}
      </span>
      <span className="border-r px-2 py-1 text-right text-muted-foreground select-none">{line.oldLine}</span>
      <span className="border-r px-2 py-1 text-right text-muted-foreground select-none">{line.newLine}</span>
      <code className="px-2 py-1 whitespace-pre"><span className={line.prefix === "+" ? "text-success" : line.prefix === "-" ? "text-destructive" : "text-muted-foreground"}>{line.prefix}</span>{line.content}</code>
    </label>
  );
});

/** Splits consecutive virtual items into runs that belong to the same hunk. */
function groupByHunk(items: readonly VirtualItem[], rows: readonly Row[]): { hunk: HunkRows; items: VirtualItem[] }[] {
  const groups: { hunk: HunkRows; items: VirtualItem[] }[] = [];
  for (const item of items) {
    const row = rows[item.index];
    if (!row) continue;
    const last = groups[groups.length - 1];
    if (last && last.hunk === row.hunk) last.items.push(item);
    else groups.push({ hunk: row.hunk, items: [item] });
  }
  return groups;
}

function buildRows(hunks: readonly PatchHunk[], selection: FileCommitSelection): Row[] {
  return hunks.flatMap((hunk) => {
    const selected = selectedLinesForHunk(selection, hunk);
    const selectedSet = new Set(selected);
    const group: HunkRows = { hunk, selected, selectable: selectableLineIndices(hunk.patch), lines: parseSelectableLines(hunk.patch) };
    return [
      { kind: "header", hunk: group } satisfies Row,
      ...group.lines.map((line) => ({ kind: "line", hunk: group, line, checked: selectedSet.has(line.index) }) satisfies Row),
    ];
  });
}

function parseSelectableLines(patch: string): SelectableLine[] {
  const hunkOffset = patch.indexOf("@@ ");
  if (hunkOffset < 0) return [];
  const hunk = patch.slice(hunkOffset);
  const headerEnd = hunk.indexOf("\n");
  const match = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(hunk.slice(0, headerEnd));
  if (!match) return [];
  let oldLine = Number(match[1]);
  let newLine = Number(match[2]);
  return hunk.slice(headerEnd + 1).split("\n").filter((line, index, values) => index < values.length - 1 || line !== "").map((line, index) => {
    const prefix = line.slice(0, 1);
    const result: SelectableLine = {
      index,
      oldLine: prefix === "+" ? null : oldLine,
      newLine: prefix === "-" ? null : newLine,
      prefix,
      content: line.slice(1),
      selectable: prefix === "+" || prefix === "-",
    };
    if (prefix !== "+") oldLine += 1;
    if (prefix !== "-") newLine += 1;
    return result;
  });
}
