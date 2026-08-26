import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ConflictResolutionDialog from "./ConflictResolutionDialog";
import type { ConflictResolutionKind, FileChange, RepositorySummary, WorkingCopySnapshot, WorktreeRecord } from "../ipc/types";

const ipc = vi.hoisted(() => ({
  loadConflictFile: vi.fn(),
  resolveConflict: vi.fn(),
}));

vi.mock("../ipc/worktrees", () => ipc);

const repository = { id: "repo", name: "repola", path: "/tmp/repola" } as RepositorySummary;
const worktree = { id: "wt", path: "/tmp/repola-feature", repositoryPath: "/tmp/repola" } as WorktreeRecord;

function conflictChange(id: string, conflicted = true): FileChange {
  return {
    id,
    path: { display: id, token: id },
    previousPath: null,
    kind: conflicted ? "unmerged" : "modified",
    indexStatus: conflicted ? "U" : ".",
    worktreeStatus: conflicted ? "U" : "M",
    staged: false,
    unstaged: true,
    conflicted,
    untracked: false,
    ignored: false,
    submodule: false,
    headMode: null,
    indexMode: null,
    worktreeMode: null,
    modeChange: null,
  };
}

const change = conflictChange("src/a.ts");
const snapshot: WorkingCopySnapshot = {
  repositoryPath: repository.path,
  worktreePath: worktree.path,
  head: "abc123def456",
  branch: "feature",
  upstream: null,
  upstreamHead: null,
  remote: null,
  ahead: 0,
  behind: 0,
  changes: [change, conflictChange("src/b.ts"), conflictChange("src/c.ts", false)],
  operation: "merge",
};

const original = "<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> other\n";

function renderDialog(initialKind: ConflictResolutionKind) {
  const onClose = vi.fn();
  const onSnapshot = vi.fn();
  render(
    <ConflictResolutionDialog
      machineId="local"
      repository={repository}
      worktree={worktree}
      snapshot={snapshot}
      change={change}
      initialKind={initialKind}
      onClose={onClose}
      onSnapshot={onSnapshot}
    />,
  );
  return { onClose, onSnapshot };
}

const applyButton = () => screen.getByRole("button", { name: /apply and stage|remove and stage/i });

describe("ConflictResolutionDialog", () => {
  afterEach(cleanup);

  beforeEach(() => {
    ipc.loadConflictFile.mockReset();
    ipc.resolveConflict.mockReset();
    ipc.loadConflictFile.mockResolvedValue({ content: original, byteLength: original.length });
    ipc.resolveConflict.mockResolvedValue({ ...snapshot, changes: [] });
  });

  it("shows the unresolved count from the snapshot, counting only conflicted files", async () => {
    renderDialog("ours");
    expect(screen.getByText("2 unresolved")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Resolve src/a.ts" })).toBeInTheDocument();
    expect(await screen.findByText(`${original.length} B`)).toBeInTheDocument();
  });

  it.each([
    ["ours", "Ours"],
    ["theirs", "Theirs"],
    ["markResolved", "As-is"],
    ["remove", "Remove"],
  ] as const)("resolves with %s without sending file content", async (kind, label) => {
    const { onClose, onSnapshot } = renderDialog("ours");
    await screen.findByText(`${original.length} B`);
    fireEvent.click(screen.getByRole("button", { name: label }));
    fireEvent.click(applyButton());

    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
    expect(ipc.resolveConflict).toHaveBeenCalledExactlyOnceWith(
      "local", repository.path, worktree.path, change.path, kind, "abc123def456", null, null,
    );
    expect(onSnapshot).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ changes: [] }));
  });

  it("resolves a union merge with the revalidated original content", async () => {
    const { onClose } = renderDialog("both");
    expect(screen.getByText("Three-way union resolution")).toBeInTheDocument();
    await waitFor(() => expect(applyButton()).toBeEnabled());
    fireEvent.click(applyButton());

    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
    expect(ipc.resolveConflict).toHaveBeenCalledExactlyOnceWith(
      "local", repository.path, worktree.path, change.path, "both", "abc123def456", original, null,
    );
  });

  it("resolves manually with both the original and the edited content", async () => {
    const { onClose } = renderDialog("manual");
    const editor = await screen.findByRole("textbox", { name: "Manual conflict resolution" });
    expect(editor).toHaveValue(original);
    fireEvent.change(editor, { target: { value: "merged\n" } });
    fireEvent.click(applyButton());

    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
    expect(ipc.resolveConflict).toHaveBeenCalledExactlyOnceWith(
      "local", repository.path, worktree.path, change.path, "manual", "abc123def456", original, "merged\n",
    );
  });

  it("disables text resolutions until the conflicted file has loaded, and reports load failures", async () => {
    ipc.loadConflictFile.mockRejectedValue(new Error("file is not valid UTF-8"));
    renderDialog("manual");
    expect(applyButton()).toBeDisabled();
    expect(await screen.findByText("Text resolution unavailable")).toBeInTheDocument();
    expect(screen.getByText("file is not valid UTF-8")).toBeInTheDocument();
    expect(applyButton()).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: "Ours" }));
    expect(screen.queryByText("Text resolution unavailable")).not.toBeInTheDocument();
    expect(applyButton()).toBeEnabled();
  });

  it("surfaces resolveConflict failures and leaves the dialog open", async () => {
    ipc.resolveConflict.mockRejectedValue(new Error("error: HEAD changed since review"));
    const { onClose, onSnapshot } = renderDialog("theirs");
    fireEvent.click(applyButton());

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The conflict was not changed");
    expect(alert).toHaveTextContent("error: HEAD changed since review");
    expect(onClose).not.toHaveBeenCalled();
    expect(onSnapshot).not.toHaveBeenCalled();
    expect(applyButton()).toBeEnabled();
  });
});
