export type ReleaseChannel = "stable" | "nightly" | "development";

const stableVersion = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const nightlyVersion = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)-nightly\.([1-9]\d{7})\.[1-9]\d*$/;

/** Reject offers from another channel even if the update feed is misconfigured. */
export function acceptsReleaseVersion(channel: ReleaseChannel, version: string): boolean {
  if (channel === "development") return false;
  const pattern = channel === "nightly" ? nightlyVersion : stableVersion;
  const match = pattern.exec(version);
  if (match?.[0] !== version) return false;
  if (channel === "nightly") {
    const date = `${match[4].slice(0, 4)}-${match[4].slice(4, 6)}-${match[4].slice(6, 8)}`;
    const timestamp = Date.parse(`${date}T00:00:00Z`);
    return Number.isFinite(timestamp) && new Date(timestamp).toISOString().slice(0, 10) === date;
  }
  return true;
}
