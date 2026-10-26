import type { DiffTokens } from "../domain/diff-tokens";
import { resolveGrammar, type HighlightGrammar } from "./highlight-grammar";

export interface HighlightRequest {
  readonly id: number;
  readonly patch: string;
  readonly grammar: HighlightGrammar;
}

export type HighlightResponse =
  | { readonly id: number; readonly tokens: DiffTokens }
  | { readonly id: number; readonly error: string };

/** The parts of a `Worker` the highlighter uses, so tests can stand in for one. */
export interface HighlightWorker {
  postMessage(request: HighlightRequest): void;
  terminate(): void;
  onmessage: ((event: MessageEvent<HighlightResponse>) => void) | null;
  onerror: ((event: ErrorEvent) => void) | null;
}

interface Job {
  readonly request: HighlightRequest;
  readonly signal: AbortSignal;
  readonly resolve: (tokens: DiffTokens) => void;
  readonly reject: (reason: unknown) => void;
  readonly onAbort: () => void;
}

/**
 * Runs diff highlighting on a worker, one diff at a time, so the UI thread
 * only ever receives finished tokens. A job that is cancelled or runs past
 * the timeout while the worker is busy with it ends that worker and the next
 * job starts on a fresh one: the highlighter cannot be interrupted any other
 * way, and a stalled diff must not hold up every diff after it.
 */
export class DiffHighlighter {
  private worker: HighlightWorker | null = null;
  private active: { job: Job; timer: ReturnType<typeof setTimeout> } | null = null;
  private readonly queue: Job[] = [];
  private nextId = 1;

  constructor(
    private readonly createWorker: () => HighlightWorker,
    private readonly timeoutMs: number,
  ) {}

  highlight(patch: string, grammar: HighlightGrammar, signal: AbortSignal): Promise<DiffTokens> {
    if (signal.aborted) return Promise.reject(signal.reason);
    return new Promise<DiffTokens>((resolve, reject) => {
      const job: Job = {
        request: { id: this.nextId++, patch, grammar },
        signal,
        resolve,
        reject,
        onAbort: () => this.cancel(job, signal.reason),
      };
      signal.addEventListener("abort", job.onAbort, { once: true });
      this.queue.push(job);
      this.startNext();
    });
  }

  private startNext() {
    if (this.active) return;
    const job = this.queue.shift();
    if (!job) return;
    let worker: HighlightWorker;
    try {
      worker = this.ensureWorker();
    } catch (cause) {
      this.settle(job, () => job.reject(cause));
      this.startNext();
      return;
    }
    const timer = setTimeout(() => this.cancel(job, new Error("Syntax highlighting took too long.")), this.timeoutMs);
    this.active = { job, timer };
    worker.postMessage(job.request);
  }

  private ensureWorker(): HighlightWorker {
    if (this.worker) return this.worker;
    const worker = this.createWorker();
    worker.onmessage = (event) => this.finish(event.data);
    worker.onerror = (event) => {
      // The worker itself failed (it may not have loaded at all). Fail the
      // diff it was working on; the next diff gets a fresh worker.
      event.preventDefault();
      const job = this.active?.job;
      this.discardWorker();
      if (job) this.settle(job, () => job.reject(new Error(event.message || "Syntax highlighting failed.")));
      this.startNext();
    };
    this.worker = worker;
    return worker;
  }

  private finish(response: HighlightResponse) {
    const job = this.active?.job;
    if (!job || job.request.id !== response.id) return;
    this.settle(job, () => ("tokens" in response ? job.resolve(response.tokens) : job.reject(new Error(response.error))));
    this.startNext();
  }

  private cancel(job: Job, reason: unknown) {
    if (this.active?.job === job) {
      this.discardWorker();
    } else {
      const index = this.queue.indexOf(job);
      if (index < 0) return;
      this.queue.splice(index, 1);
    }
    this.settle(job, () => job.reject(reason));
    this.startNext();
  }

  private settle(job: Job, outcome: () => void) {
    if (this.active?.job === job) {
      clearTimeout(this.active.timer);
      this.active = null;
    }
    job.signal.removeEventListener("abort", job.onAbort);
    outcome();
  }

  private discardWorker() {
    this.worker?.terminate();
    this.worker = null;
  }
}

/**
 * How long one diff may take to highlight before it is shown as plain text.
 * The largest diffs that are highlighted at all take a few seconds.
 */
const HIGHLIGHT_TIMEOUT_MS = 20_000;

let shared: DiffHighlighter | null = null;

/**
 * Highlights a single-file patch off the UI thread; `path` is the file's real
 * path, which decides the language. Rejects if highlighting fails, times out,
 * or is aborted.
 */
export async function highlightDiff(patch: string, path: string, signal: AbortSignal): Promise<DiffTokens> {
  const grammar = await resolveGrammar(path);
  shared ??= new DiffHighlighter(
    () => new Worker(new URL("./diff-highlight.worker.ts", import.meta.url), { type: "module" }),
    HIGHLIGHT_TIMEOUT_MS,
  );
  return shared.highlight(patch, grammar, signal);
}
