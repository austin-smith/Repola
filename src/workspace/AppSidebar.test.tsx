import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { MachineProfile } from "../ipc/types";
import { AppSidebar } from "./AppSidebar";

const release = vi.hoisted(() => ({ channelLabel: "Dev" as string | null }));
vi.mock("@/app/release", () => ({
  get applicationChannelLabel() { return release.channelLabel; },
}));

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
  afterEach(cleanup);
  beforeEach(() => { release.channelLabel = "Dev"; });

  it.each([null, "Nightly", "Dev"])("identifies the build with channel label %s", (label) => {
    release.channelLabel = label;
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
    expect(screen.getByRole("heading", { level: 1, name: label ? `Repola ${label}` : "Repola" })).toBeVisible();
    expect(screen.queryByText("Stable")).not.toBeInTheDocument();
    if (label) expect(screen.getByText(label)).toBeVisible();
    else {
      expect(screen.queryByText("Nightly")).not.toBeInTheDocument();
      expect(screen.queryByText("Dev")).not.toBeInTheDocument();
    }
  });

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
