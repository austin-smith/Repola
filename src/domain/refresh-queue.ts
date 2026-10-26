/** Scopes accumulate while a read is active; no event cancels useful work. */
export interface RefreshIntent {
  all: boolean;
  repositories: ReadonlySet<string>;
  worktrees: ReadonlySet<string>;
  measurements: ReadonlySet<string>;
  metadata: boolean;
  git: boolean;
  visible: boolean;
  manual: boolean;
}

export function refreshIntent(overrides: Partial<RefreshIntent> = {}): RefreshIntent {
  return { all: false, repositories: new Set(), worktrees: new Set(), measurements: new Set(), metadata: false, git: false, visible: false, manual: false, ...overrides };
}

function merge(left: RefreshIntent, right: RefreshIntent): RefreshIntent {
  return {
    all: left.all || right.all,
    repositories: new Set([...left.repositories, ...right.repositories]),
    worktrees: new Set([...left.worktrees, ...right.worktrees]),
    measurements: new Set([...left.measurements, ...right.measurements]),
    metadata: left.metadata || right.metadata,
    git: left.git || right.git,
    visible: left.visible || right.visible,
    manual: left.manual || right.manual,
  };
}

/** One read at a time, one bounded follow-up, and a synchronous mutation fence. */
export class RefreshQueue {
  private pending: RefreshIntent | null = null;
  private running = false;
  private scheduled = false;
  private paused = false;
  private mutations = 0;
  private disposed = false;
  private generation = 0;
  private idlePromise: Promise<void> | null = null;
  private resolveIdle: (() => void) | null = null;
  private lastBusy = false;

  constructor(
    private readonly run: (intent: RefreshIntent, isCurrent: () => boolean) => Promise<void>,
    private readonly onBusyChange?: (busy: boolean) => void,
  ) {}

  get busy(): boolean {
    return !this.disposed && (this.running || this.pending !== null || this.mutations > 0);
  }

  private notify(): void {
    if (this.busy === this.lastBusy) return;
    this.lastBusy = this.busy;
    this.onBusyChange?.(this.busy);
  }

  request(intent: RefreshIntent): void {
    if (this.disposed) return;
    this.pending = this.pending ? merge(this.pending, intent) : intent;
    this.notify();
    // Deferring the start also combines independent invalidations in one turn.
    if (!this.scheduled) {
      this.scheduled = true;
      void Promise.resolve().then(() => {
        this.scheduled = false;
        return this.drain();
      });
    }
  }

  waitForIdle(): Promise<void> {
    if (!this.busy) return Promise.resolve();
    if (!this.idlePromise) this.idlePromise = new Promise((resolve) => { this.resolveIdle = resolve; });
    return this.idlePromise;
  }

  setPaused(paused: boolean): void {
    this.paused = paused;
    if (!paused) void this.drain();
  }

  mutationStarted(): void {
    this.mutations += 1;
    this.generation += 1;
    this.notify();
  }

  mutationSettled(intent: RefreshIntent): void {
    this.mutations = Math.max(0, this.mutations - 1);
    this.request(intent);
  }

  dispose(): void {
    this.disposed = true;
    this.generation += 1;
    this.pending = null;
    this.notify();
    this.resolveWaiters();
  }

  private resolveWaiters(): void {
    this.resolveIdle?.();
    this.resolveIdle = null;
    this.idlePromise = null;
  }

  private async drain(): Promise<void> {
    if (this.running || this.disposed || this.mutations > 0 || !this.pending || (this.paused && !this.pending.manual)) return;
    this.running = true;
    try {
      while (this.pending && !this.disposed && this.mutations === 0 && (!this.paused || this.pending.manual)) {
        const intent = this.pending;
        this.pending = null;
        const generation = this.generation;
        // The owner reports read errors and retains prior data. A failed request
        // must still release the queue so recovery can run.
        try {
          await this.run(intent, () => !this.disposed && this.generation === generation);
        } catch {
          // Transport errors are published by the owner, not as unhandled tasks.
        }
      }
    } finally {
      this.running = false;
      this.notify();
      if (!this.busy) this.resolveWaiters();
    }
  }
}
