import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useDelayedPending } from "./use-delayed-pending";

describe("useDelayedPending", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("never shows an indicator for operations that finish within the delay", () => {
    const { result, rerender } = renderHook(({ pending }) => useDelayedPending(pending, { delay: 150, minimum: 300 }), { initialProps: { pending: true } });
    expect(result.current).toBe(false);
    act(() => { vi.advanceTimersByTime(100); });
    rerender({ pending: false });
    act(() => { vi.advanceTimersByTime(1000); });
    expect(result.current).toBe(false);
  });

  it("shows the indicator after the delay and holds it for the minimum duration", () => {
    const { result, rerender } = renderHook(({ pending }) => useDelayedPending(pending, { delay: 150, minimum: 300 }), { initialProps: { pending: true } });
    act(() => { vi.advanceTimersByTime(150); });
    expect(result.current).toBe(true);

    act(() => { vi.advanceTimersByTime(100); });
    rerender({ pending: false });
    expect(result.current).toBe(true);
    act(() => { vi.advanceTimersByTime(199); });
    expect(result.current).toBe(true);
    act(() => { vi.advanceTimersByTime(1); });
    expect(result.current).toBe(false);
  });

  it("keeps a visible indicator up when a new operation starts before it hides", () => {
    const { result, rerender } = renderHook(({ pending }) => useDelayedPending(pending, { delay: 150, minimum: 300 }), { initialProps: { pending: true } });
    act(() => { vi.advanceTimersByTime(150); });
    rerender({ pending: false });
    rerender({ pending: true });
    act(() => { vi.advanceTimersByTime(1000); });
    expect(result.current).toBe(true);
  });
});
