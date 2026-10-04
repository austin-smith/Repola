import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { BulkActionDialog, type BulkItem, type BulkStage } from "./BulkActionDialog";
import type { BranchDeletionResult, BranchDeletionStep } from "../ipc/types";

// BulkActionDialog is presentational: preflight (one review per item) and
// sequential execution live in App.tsx's reviewBulkRemoval,
// reviewBulkBranchDeletion, and executeBulk, which feed `items` and `stage` back into this dialog. These
// tests pin down the contract the dialog exposes for that flow.

const ready = (key: string, warnings?: string[]): BulkItem => ({
  key,
  title: key,
  subtitle: `/tmp/${key}`,
  sizeLabel: "12 MiB",
  command: `git worktree remove -- /tmp/${key}`,
  warnings: warnings ?? [],
  error: null,
  done: false,
});

const blocked = (key: string, error: string): BulkItem => ({
  key,
  title: key,
  subtitle: `/tmp/${key}`,
  command: null,
  warnings: [],
  error,
  done: false,
});

function renderDialog(stage: BulkStage, items: BulkItem[], followUpLabel: string | null = null) {
  const onCancel = vi.fn();
  const onConfirm = vi.fn();
  const onFollowUp = vi.fn();
  render(
    <BulkActionDialog
      title="Remove 3 clean worktrees?"
      summary="Each worktree passed its own preflight."
      confirmationText="REMOVE"
      stage={stage}
      items={items}
      followUpLabel={followUpLabel}
      onCancel={onCancel}
      onConfirm={onConfirm}
      onFollowUp={onFollowUp}
    />,
  );
  return { onCancel, onConfirm, onFollowUp };
}

