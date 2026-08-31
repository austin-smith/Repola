import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from "react";
import { homeDir, sep } from "@tauri-apps/api/path";
import { shortPath as abbreviatePath, type PathDisplay } from "../domain/format";

/**
 * Host facts the UI must not guess: the user's home directory and the path
 * separator. Both come from Tauri's path API, which answers for the OS the
 * app is actually running on.
 */
const EnvironmentContext = createContext<PathDisplay>({ homeDir: null, separator: "/" });

async function resolvePathDisplay(): Promise<PathDisplay> {
  const separator = sep();
  try {
    return { homeDir: await homeDir(), separator };
  } catch {
    // Outside Tauri (unit tests) or if the lookup fails, show full paths.
    return { homeDir: null, separator };
  }
}

export function EnvironmentProvider({ children }: { children: ReactNode }) {
  const [display, setDisplay] = useState<PathDisplay | null>(null);

  useEffect(() => {
    let cancelled = false;
    void resolvePathDisplay().then((value) => {
      if (!cancelled) setDisplay(value);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  if (!display) return null;
  return <EnvironmentContext.Provider value={display}>{children}</EnvironmentContext.Provider>;
}

/** Returns a stable `shortPath(path)` bound to the host's home directory and separator. */
export function useShortPath(): (path: string) => string {
  const display = useContext(EnvironmentContext);
  return useCallback((path: string) => abbreviatePath(path, display), [display]);
}

/** The host's path separator, for building absolute paths from Git's forward-slash relative paths. */
export function usePathSeparator(): string {
  return useContext(EnvironmentContext).separator;
}
