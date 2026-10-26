import { buildIdentities } from "../src/domain/build-identity.ts";

export function buildIdentityConfig(channel, tauriConfig) {
  const identity = Object.hasOwn(buildIdentities, channel) ? buildIdentities[channel] : null;
  if (!identity) throw new Error(`Unknown Repola build channel: ${channel}`);
  const config = {
    productName: identity.name,
    mainBinaryName: identity.binaryName,
    identifier: identity.identifier,
    bundle: {
      icon: ["32x32.png", "128x128.png", "128x128@2x.png", "icon.icns", "icon.ico"].map((name) => `${identity.iconDirectory}/${name}`),
    },
  };
  // Tauri replaces arrays instead of merging their elements. Preserve all window settings.
  if (tauriConfig.app?.windows) config.app = {
    windows: tauriConfig.app.windows.map((window) => ({ ...window, title: identity.name })),
  };
  return config;
}
