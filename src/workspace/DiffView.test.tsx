import { useState } from "react";
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ThemeProvider } from "@/components/theme-provider";
import type { DiffTokens } from "../domain/diff-tokens";
import { DiffView, MAX_HIGHLIGHTED_LINES_PER_SIDE } from "./DiffView";

const highlighter = vi.hoisted(() => ({ highlightDiff: vi.fn() }));
vi.mock("./diff-highlighter", () => highlighter);

const patch = "diff --git a/src/a.ts b/src/a.ts\n--- a/src/a.ts\n+++ b/src/a.ts\n@@ -1,2 +1,2 @@ function run()\n const a = 1;\n-let b = 2;\n+let b = 3;\n";
const tokens: DiffTokens = {
  deletions: [
    [{ text: "const a = 1;" }],
    [{ text: "let", light: { color: "#d73a49" }, dark: { color: "#f97583" } }, { text: " b = " }, { text: "2", changed: true }, { text: ";" }],
  ],
  additions: [
    [{ text: "const", light: { color: "#d73a49" }, dark: { color: "#f97583" } }, { text: " a = 1;" }],
    [{ text: "let", light: { color: "#d73a49", italic: true }, dark: { color: "#f97583" } }, { text: " b = " }, { text: "3", changed: true }, { text: ";" }],
  ],
};

function largePatch(lines: number) {
  const body = Array.from({ length: lines }, (_, index) => `+line ${index + 1}`).join("\n");
  return `diff --git a/big.ts b/big.ts\nnew file mode 100644\n--- /dev/null\n+++ b/big.ts\n@@ -0,0 +1,${lines} @@\n${body}\n`;
}

// jsdom performs no layout, so give the scroll container the viewport size
// the virtualizer reads. 400px holds twenty 20px lines.
function ScrollHost({ height, children }: { height: number; children: (scrollElement: HTMLDivElement | null) => React.ReactNode }) {
  const [scrollElement, setScrollElement] = useState<HTMLDivElement | null>(null);
  return (
    <div
      data-testid="scroller"
      ref={(element) => {
        if (element) Object.defineProperties(element, { clientWidth: { value: 800 }, clientHeight: { value: height } });
        setScrollElement(element);
      }}
    >
      {children(scrollElement)}
    </div>
  );
}

function renderView(view: { patch: string; path?: string; height?: number }) {
  const tree = (next: string) => (
    <ThemeProvider storageKey="test-theme">
      <ScrollHost height={view.height ?? 100_000}>
        {(scrollElement) => <DiffView patch={next} path={view.path ?? "src/a.ts"} scrollElement={scrollElement} />}
      </ScrollHost>
    </ThemeProvider>
  );
  const result = render(tree(view.patch));
  return { ...result, rerenderWith: (next: string) => result.rerender(tree(next)) };
}

function pendingHighlight() {
  let resolve: (value: DiffTokens) => void = () => undefined;
  let reject: (reason: unknown) => void = () => undefined;
  highlighter.highlightDiff.mockReturnValueOnce(new Promise<DiffTokens>((onResolve, onReject) => { resolve = onResolve; reject = onReject; }));
  return { resolve: (value: DiffTokens) => act(async () => resolve(value)), reject: (reason: unknown) => act(async () => reject(reason)) };
}

