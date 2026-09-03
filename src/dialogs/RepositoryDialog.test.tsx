import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { MachineProfile } from "../ipc/types";
import { RepositoryDialog } from "./RepositoryDialog";

const localMachine: MachineProfile = {
  id: "local",
  name: "This computer",
  kind: "local",
  enabled: true,
  ssh: null,
};

describe("RepositoryDialog", () => {
  it("opens on the existing-repository flow with concise local copy", () => {
    render(
      <RepositoryDialog
        machine={localMachine}
        onAddExisting={vi.fn()}
        onCompleted={vi.fn()}
        onClose={vi.fn()}
      />,
    );

    expect(screen.getByRole("heading", { name: "Add Repository" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Choose Repository…" })).toBeInTheDocument();
    expect(screen.queryByLabelText("Repository URL")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Clone" }));
    expect(screen.getByLabelText("Repository URL")).toBeInTheDocument();
  });
});
