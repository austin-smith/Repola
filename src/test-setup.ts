import "@testing-library/jest-dom/vitest";
import { configure } from "@testing-library/react";

// findBy*/waitFor gates wait on real async work (lazy chunks, timers) and return
// as soon as their condition holds. The 1s default expired under heavy CPU load,
// so treat it as a hang detector like testTimeout, which stays larger so a stuck
// gate reports what it was waiting for instead of a bare test timeout.
configure({ asyncUtilTimeout: 10_000 });

// jsdom does not implement matchMedia; the ThemeProvider reads it for the "system" theme.
if (typeof window !== "undefined" && typeof window.matchMedia !== "function") {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: (query: string): MediaQueryList => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => undefined,
      removeListener: () => undefined,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
      dispatchEvent: () => false,
    }),
  });
}
