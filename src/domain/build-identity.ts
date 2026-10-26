import identities from "./build-identities.json" with { type: "json" };
import type { ReleaseChannel } from "./release-channel.ts";

export const buildIdentities = identities;

export function channelForIdentifier(identifier: string): ReleaseChannel {
  for (const channel of ["stable", "nightly", "development"] as const) {
    if (identities[channel].identifier === identifier) return channel;
  }
  throw new Error(`Unknown Repola application identifier: ${identifier}`);
}

/** The native identity is authoritative; a mismatched frontend must never ship. */
export function resolveBuildChannel(identifier: string | undefined, channel: string | undefined): ReleaseChannel {
  if (channel !== undefined && channel !== "stable" && channel !== "nightly" && channel !== "development") {
    throw new Error(`Unknown Repola release channel: ${channel}`);
  }
  const nativeChannel = identifier === undefined ? undefined : channelForIdentifier(identifier);
  if (nativeChannel !== undefined && channel !== undefined && nativeChannel !== channel) {
    throw new Error(`Repola frontend channel ${channel} does not match native channel ${nativeChannel}.`);
  }
  return nativeChannel ?? channel ?? "development";
}
