import { useState } from "react";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ThemeProvider } from "@/components/theme-provider";
import { CommitFileDiffView } from "./CommitFileDiffView";
import type { CommitChangedFile, FileDiff } from "../ipc/types";

const ipc = vi.hoisted(() => ({ fetchCommitFileDiff: vi.fn() }));
vi.mock("../ipc/worktrees", () => ipc);
vi.mock("@pierre/diffs/react", () => ({ PatchDiff: ({ patch }: { patch: string }) => <pre data-testid="patch-diff">{patch}</pre> }));

const commit = "a".repeat(40);
const file: CommitChangedFile = {
  id: "src/a.ts",
  path: { display: "src/a.ts", token: "src/a.ts" },
  previousPath: null,
  status: "M",
  kind: "modified",
};
const exact: FileDiff = {
  patch: "diff --git a/src/a.ts b/src/a.ts\n@@ -1,2 +1,2 @@\n-a \n+a\n-b\n+B\n",
  truncated: false,
  binary: false,
  submodule: false,
  image: null,
  hunks: [],
  stagedHunks: [],
  unstagedHunks: [],
};
const filtered: FileDiff = { ...exact, patch: "diff --git a/src/a.ts b/src/a.ts\n@@ -1,2 +1,2 @@\n a\n-b\n+B\n" };

function renderView(hideWhitespace: boolean) {
  const onHideWhitespaceChange = vi.fn();
  const tree = (hidden: boolean) => (
    <ThemeProvider storageKey="test-theme">
      <Host>
        {(controlsElement) => (
          <CommitFileDiffView
            machineId="local"
            repositoryPath="/repo"
            worktreePath="/repo"
            commit={commit}
            file={file}
            hideWhitespace={hidden}
            onHideWhitespaceChange={onHideWhitespaceChange}
            controlsElement={controlsElement}
          />
        )}
      </Host>
    </ThemeProvider>
  );
  const { rerender } = render(tree(hideWhitespace));
  return { onHideWhitespaceChange, rerenderWith: (hidden: boolean) => rerender(tree(hidden)) };
}

function Host({ children }: { children: (controlsElement: HTMLDivElement | null) => React.ReactNode }) {
  const [controlsElement, setControlsElement] = useState<HTMLDivElement | null>(null);
  return (
    <>
      <div ref={setControlsElement} data-testid="diff-controls" />
      {children(controlsElement)}
    </>
  );
}

describe("CommitFileDiffView", () => {
  afterEach(cleanup);
  beforeEach(() => {
    ipc.fetchCommitFileDiff.mockReset();
  });

  it("loads the commit diff in the chosen whitespace mode and keeps it up while the other mode loads", async () => {
    ipc.fetchCommitFileDiff.mockResolvedValueOnce(exact);
    const { rerenderWith } = renderView(false);
    expect(await screen.findByTestId("patch-diff")).toHaveTextContent("+B");
    expect(ipc.fetchCommitFileDiff).toHaveBeenLastCalledWith("local", "/repo", "/repo", commit, file.path, { ignoreWhitespace: false }, expect.any(AbortSignal));

    let resolve: (value: FileDiff) => void = () => undefined;
    ipc.fetchCommitFileDiff.mockReturnValueOnce(new Promise<FileDiff>((next) => { resolve = next; }));
    rerenderWith(true);
    expect(ipc.fetchCommitFileDiff).toHaveBeenLastCalledWith("local", "/repo", "/repo", commit, file.path, { ignoreWhitespace: true }, expect.any(AbortSignal));
    // The previous mode's diff and the control that switched it stay on screen.
    expect(screen.getByTestId("patch-diff").textContent).toBe(exact.patch);
    expect(screen.getByRole("button", { name: /^Diff options/ })).toBeInTheDocument();

    await act(async () => { resolve(filtered); });
    expect(screen.getByTestId("patch-diff").textContent).toBe(filtered.patch);
  });

  it("offers the whitespace choice in the header for text diffs", async () => {
    ipc.fetchCommitFileDiff.mockResolvedValue(exact);
    const { onHideWhitespaceChange } = renderView(false);
    fireEvent.click(await within(screen.getByTestId("diff-controls")).findByRole("button", { name: "Diff options" }));
    fireEvent.click(await screen.findByRole("menuitemradio", { name: "Hide" }));
    expect(onHideWhitespaceChange).toHaveBeenCalledExactlyOnceWith(true);
  });

  it("explains a whitespace-only change and shows whitespace on request", async () => {
    ipc.fetchCommitFileDiff.mockResolvedValue({ ...exact, patch: "" });
    const { onHideWhitespaceChange } = renderView(true);
    expect(await screen.findByText("Only whitespace changes found")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Show whitespace" }));
    expect(onHideWhitespaceChange).toHaveBeenCalledExactlyOnceWith(false);
    expect(screen.getByRole("button", { name: /^Diff options/ })).toHaveFocus();
  });

  it("keeps binary and submodule fallbacks free of diff options", async () => {
    ipc.fetchCommitFileDiff.mockResolvedValue({ ...exact, binary: true });
    renderView(false);
    expect(await screen.findByText("Binary file changed")).toBeInTheDocument();
    expect(screen.getByTestId("diff-controls")).toBeEmptyDOMElement();
    cleanup();

    ipc.fetchCommitFileDiff.mockResolvedValue({ ...exact, submodule: true, patch: "Subproject commit abc" });
    renderView(true);
    expect(await screen.findByText("Submodule commit changed")).toBeInTheDocument();
    expect(screen.getByTestId("diff-controls")).toBeEmptyDOMElement();
  });
});
