import { invoke } from "@tauri-apps/api/core";
import { availableMonitors, getCurrentWindow, PhysicalPosition, PhysicalSize } from "@tauri-apps/api/window";
import type { WindowState } from "../ipc/types";

function intersectsMonitor(state: WindowState, monitors: Awaited<ReturnType<typeof availableMonitors>>): boolean {
  return monitors.some((monitor) => {
    const left = monitor.position.x;
    const top = monitor.position.y;
    const right = left + monitor.size.width;
    const bottom = top + monitor.size.height;
    return state.x < right - 80 && state.y < bottom - 80 && state.x + state.width > left + 80 && state.y + state.height > top + 80;
  });
}

export async function restoreAndTrackWindow(): Promise<() => void> {
  const window = getCurrentWindow();
  const state = await invoke<WindowState | null>("load_window_state");
  if (state) {
    await window.setSize(new PhysicalSize(state.width, state.height));
    if (intersectsMonitor(state, await availableMonitors())) {
      await window.setPosition(new PhysicalPosition(state.x, state.y));
    }
    if (state.maximized) await window.maximize();
  }

  let timeout: ReturnType<typeof setTimeout> | null = null;
  const persist = () => {
    if (timeout) clearTimeout(timeout);
    timeout = setTimeout(() => {
      void Promise.all([window.outerPosition(), window.outerSize(), window.isMaximized()]).then(([position, size, maximized]) => invoke("save_window_state", {
        state: { version: 1, x: position.x, y: position.y, width: size.width, height: size.height, maximized },
      })).catch(() => undefined);
    }, 400);
  };
  const unlistenMoved = await window.onMoved(persist);
  const unlistenResized = await window.onResized(persist);
  return () => {
    if (timeout) clearTimeout(timeout);
    unlistenMoved();
    unlistenResized();
  };
}
