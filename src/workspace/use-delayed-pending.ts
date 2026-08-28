import { useEffect, useRef, useState } from "react";

export interface DelayedPendingOptions {
  /** How long an operation must be pending before an indicator is shown. */
  readonly delay?: number;
  /** Once shown, how long the indicator stays visible so it does not blink. */
  readonly minimum?: number;
}

/**
 * Turns a raw "pending" flag into one suitable for driving a loading
 * indicator: fast operations never show it, and once it does appear it stays
 * for a minimum duration instead of flashing.
 */
export function useDelayedPending(pending: boolean, { delay = 150, minimum = 300 }: DelayedPendingOptions = {}): boolean {
  const [visible, setVisible] = useState(false);
  const shownAt = useRef<number | null>(null);

  useEffect(() => {
    if (pending) {
      if (visible) return;
      const timer = setTimeout(() => {
        shownAt.current = Date.now();
        setVisible(true);
      }, delay);
      return () => clearTimeout(timer);
    }
    if (!visible) return;
    const remaining = Math.max(0, minimum - (Date.now() - (shownAt.current ?? 0)));
    const timer = setTimeout(() => {
      shownAt.current = null;
      setVisible(false);
    }, remaining);
    return () => clearTimeout(timer);
  }, [pending, visible, delay, minimum]);

  return visible;
}
