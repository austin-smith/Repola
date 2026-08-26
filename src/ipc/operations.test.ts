import { beforeEach, describe, expect, it, vi } from "vitest";
import { invokeOperation } from "./operations";

const invokeMock = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

describe("invokeOperation", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("attaches a collision-resistant operation ID", async () => {
    invokeMock.mockResolvedValue({ ok: true });
    await invokeOperation("example", { value: 7 });
    expect(invokeMock).toHaveBeenCalledOnce();
    expect(invokeMock.mock.calls[0][0]).toBe("example");
    expect(invokeMock.mock.calls[0][1]).toMatchObject({
      value: 7,
      operationId: expect.stringMatching(/^ui-[0-9a-f-]{36}$/),
    });
  });

  it("cancels the backend operation and rejects when aborted", async () => {
    let finish!: (value: unknown) => void;
    invokeMock.mockImplementation((command: string) => {
      if (command === "cancel_operation") return Promise.resolve(true);
      return new Promise((resolve) => { finish = resolve; });
    });
    const controller = new AbortController();
    const pending = invokeOperation("slow_operation", {}, { signal: controller.signal });
    controller.abort();
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(2));
    const primaryId = invokeMock.mock.calls[0][1].operationId;
    expect(invokeMock.mock.calls[1]).toEqual(["cancel_operation", { operationId: primaryId }]);
    finish({ ok: true });
    await expect(pending).rejects.toMatchObject({ name: "AbortError" });
  });

  it("does not start an operation for an already-aborted signal", async () => {
    const controller = new AbortController();
    controller.abort();
    await expect(invokeOperation("never", {}, { signal: controller.signal })).rejects.toMatchObject({
      name: "AbortError",
    });
    expect(invokeMock).not.toHaveBeenCalled();
  });
});
