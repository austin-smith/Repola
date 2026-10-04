import packageJson from "../../package.json";
import type { ReleaseChannel } from "@/domain/release-channel";

const channel = import.meta.env.VITE_REPOLA_RELEASE_CHANNEL;
const version = import.meta.env.VITE_REPOLA_VERSION || packageJson.version;

export const releaseChannel: ReleaseChannel = channel === "stable" || channel === "nightly" ? channel : "development";

export const releaseLabel = releaseChannel === "stable" ? `${version} · Stable`
  : releaseChannel === "nightly" ? `${version} · Nightly`
    : `${version} · Development build`;
