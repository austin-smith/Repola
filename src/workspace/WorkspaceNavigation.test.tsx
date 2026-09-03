import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { WorkspaceNavigation } from "./WorkspaceNavigation";

describe("WorkspaceNavigation", () => {
  it("shows the three primary destinations without redundant group labels", () => {
    const onViewChange = vi.fn();
    render(<WorkspaceNavigation view="changes" onViewChange={onViewChange} />);

    expect(screen.queryByText("Workspace")).not.toBeInTheDocument();
    expect(screen.queryByText("Manage")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show Changes" })).toHaveAttribute("aria-current", "page");
    fireEvent.click(screen.getByRole("button", { name: "Open Worktree Manager" }));
    expect(onViewChange).toHaveBeenCalledWith("worktrees");
  });
});