describe("BulkActionDialog", () => {
  afterEach(cleanup);

  it("renders per-item preflight results with the command that will run or the blocking error", () => {
    renderDialog("review", [ready("alpha"), blocked("beta", "worktree has uncommitted changes"), ready("gamma")]);

    const rows = screen.getAllByRole("listitem");
    expect(rows).toHaveLength(3);
    expect(within(rows[0]).getByText("Ready")).toBeInTheDocument();
    expect(within(rows[0]).getByText("git worktree remove -- /tmp/alpha")).toBeInTheDocument();
    expect(within(rows[0]).getByText("12 MiB")).toBeInTheDocument();
    expect(within(rows[1]).getByText("Blocked")).toBeInTheDocument();
    expect(within(rows[1]).getByText("worktree has uncommitted changes")).toBeInTheDocument();
    expect(within(rows[1]).queryByText(/git worktree remove/)).not.toBeInTheDocument();
    expect(within(rows[2]).getByText("Ready")).toBeInTheDocument();
    expect(screen.getByText("Batch preflight complete")).toBeInTheDocument();
  });

  it("counts only items that passed preflight toward execution and says blocked entries are skipped", () => {
    renderDialog("review", [ready("alpha"), blocked("beta", "dirty"), ready("gamma")]);
    expect(screen.getByRole("button", { name: "REMOVE (2)" })).toBeInTheDocument();
    expect(screen.getByText("Runs the 2 ready actions one at a time; blocked entries are skipped.")).toBeInTheDocument();
  });

  it("deduplicates plan warnings across items", () => {
    renderDialog("review", [ready("alpha", ["Branch will be retained."]), ready("beta", ["Branch will be retained.", "Directory is large."])]);
    expect(screen.getAllByText("Branch will be retained.")).toHaveLength(1);
    expect(screen.getByText("Directory is large.")).toBeInTheDocument();
  });

  it("requires the typed confirmation (upper-cased) and at least one ready item before confirming", () => {
    const { onConfirm } = renderDialog("review", [ready("alpha"), blocked("beta", "dirty")]);
    const confirm = screen.getByRole("button", { name: "REMOVE (1)" });
    const input = screen.getByLabelText("Type REMOVE to confirm");
    expect(confirm).toBeDisabled();

    fireEvent.change(input, { target: { value: "remove" } });
    expect(input).toHaveValue("REMOVE");
    expect(confirm).toBeEnabled();
    fireEvent.click(confirm);
    expect(onConfirm).toHaveBeenCalledOnce();
    cleanup();

    renderDialog("review", [blocked("beta", "dirty")]);
    fireEvent.change(screen.getByLabelText("Type REMOVE to confirm"), { target: { value: "REMOVE" } });
    expect(screen.getByRole("button", { name: "REMOVE (0)" })).toBeDisabled();
  });

  it("reports sequential progress while running and locks the controls", () => {
    const items = [
      { ...ready("alpha"), done: true },
      blocked("beta", "dirty"),
      ready("gamma"),
      ready("delta"),
    ];
    const { onCancel } = renderDialog("running", items);

    // Two items are already settled (one done, one blocked), so the third is in flight.
    const progress = screen.getByRole("button", { name: /Running 3 of 4…/ });
    expect(progress).toBeDisabled();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled();
    expect(screen.getByLabelText("Type REMOVE to confirm")).toBeDisabled();

    const rows = screen.getAllByRole("listitem");
    expect(within(rows[0]).getByText("Done")).toBeInTheDocument();
    expect(within(rows[1]).getByText("Failed")).toBeInTheDocument();
    expect(within(rows[2]).getByText("Pending…")).toBeInTheDocument();
    expect(within(rows[3]).getByText("Pending…")).toBeInTheDocument();
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("summarizes a finished batch, keeps a failed item alongside completed ones, and offers the follow-up", () => {
    const items = [
      { ...ready("alpha"), done: true },
      { ...ready("beta"), error: "fatal: worktree is locked" },
      { ...ready("gamma"), done: true },
    ];
    const { onCancel, onFollowUp } = renderDialog("done", items, "Review 2 branch deletions…");

    expect(screen.getByText("2 completed · 1 not completed")).toBeInTheDocument();
    expect(screen.queryByLabelText("Type REMOVE to confirm")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^REMOVE/ })).not.toBeInTheDocument();
    const rows = screen.getAllByRole("listitem");
    expect(within(rows[1]).getByText("Failed")).toBeInTheDocument();
    expect(within(rows[1]).getByText("fatal: worktree is locked")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Review 2 branch deletions…" }));
    expect(onFollowUp).toHaveBeenCalledOnce();
    // The dialog also renders an icon-only "Close" (X); target the footer button by its visible text.
    fireEvent.click(screen.getAllByRole("button", { name: "Close" }).find((button) => button.textContent === "Close")!);
    expect(onCancel).toHaveBeenCalledOnce();
  });

  it("marks an interrupted item as unconfirmed rather than failed", () => {
    const items = [
      { ...ready("alpha"), done: true },
      { ...ready("beta"), error: "The SSH connection closed. The deletion may have completed anyway.", unconfirmed: true },
    ];
    renderDialog("done", items);

    expect(screen.getByText("1 completed · 0 not completed · 1 unconfirmed")).toBeInTheDocument();
    const rows = screen.getAllByRole("listitem");
    expect(within(rows[1]).getByText("Unconfirmed")).toBeInTheDocument();
    expect(within(rows[1]).queryByText("Failed")).not.toBeInTheDocument();
    expect(within(rows[1]).getByText(/may have completed anyway/)).toBeInTheDocument();
  });

  it("reports what each finished branch deletion left to do and how to restore it", () => {
    const step = (warning: string | null, finishCommands: string[]): BranchDeletionStep => ({
      target: "feature",
      deletedOid: "a".repeat(40),
      succeeded: true,
      output: "",
      warning,
      finishCommands,
      recoveryCommands: ["git branch -- feature aaaa"],
    });
    const deletion = (local: BranchDeletionStep): BranchDeletionResult => ({
      message: "Deleted local branch feature.",
      local,
      remote: null,
      auditPath: null,
      auditWarning: null,
    });
    const items = [
      { ...ready("alpha"), done: true, deletion: deletion(step(null, [])) },
      {
        ...ready("beta"),
        done: true,
        deletion: deletion(step("feature was deleted, but its configuration was not removed.", ["git config --local --remove-section branch.feature"])),
      },
    ];
    renderDialog("done", items);

    const rows = screen.getAllByRole("listitem");
    expect(within(rows[0]).getByText("Done")).toBeInTheDocument();
    expect(within(rows[0]).getByText("To restore: git branch -- feature aaaa")).toBeInTheDocument();
    expect(within(rows[1]).getByText("Done with warnings")).toBeInTheDocument();
    expect(within(rows[1]).getByText(/To finish: git config --local --remove-section branch\.feature/)).toBeInTheDocument();
    expect(within(rows[1]).getByText(/To restore: git branch -- feature aaaa/)).toBeInTheDocument();
  });
});
