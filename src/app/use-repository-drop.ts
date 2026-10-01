import { useEffect, useEffectEvent, useRef, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWebview, type DragDropEvent } from "@tauri-apps/api/webview";
import { toast } from "../components/ui/toast";
import { toMessage } from "../lib/errors";

/** Native file drops are webview-wide; accept them only over the visible target. */
export function useRepositoryDrop(enabled: boolean, onDrop: (paths: string[]) => Promise<unknown>) {
  const ref = useRef<HTMLDivElement>(null);
  const [active, setActive] = useState(false);
  const handleDrop = useEffectEvent(async (paths: string[]) => onDrop(paths));
  const acceptsPosition = useEffectEvent((position: { x: number; y: number }) => {
    const target = ref.current;
    if (!enabled || !target) return false;
    // Tauri supplies physical pixels; DOM bounds and hit testing use CSS pixels.
    const x = position.x / window.devicePixelRatio;
    const y = position.y / window.devicePixelRatio;
    const bounds = target.getBoundingClientRect();
    return x >= bounds.left && x < bounds.right && y >= bounds.top && y < bounds.bottom
      && target.contains(document.elementFromPoint(x, y));
  });

  useEffect(() => {
    if (!enabled || !isTauri()) return;
    let cancelled = false;
    let pending = false;
    let unlisten: (() => void) | undefined;
    const handleEvent = (payload: DragDropEvent) => {
      if (cancelled || pending) return;
      if (payload.type === "leave") {
        setActive(false);
        return;
      }
      const accepted = acceptsPosition(payload.position);
      setActive(payload.type !== "drop" && accepted);
      if (payload.type !== "drop" || !accepted || payload.paths.length === 0) return;
      pending = true;
      void handleDrop(payload.paths).catch((cause: unknown) => {
        toast.add({ type: "error", title: "Could not add dropped repositories", description: toMessage(cause) });
      }).finally(() => { pending = false; });
    };
    void getCurrentWebview().onDragDropEvent((event) => handleEvent(event.payload)).then((dispose) => {
      // Subscription can finish after the target was closed or disabled.
      if (cancelled) dispose();
      else unlisten = dispose;
    }).catch((cause: unknown) => {
      if (!cancelled) toast.add({ type: "error", title: "Could not enable repository drops", description: toMessage(cause) });
    });
    return () => {
      cancelled = true;
      unlisten?.();
      setActive(false);
    };
  }, [enabled]);

  return { ref, active: enabled && active };
}
