import { describe, expect, it } from "vitest";
import { RefreshQueue, refreshIntent, type RefreshIntent } from "./refresh-queue";

const tick = () => Promise.resolve();

describe("refresh queue", () => {
  it("combines a burst and runs one follow-up for events received during a read", async () => {
    const requests: RefreshIntent[] = [];
    let finish: (() => void) | undefined;
    const queue = new RefreshQueue(async (intent) => {
      requests.push(intent);
      if (requests.length === 1) await new Promise<void>((resolve) => { finish = resolve; });
    });
    queue.request(refreshIntent({ worktrees: new Set(["a"]) }));
    queue.request(refreshIntent({ worktrees: new Set(["b"]) }));
    await tick();
    expect([...requests[0].worktrees]).toEqual(["a", "b"]);
    for (let i = 0; i < 10_000; i++) queue.request(refreshIntent({ worktrees: new Set(["c"]), git: true }));
    await tick();
    expect(requests).toHaveLength(1);
    finish?.();
    await queue.waitForIdle();
    expect(requests).toHaveLength(2);
    expect([...requests[1].worktrees]).toEqual(["c"]);
    expect(requests[1].git).toBe(true);
  });

  it("fences an older read synchronously and retains invalidations across nested mutations", async () => {
    const accepted: boolean[] = [];
    let finish: (() => void) | undefined;
    const queue = new RefreshQueue(async (_intent, isCurrent) => {
      if (accepted.length === 0) await new Promise<void>((resolve) => { finish = resolve; });
      accepted.push(isCurrent());
    });
    queue.request(refreshIntent({ all: true }));
    await tick();
    queue.mutationStarted();
    queue.mutationStarted();
    finish?.();
    await tick();
    expect(accepted).toEqual([false]);
    queue.mutationSettled(refreshIntent({ worktrees: new Set(["a"]) }));
    await tick();
    expect(accepted).toHaveLength(1);
    queue.mutationSettled(refreshIntent({ worktrees: new Set(["b"]) }));
    await queue.waitForIdle();
    expect(accepted).toEqual([false, true]);
  });

  it("retains hidden-window work, permits explicit refresh, and rejects disposed responses", async () => {
    const accepted: boolean[] = [];
    const queue = new RefreshQueue(async (_intent, isCurrent) => { accepted.push(isCurrent()); });
    queue.setPaused(true);
    queue.request(refreshIntent({ all: true }));
    await tick();
    expect(accepted).toEqual([]);
    queue.request(refreshIntent({ manual: true }));
    await queue.waitForIdle();
    expect(accepted).toEqual([true]);
    queue.request(refreshIntent({ all: true }));
    queue.dispose();
    await queue.waitForIdle();
    expect(accepted).toEqual([true]);
  });

  it("recovers after a rejected read without leaking a waiter", async () => {
    let reads = 0;
    const queue = new RefreshQueue(async () => { if (++reads === 1) throw new Error("offline"); });
    queue.request(refreshIntent({ all: true }));
    await queue.waitForIdle();
    queue.request(refreshIntent({ all: true }));
    await queue.waitForIdle();
    expect(reads).toBe(2);
  });

  it("does not declare a mutation idle before its finally reconciliation", async () => {
    const busy: boolean[] = [];
    const queue = new RefreshQueue(async () => {}, (value) => busy.push(value));
    queue.mutationStarted();
    let settled = false;
    const waiting = queue.waitForIdle().then(() => { settled = true; });
    await tick();
    expect(settled).toBe(false);
    expect(queue.busy).toBe(true);
    queue.mutationSettled(refreshIntent({ all: true }));
    await waiting;
    expect(busy).toEqual([true, false]);
  });
});
