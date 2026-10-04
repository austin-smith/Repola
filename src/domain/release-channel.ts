export type ReleaseChannel = "stable" | "nightly" | "development";

const stableVersion = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const nightlyVersion = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)-nightly\.[1-9]\d*$/;

/** Reject offers from another channel even if the update feed is misconfigured. */
export function acceptsReleaseVersion(channel: ReleaseChannel, version: string): boolean {
  if (channel === "development") return true;
  const pattern = channel === "nightly" ? nightlyVersion : stableVersion;
  return pattern.exec(version)?.[0] === version;
}
