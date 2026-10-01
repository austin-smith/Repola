import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PhysicalPosition } from "@tauri-apps/api/dpi";
import type { EventCallback } from "@tauri-apps/api/event";
import type { DragDropEvent } from "@tauri-apps/api/webview";
import { RepositoryDropZone } from "../components/RepositoryDropZone";
import { useRepositoryDrop } from "./use-repository-drop";

const native = vi.hoisted(() => ({ subscribe: vi.fn(), isTauri: vi.fn(), toast: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ isTauri: native.isTauri }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: () => ({ onDragDropEvent: native.subscribe }) }));
vi.mock("../components/ui/toast", () => ({ toast: { add: native.toast } }));

function Target({ enabled = true, onDrop }: { enabled?: boolean; onDrop: (paths: string[]) => Promise<unknown> }) {
  const { ref, active } = useRepositoryDrop(enabled, onDrop);
  return <RepositoryDropZone ref={ref} active={active} disabled={!enabled} />;
}

let listener: EventCallback<DragDropEvent>;
const dispose = vi.fn();
const hitTest = vi.fn();
const paths = ["exact dropped path"];
function emit(payload: DragDropEvent) {
  act(() => listener({ event: "tauri://drag-drop", id: 1, payload }));
}
const inside = new PhysicalPosition(250, 150);
const outside = new PhysicalPosition(50, 50);

describe("scoped repository drops", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    native.isTauri.mockReturnValue(true);
    native.subscribe.mockImplementation((callback: EventCallback<DragDropEvent>) => {
      listener = callback;
      return Promise.resolve(dispose);
    });
    vi.stubGlobal("devicePixelRatio", 2);
    Object.defineProperty(document, "elementFromPoint", { configurable: true, value: hitTest });
    hitTest.mockImplementation(() => screen.getByRole("region", { name: "Drop repositories" }));
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
      left: 100, right: 300, top: 50, bottom: 150, x: 100, y: 50, width: 200, height: 100, toJSON: () => ({}),
    });
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("highlights only the target and converts physical positions to CSS pixels", () => {
    render(<Target onDrop={vi.fn()} />);
    emit({ type: "enter", position: outside, paths });
    expect(screen.getByText("Drop repositories here")).toBeInTheDocument();
    emit({ type: "over", position: inside });
    expect(hitTest).toHaveBeenLastCalledWith(125, 75);
    expect(screen.getByText("Drop to add repositories")).toBeInTheDocument();
    emit({ type: "over", position: outside });
    expect(screen.getByText("Drop repositories here")).toBeInTheDocument();
    emit({ type: "over", position: inside });
    emit({ type: "leave" });
    expect(screen.getByText("Drop repositories here")).toBeInTheDocument();
  });

  it("checks the release position instead of trusting the previous hover", async () => {
    const onDrop = vi.fn().mockResolvedValue(undefined);
    render(<Target onDrop={onDrop} />);
    emit({ type: "over", position: inside });
    emit({ type: "drop", position: outside, paths });
    expect(onDrop).not.toHaveBeenCalled();
    await act(async () => emit({ type: "drop", position: inside, paths }));
    expect(onDrop).toHaveBeenCalledExactlyOnceWith(paths);
    expect(screen.getByText("Drop repositories here")).toBeInTheDocument();
  });

  it("ignores a target covered by a dialog or another element", () => {
    const onDrop = vi.fn();
    render(<Target onDrop={onDrop} />);
    hitTest.mockReturnValue(document.body);
    emit({ type: "enter", position: inside, paths });
    emit({ type: "drop", position: inside, paths });
    expect(onDrop).not.toHaveBeenCalled();
    expect(screen.getByText("Drop repositories here")).toBeInTheDocument();
  });

  it("ignores repeated drops while the accepted drop is processing", async () => {
    let complete!: () => void;
    const onDrop = vi.fn(() => new Promise<void>((resolve) => { complete = resolve; }));
    render(<Target onDrop={onDrop} />);
    emit({ type: "drop", position: inside, paths });
    emit({ type: "drop", position: inside, paths });
    expect(onDrop).toHaveBeenCalledTimes(1);
    await act(async () => complete());
    emit({ type: "drop", position: inside, paths });
    expect(onDrop).toHaveBeenCalledTimes(2);
    await act(async () => complete());
  });

  it("does not subscribe when disabled and rejects stale events after disabling", async () => {
    const onDrop = vi.fn();
    const view = render(<Target enabled={false} onDrop={onDrop} />);
    expect(native.subscribe).not.toHaveBeenCalled();
    view.rerender(<Target enabled onDrop={onDrop} />);
    await act(async () => undefined);
    emit({ type: "over", position: inside });
    view.rerender(<Target enabled={false} onDrop={onDrop} />);
    emit({ type: "drop", position: inside, paths });
    expect(dispose).toHaveBeenCalledOnce();
    expect(onDrop).not.toHaveBeenCalled();
    view.rerender(<Target enabled onDrop={onDrop} />);
    expect(screen.getByText("Drop repositories here")).toBeInTheDocument();
  });

  it("disposes a listener that finishes subscribing after unmount", async () => {
    let complete!: (unlisten: () => void) => void;
    native.subscribe.mockImplementationOnce((callback: EventCallback<DragDropEvent>) => {
      listener = callback;
      return new Promise((resolve) => { complete = resolve; });
    });
    const onDrop = vi.fn();
    const view = render(<Target onDrop={onDrop} />);
    view.unmount();
    emit({ type: "drop", position: inside, paths });
    await act(async () => complete(dispose));
    expect(dispose).toHaveBeenCalledOnce();
    expect(onDrop).not.toHaveBeenCalled();
  });

  it("uses the latest callback without creating another native listener", async () => {
    const oldDrop = vi.fn();
    const newDrop = vi.fn().mockResolvedValue(undefined);
    const view = render(<Target onDrop={oldDrop} />);
    view.rerender(<Target onDrop={newDrop} />);
    await act(async () => emit({ type: "drop", position: inside, paths }));
    expect(native.subscribe).toHaveBeenCalledOnce();
    expect(oldDrop).not.toHaveBeenCalled();
    expect(newDrop).toHaveBeenCalledExactlyOnceWith(paths);
  });

  it("reports a failed drop and accepts a later retry", async () => {
    const onDrop = vi.fn().mockRejectedValueOnce(new Error("Registration failed")).mockResolvedValue(undefined);
    render(<Target onDrop={onDrop} />);
    await act(async () => emit({ type: "drop", position: inside, paths }));
    expect(native.toast).toHaveBeenCalledWith(expect.objectContaining({ type: "error", description: "Registration failed" }));
    await act(async () => emit({ type: "drop", position: inside, paths }));
    expect(onDrop).toHaveBeenCalledTimes(2);
  });
});
