import path from "node:path";
import { readFileSync } from "node:fs";
import { loadEnv, type Plugin } from "vite";
import { defineConfig, type ViteUserConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { buildIdentities, resolveBuildChannel } from "./src/domain/build-identity.ts";

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
            size + (output.type === "asset" ? (typeof output.source === "string" ? Buffer.byteLength(output.source) : output.source.byteLength) : 0),
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
export default defineConfig(async ({ mode }) => {
  const environment = loadEnv(mode, process.cwd(), "VITE_REPOLA_");
  const baseConfig = JSON.parse(readFileSync(new URL("./src-tauri/tauri.conf.json", import.meta.url), "utf8"));
  const nativeConfig = process.env.TAURI_CONFIG ? JSON.parse(process.env.TAURI_CONFIG) : null;
  const identifier = process.env.TAURI_ENV_PLATFORM ? nativeConfig?.identifier ?? baseConfig.identifier : undefined;
  const channel = resolveBuildChannel(identifier, environment.VITE_REPOLA_RELEASE_CHANNEL);
  const identity = buildIdentities[channel];
  return {
    plugins: [react(), tailwindcss(), {
      name: "repola-build-identity",
      transformIndexHtml(html) {
        return html.replace(/%REPOLA_BUILD_CHANNEL%/g, channel).replace(/%REPOLA_APPLICATION_NAME%/g, identity.name);
      },
    }, startupBudget()],
    define: {
      "import.meta.env.VITE_REPOLA_RELEASE_CHANNEL": JSON.stringify(channel),
    },
    resolve: {
      alias: {
        "@": path.resolve(import.meta.dirname, "./src"),
      },
    },
    test: {
      environment: "jsdom",
      setupFiles: "./src/test-setup.ts",
      // The first render in each test file pays jsdom and React warm-up (1-2.5s
      // idle), which exceeded the 5s default under heavy CPU load. This is only
      // a hang detector; tests must not depend on wall-clock timing.
      testTimeout: 20_000,
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
            // Keep image-only slider code in the lazily loaded diff rather than startup UI.
            if (dependency.startsWith("@base-ui/react/slider/")) return undefined;
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
  } satisfies ViteUserConfig;
});
