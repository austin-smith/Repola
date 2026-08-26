import type { PointerEvent as ReactPointerEvent } from "react";

export function PaneResizeHandle({ side, value, minimum, maximum, onChange }: { side: "left" | "right"; value: number; minimum: number; maximum: number; onChange: (value: number) => void }) {
  const clamp = (next: number) => Math.max(minimum, Math.min(maximum, Math.round(next)));
  const beginResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    event.preventDefault();
    const startX = event.clientX;
    const startValue = value;
    const move = (moveEvent: PointerEvent) => onChange(clamp(startValue + (moveEvent.clientX - startX) * (side === "left" ? 1 : -1)));
    const finish = () => {
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", finish);
    };
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", finish, { once: true });
  };
  return (
    <div role="separator" tabIndex={0} aria-label={`Resize ${side} pane`} aria-orientation="vertical" aria-valuemin={minimum} aria-valuemax={maximum} aria-valuenow={value} className="group relative z-10 cursor-col-resize bg-border/35 outline-none hover:bg-brand/30 focus-visible:bg-brand/40" onPointerDown={beginResize} onKeyDown={(event) => {
      if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
      event.preventDefault();
      const direction = event.key === "ArrowRight" ? 1 : -1;
      onChange(clamp(value + direction * 10 * (side === "left" ? 1 : -1)));
    }}>
      <span className="absolute inset-y-0 left-1/2 w-px -translate-x-1/2 bg-border group-hover:bg-brand" aria-hidden="true" />
    </div>
  );
}