describe("DiffView", () => {
  beforeEach(() => {
    localStorage.clear();
    highlighter.highlightDiff.mockReset();
  });
  afterEach(cleanup);

  it("shows the text at once and colors it when highlighting finishes", async () => {
    const pending = pendingHighlight();
    const { container } = renderView({ patch });
    expect(screen.getByText("@@ -1,2 +1,2 @@ function run()")).toBeInTheDocument();
    expect(screen.getByText("let b = 3;")).toBeInTheDocument();
    expect(highlighter.highlightDiff).toHaveBeenCalledWith(patch, "src/a.ts", expect.any(AbortSignal));

    await pending.resolve(tokens);
    const code = (text: string) => [...container.querySelectorAll<HTMLElement>("code span")].filter((span) => span.textContent === text);
    expect(code("let").map((span) => span.style.color)).toEqual(["rgb(215, 58, 73)", "rgb(215, 58, 73)"]);
    expect(code("let")[1].style.fontStyle).toBe("italic");
    expect(code("3")).toEqual([expect.objectContaining({ className: "rounded-[2px] bg-success/30" })]);
    expect(code("2")).toEqual([expect.objectContaining({ className: "rounded-[2px] bg-destructive/30" })]);
    expect(screen.queryByText(/Syntax highlighting/)).not.toBeInTheDocument();
  });

  it("uses each token's dark-theme look in the dark theme", async () => {
    localStorage.setItem("test-theme", "dark");
    const pending = pendingHighlight();
    const { container } = renderView({ patch });
    await pending.resolve(tokens);
    const keyword = [...container.querySelectorAll("code span")].find((span) => span.textContent === "const");
    expect((keyword as HTMLElement).style.color).toBe("rgb(249, 117, 131)");
  });

  it("numbers lines and tells assistive technology which lines were added and removed", () => {
    highlighter.highlightDiff.mockReturnValue(new Promise(() => undefined));
    renderView({ patch });
    const added = screen.getByText("let b = 3;").closest("div");
    const removed = screen.getByText("let b = 2;").closest("div");
    expect(added).toHaveTextContent(/^2\+Added: let b = 3;$/);
    expect(removed).toHaveTextContent(/^2-Removed: let b = 2;$/);
    expect(screen.getByText("const a = 1;").closest("div")).toHaveTextContent(/^11 const a = 1;$/);
  });

  it("marks a missing newline at the end of the file", () => {
    highlighter.highlightDiff.mockReturnValue(new Promise(() => undefined));
    renderView({ patch: "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n\\ No newline at end of file\n+x\n" });
    expect(screen.getByText("No newline at end of file")).toBeInTheDocument();
  });

  it("stays plain text and says so when highlighting fails or times out", async () => {
    const pending = pendingHighlight();
    renderView({ patch });
    await pending.reject(new Error("Syntax highlighting took too long."));
    expect(screen.getByText("Syntax highlighting is unavailable for this diff.")).toBeInTheDocument();
    expect(screen.getByText("let b = 3;")).toBeInTheDocument();
  });

  it("drops a stale result and cancels highlighting the diff it replaced", async () => {
    const first = pendingHighlight();
    const second = pendingHighlight();
    const next = patch.replace("+let b = 3;", "+let b = 4;");
    const view = renderView({ patch });
    const firstSignal = highlighter.highlightDiff.mock.calls[0][2] as AbortSignal;
    view.rerenderWith(next);
    expect(firstSignal.aborted).toBe(true);

    await first.resolve(tokens);
    expect(screen.getByText("let b = 4;")).toBeInTheDocument();
    await second.resolve({ ...tokens, additions: [tokens.additions[0], [{ text: "let b = 4;", light: { color: "#005cc5" } }]] });
    expect(screen.getByText("let b = 4;")).toHaveStyle({ color: "rgb(0, 92, 197)" });

    view.unmount();
    expect((highlighter.highlightDiff.mock.calls[1][2] as AbortSignal).aborted).toBe(true);
  });

  it("windows a very large diff and leaves it unhighlighted", () => {
    const lines = MAX_HIGHLIGHTED_LINES_PER_SIDE * 5;
    renderView({ patch: largePatch(lines), path: "big.ts", height: 400 });
    expect(highlighter.highlightDiff).not.toHaveBeenCalled();
    expect(screen.getByText("Syntax highlighting is off for diffs this large.")).toBeInTheDocument();
    expect(screen.getByText("line 1")).toBeInTheDocument();
    expect(screen.queryByText(`line ${lines}`)).not.toBeInTheDocument();
    // Only the visible rows plus a small overscan are mounted.
    const mounted = screen.getAllByText(/^line \d+$/).length;
    expect(mounted).toBeGreaterThan(10);
    expect(mounted).toBeLessThan(50);
    // The full height is still reserved, so the scrollbar reflects the whole diff.
    const spacer = screen.getByTestId("scroller").querySelector<HTMLElement>(".relative");
    expect(spacer?.style.height).toBe(`${32 + lines * 20}px`);
  });

  it("highlights a diff right at the size limit", () => {
    highlighter.highlightDiff.mockReturnValue(new Promise(() => undefined));
    renderView({ patch: largePatch(MAX_HIGHLIGHTED_LINES_PER_SIDE), path: "big.ts", height: 400 });
    expect(highlighter.highlightDiff).toHaveBeenCalledTimes(1);
    expect(screen.queryByText(/Syntax highlighting/)).not.toBeInTheDocument();
  });

  it("shows a change with no lines, such as a mode change, verbatim", () => {
    const modeOnly = "diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n";
    renderView({ patch: modeOnly, path: "run.sh" });
    expect(highlighter.highlightDiff).not.toHaveBeenCalled();
    expect(document.querySelector("pre")?.textContent).toBe(modeOnly);
  });

  it("shows a patch that is not exactly one file verbatim", () => {
    const raw = "@@ -1 +1 @@\n-a\n+b\n";
    renderView({ patch: raw });
    expect(highlighter.highlightDiff).not.toHaveBeenCalled();
    expect(document.querySelector("pre")?.textContent).toBe(raw);
  });
});
