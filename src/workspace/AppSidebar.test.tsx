import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { MachineProfile } from "../ipc/types";
import { AppSidebar } from "./AppSidebar";

const localMachine: MachineProfile = {
  id: "local",
  name: "This computer",
  kind: "local",
  enabled: true,
  ssh: null,
};

const remoteMachine: MachineProfile = {
  id: "remote",
  name: "Build server",
  kind: "ssh",
  enabled: true,
  ssh: {
    host: "build.example.com",
    port: 22,
    user: "developer",
  },
};

describe("AppSidebar", () => {
  it("hides machine switching when there is only one enabled machine", () => {
    render(
      <TooltipProvider>
        <AppSidebar
          machines={[localMachine]}
          selectedMachine={localMachine}
          selectedMachineId={localMachine.id}
          connection={null}
          disabled={false}
          view="changes"
          onMachineChange={vi.fn()}
          onRetry={vi.fn()}
          onViewChange={vi.fn()}
          onOpenSettings={vi.fn()}
        />
      </TooltipProvider>,
    );

    expect(screen.queryByRole("combobox", { name: "Current machine" })).not.toBeInTheDocument();
    expect(screen.queryByText("Machine")).not.toBeInTheDocument();
    expect(screen.queryByText("Runs on this computer")).not.toBeInTheDocument();
    expect(screen.queryByText("Git workspace")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Toggle theme" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Settings" })).toBeInTheDocument();
  });

  it("shows machine switching when multiple enabled machines exist", () => {
    render(
      <TooltipProvider>
        <AppSidebar
          machines={[localMachine, remoteMachine]}
          selectedMachine={localMachine}
          selectedMachineId={localMachine.id}
          connection={null}
          disabled={false}
          view="changes"
          onMachineChange={vi.fn()}
          onRetry={vi.fn()}
          onViewChange={vi.fn()}
          onOpenSettings={vi.fn()}
        />
      </TooltipProvider>,
    );

    expect(screen.getByRole("combobox", { name: "Current machine" })).toHaveTextContent("This computer");
  });
});
