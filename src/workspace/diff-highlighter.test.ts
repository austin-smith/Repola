import { afterEach, describe, expect, it, vi } from "vitest";
import type { DiffTokens } from "../domain/diff-tokens";
import { DiffHighlighter, type HighlightRequest, type HighlightResponse, type HighlightWorker } from "./diff-highlighter";
import type { HighlightGrammar } from "./highlight-grammar";

const tokens: DiffTokens = { deletions: [], additions: [[{ text: "a" }]] };
const typescript: HighlightGrammar = { lang: "typescript", themes: [], languages: [] };
const python: HighlightGrammar = { lang: "python", themes: [], languages: [] };

class FakeWorker implements HighlightWorker {
  readonly received: HighlightRequest[] = [];
  terminated = false;
  onmessage: ((event: MessageEvent<HighlightResponse>) => void) | null = null;
  onerror: ((event: ErrorEvent) => void) | null = null;

  postMessage(request: HighlightRequest) {
    this.received.push(request);
  }

  terminate() {
    this.terminated = true;
  }

  reply(response: HighlightResponse) {
    this.onmessage?.(new MessageEvent("message", { data: response }));
  }

  crash(message: string) {
    this.onerror?.(new ErrorEvent("error", { message, cancelable: true }));
  }

  get last(): HighlightRequest {
    const request = this.received[this.received.length - 1];
    if (!request) throw new Error("the worker received no request");
    return request;
  }
}

function setup(timeoutMs = 1_000) {
  const workers: FakeWorker[] = [];
  const highlighter = new DiffHighlighter(() => {
    const worker = new FakeWorker();
    workers.push(worker);
    return worker;
  }, timeoutMs);
  return { highlighter, workers };
}

describe("DiffHighlighter", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("hands the worker one diff at a time, in order", async () => {
    const { highlighter, workers } = setup();
    const first = highlighter.highlight("patch 1", typescript, new AbortController().signal);
    const second = highlighter.highlight("patch 2", python, new AbortController().signal);
    expect(workers).toHaveLength(1);
    expect(workers[0].received).toEqual([{ id: 1, patch: "patch 1", grammar: typescript }]);

    workers[0].reply({ id: 1, tokens });
    await expect(first).resolves.toBe(tokens);
    expect(workers[0].last).toEqual({ id: 2, patch: "patch 2", grammar: python });
    workers[0].reply({ id: 2, error: "Expected exactly one file in the patch." });
    await expect(second).rejects.toThrow("Expected exactly one file in the patch.");
    expect(workers).toHaveLength(1);
  });

  it("drops a cancelled diff that has not started without disturbing the worker", async () => {
    const { highlighter, workers } = setup();
    const first = highlighter.highlight("patch 1", typescript, new AbortController().signal);
    const cancelled = new AbortController();
    const second = highlighter.highlight("patch 2", typescript, cancelled.signal);
    const third = highlighter.highlight("patch 3", typescript, new AbortController().signal);
    cancelled.abort();
    await expect(second).rejects.toBe(cancelled.signal.reason);

    workers[0].reply({ id: 1, tokens });
    await first;
    expect(workers[0].last.patch).toBe("patch 3");
    expect(workers[0].terminated).toBe(false);
    workers[0].reply({ id: 3, tokens });
    await expect(third).resolves.toBe(tokens);
  });

  it("ends the worker when the diff it is busy with is cancelled, and starts the next on a fresh one", async () => {
    const { highlighter, workers } = setup();
    const cancelled = new AbortController();
    const first = highlighter.highlight("patch 1", typescript, cancelled.signal);
    const second = highlighter.highlight("patch 2", typescript, new AbortController().signal);
    cancelled.abort();
    await expect(first).rejects.toBe(cancelled.signal.reason);
    expect(workers[0].terminated).toBe(true);
    expect(workers[1].received).toEqual([{ id: 2, patch: "patch 2", grammar: typescript }]);
    workers[1].reply({ id: 2, tokens });
    await expect(second).resolves.toBe(tokens);
  });

  it("gives up on a diff that takes too long", async () => {
    vi.useFakeTimers();
    const { highlighter, workers } = setup(1_000);
    const slow = highlighter.highlight("patch 1", typescript, new AbortController().signal);
    const next = highlighter.highlight("patch 2", typescript, new AbortController().signal);
    vi.advanceTimersByTime(999);
    expect(workers[0].terminated).toBe(false);
    vi.advanceTimersByTime(1);
    await expect(slow).rejects.toThrow("Syntax highlighting took too long.");
    expect(workers[0].terminated).toBe(true);
    // The next diff gets its own full allowance.
    workers[1].reply({ id: 2, tokens });
    await expect(next).resolves.toBe(tokens);
  });

  it("fails the current diff when the worker crashes and recovers for the next", async () => {
    const { highlighter, workers } = setup();
    const first = highlighter.highlight("patch 1", typescript, new AbortController().signal);
    const second = highlighter.highlight("patch 2", typescript, new AbortController().signal);
    workers[0].crash("Failed to load the worker script.");
    await expect(first).rejects.toThrow("Failed to load the worker script.");
    expect(workers[0].terminated).toBe(true);
    workers[1].reply({ id: 2, tokens });
    await expect(second).resolves.toBe(tokens);
  });

  it("ignores a late reply from a diff that is no longer current", async () => {
    const { highlighter, workers } = setup();
    const current = highlighter.highlight("patch 1", typescript, new AbortController().signal);
    workers[0].reply({ id: 99, tokens: { deletions: [], additions: [] } });
    workers[0].reply({ id: 1, tokens });
    await expect(current).resolves.toBe(tokens);
  });

  it("rejects without a worker when one cannot be created, and tries again for the next diff", async () => {
    let attempts = 0;
    const worker = new FakeWorker();
    const highlighter = new DiffHighlighter(() => {
      attempts += 1;
      if (attempts === 1) throw new Error("Workers are unavailable.");
      return worker;
    }, 1_000);
    await expect(highlighter.highlight("patch 1", typescript, new AbortController().signal)).rejects.toThrow("Workers are unavailable.");
    const next = highlighter.highlight("patch 2", typescript, new AbortController().signal);
    worker.reply({ id: 2, tokens });
    await expect(next).resolves.toBe(tokens);
  });

  it("rejects at once for an already-cancelled diff", async () => {
    const { highlighter, workers } = setup();
    const controller = new AbortController();
    controller.abort();
    await expect(highlighter.highlight("patch", typescript, controller.signal)).rejects.toBe(controller.signal.reason);
    expect(workers).toHaveLength(0);
  });
});
