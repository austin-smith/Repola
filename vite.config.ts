import path from "node:path";
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

/// <reference types="vitest/config" />

const host = process.env.TAURI_DEV_HOST;
const STARTUP_JAVASCRIPT_BUDGET_BYTES = 768 * 1024;
const STARTUP_CSS_BUDGET_BYTES = 128 * 1024;

function startupBudget(): Plugin {
  return {
    name: "repola-startup-budget",
    generateBundle(_options, bundle) {
      const visited = new Set<string>();

      const staticJavaScriptSize = (fileName: string): number => {
        if (visited.has(fileName)) return 0;
        visited.add(fileName);

        const output = bundle[fileName];
        if (!output || output.type !== "chunk") return 0;
        return output.code.length + output.imports.reduce((size, importedFile) => size + staticJavaScriptSize(importedFile), 0);
      };

      const startupJavaScriptBytes = Object.values(bundle)
        .filter((output) => output.type === "chunk" && output.isEntry)
        .reduce((size, output) => size + staticJavaScriptSize(output.fileName), 0);
      const startupCssBytes = Object.values(bundle)
        .filter((output) => output.type === "asset" && output.fileName.endsWith(".css"))
        .reduce(
          (size, output) =>
            size + (typeof output.source === "string" ? Buffer.byteLength(output.source) : output.source.byteLength),
          0,
        );

      if (startupJavaScriptBytes > STARTUP_JAVASCRIPT_BUDGET_BYTES) {
        throw new Error(
          `Startup JavaScript is ${startupJavaScriptBytes} bytes; the Repola budget is ${STARTUP_JAVASCRIPT_BUDGET_BYTES} bytes.`,
        );
      }
      if (startupCssBytes > STARTUP_CSS_BUDGET_BYTES) {
        throw new Error(`Startup CSS is ${startupCssBytes} bytes; the Repola budget is ${STARTUP_CSS_BUDGET_BYTES} bytes.`);
      }
    },
  };
}

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react(), tailwindcss(), startupBudget()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: "./src/test-setup.ts",
  },
  build: {
    // Shiki grammars are emitted as independently loaded chunks. The largest
    // bundled grammar is just under 800 KiB; startup assets are protected by
    // the stricter transitive budget above.
    chunkSizeWarningLimit: 820,
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (!id.includes("node_modules")) return undefined;
          const dependency = id.slice(id.lastIndexOf("/node_modules/") + "/node_modules/".length);
          if (/^(react|react-dom|scheduler)(\/|$)/.test(dependency)) {
            return "react-runtime";
          }
          if (dependency.startsWith("@base-ui/")) return "ui-primitives";
          if (dependency.startsWith("@tauri-apps/")) return "tauri-runtime";
          return undefined;
        },
      },
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
