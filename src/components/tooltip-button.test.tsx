import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { TooltipButton } from "./tooltip-button";

describe("TooltipButton", () => {
  afterEach(cleanup);

  it("uses the button itself as the accessible tooltip trigger", async () => {
    render(
      <TooltipProvider>
        <TooltipButton aria-label="Refresh" tooltip="Refresh worktrees">
          Refresh
        </TooltipButton>
      </TooltipProvider>,
    );

    const button = screen.getByRole("button", { name: "Refresh" });
    expect(button).toHaveAttribute("data-slot", "tooltip-trigger");
    expect(button).not.toHaveAttribute("title");

    fireEvent.mouseEnter(button);
    expect(await screen.findByText("Refresh worktrees")).toHaveAttribute("data-slot", "tooltip-content");

    fireEvent.mouseLeave(button);
    expect(screen.queryByText("Refresh worktrees")).not.toBeInTheDocument();
  });

  it("wraps a disabled button so pointer users can still reach its explanation", async () => {
    render(
      <TooltipProvider>
        <TooltipButton disabled tooltip="Configure a remote first">
          Publish
        </TooltipButton>
      </TooltipProvider>,
    );

    const button = screen.getByRole("button", { name: "Publish" });
    const trigger = button.parentElement;
    expect(button).toBeDisabled();
    expect(trigger).toHaveAttribute("data-slot", "tooltip-trigger");
    expect(button).not.toHaveAttribute("title");

    fireEvent.mouseEnter(trigger!);
    expect(await screen.findByText("Configure a remote first")).toHaveAttribute("data-slot", "tooltip-content");
  });
});
