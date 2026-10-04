import { useCallback, useEffect, useSyncExternalStore } from "react";
import { toast } from "@/components/ui/toast";
import { toMessage } from "@/lib/errors";
import { loadAppPreferences, updateAppPreferences } from "../ipc/app-preferences";
import type { DiffPreferences } from "../ipc/types";

/** The views that remember their own diff presentation. */
export type DiffPreferenceView = "changes" | "history";

const defaultDiffPreferences: DiffPreferences = {
  hideWhitespaceInChanges: false,
  hideWhitespaceInHistory: false,
};

const whitespaceKeys = {
  changes: "hideWhitespaceInChanges",
  history: "hideWhitespaceInHistory",
} as const satisfies Record<DiffPreferenceView, keyof DiffPreferences>;

// One copy of the stored choices is shared by every diff view, so Changes and
// History never disagree and a remount needs no second load.
let current: DiffPreferences | null = null;
let loading = false;
const listeners = new Set<() => void>();

function publish(next: DiffPreferences) {
  current = next;
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

function snapshot() {
  return current;
}

function ensureLoaded() {
  if (current !== null || loading) return;
  loading = true;
  void loadAppPreferences()
    .then((preferences) => publish(preferences.diff))
    // Presentation must never block reviewing a diff; fall back to the
    // exact diff when the stored choice cannot be read.
    .catch(() => publish(defaultDiffPreferences))
    .finally(() => { loading = false; });
}

/**
 * Whether a view hides whitespace changes, or null until the stored choice
 * has loaded (so a diff is never fetched in one mode and then refetched in
 * the other). Changes apply immediately and roll back if they cannot be
 * saved.
 */
export function useHideWhitespace(view: DiffPreferenceView): [boolean | null, (hide: boolean) => void] {
  const preferences = useSyncExternalStore(subscribe, snapshot);
  useEffect(ensureLoaded, []);
  const key = whitespaceKeys[view];
  const setHideWhitespace = useCallback((hide: boolean) => {
    const previous = current ?? defaultDiffPreferences;
    if (previous[key] === hide) return;
    publish({ ...previous, [key]: hide });
    // Saves are serialized in call order, so the shown value is already the
    // one that will end up stored; only a failure needs to change it back.
    void updateAppPreferences((stored) => ({ ...stored, diff: { ...stored.diff, [key]: hide } }))
      .catch((cause: unknown) => {
        if (current !== null && current[key] === hide) publish({ ...current, [key]: !hide });
        toast.add({ type: "error", title: "Could not save the whitespace setting", description: toMessage(cause) });
      });
  }, [key]);
  return [preferences === null ? null : preferences[key], setHideWhitespace];
}
