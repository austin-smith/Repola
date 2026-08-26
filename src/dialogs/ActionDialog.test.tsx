import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ActionDialog } from "./ActionDialog";
import type { ActionPlan } from "../ipc/types";

const plan: ActionPlan = {
  kind: "remove",
  title: "Remove Worktree",
  summary: "Git will remove the linked directory and retain its branch.",
  repositoryPath: "/tmp/repository",
  worktreePath: "/tmp/linked",
  branch: "old-work",
  expectedHead: "1234567890abcdef",
  commandDisplay: "git worktree remove -- /tmp/linked",
  affectedPaths: ["/tmp/linked"],
  warnings: ["The directory will be deleted by Git."],
  confirmationText: "REMOVE",
  destructive: true,
};

describe("ActionDialog", () => {
  it("requires the exact confirmation and closes with Escape", async () => {
    const onCancel = vi.fn();
    const onConfirm = vi.fn();
    render(<ActionDialog plan={plan} busy={false} error={null} onCancel={onCancel} onConfirm={onConfirm} />);

    const input = screen.getByRole("textbox", { name: /type remove to confirm/i });
    const confirm = screen.getByRole("button", { name: "REMOVE" });
    await waitFor(() => expect(input).toHaveFocus());
    expect(confirm).toBeDisabled();
    fireEvent.change(input, { target: { value: "remove" } });
    expect(confirm).toBeEnabled();
    fireEvent.keyDown(input, { key: "Escape" });
    expect(onCancel).toHaveBeenCalledOnce();
    expect(onConfirm).not.toHaveBeenCalled();
  });
});
