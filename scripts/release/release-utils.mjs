import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";

export const repository = "austin-smith/Repola";
export const updateBaseUrl = "https://austin-smith.github.io/Repola/updates";
export const targets = [
  { target: "aarch64-apple-darwin", platform: "darwin-aarch64", arch: "arm64", installers: [".dmg"], updater: ".app.tar.gz" },
  { target: "x86_64-unknown-linux-gnu", platform: "linux-x86_64", arch: "x64", installers: [".deb", ".AppImage"], updater: ".AppImage", updaterVariants: { "linux-x86_64-deb": ".deb" } },
  { target: "x86_64-pc-windows-msvc", platform: "windows-x86_64", arch: "x64", installers: ["-setup.exe"], updater: "-setup.exe" },
  { target: "aarch64-unknown-linux-gnu" },
];

export function parseVersion(version) {
  const match = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-nightly\.([1-9]\d*))?$/.exec(version);
  if (!match || match[0] !== version) throw new Error(`Unsupported release version: ${version}.`);
  return { core: match.slice(1, 4).map(BigInt), nightly: match[4] ? BigInt(match[4]) : null };
}

export function compareVersions(left, right) {
  const a = parseVersion(left);
  const b = parseVersion(right);
  for (let index = 0; index < 3; index++) {
    if (a.core[index] !== b.core[index]) return a.core[index] > b.core[index] ? 1 : -1;
  }
  if (a.nightly === b.nightly) return 0;
  if (a.nightly === null) return 1;
  if (b.nightly === null) return -1;
  return a.nightly > b.nightly ? 1 : -1;
}

export function validateRelease(release) {
  if (release.schema !== 1 || release.repository !== repository || !/^[a-f0-9]{40}$/.test(release.sha)) {
    throw new Error("Invalid release repository, schema, or source commit.");
  }
  const version = parseVersion(release.version);
  const channel = version.nightly === null ? "stable" : "nightly";
  if (release.channel !== channel || release.tag !== `v${release.version}`) {
    throw new Error("Release channel and tag must match the application version.");
  }
  if (!/^\d+$/.test(release.runId) || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/.test(release.pubDate)
    || !Number.isFinite(Date.parse(release.pubDate))) {
    throw new Error("Invalid release run or publication date.");
  }
  const allowedRefs = channel === "nightly" ? ["refs/heads/main"] : [`refs/tags/${release.tag}`];
  if (!allowedRefs.includes(release.sourceRef)) throw new Error("Release workflow must run from main or its stable tag.");
  return release;
}

export async function readJson(path) {
  return JSON.parse(await readFile(path, "utf8"));
}

export async function writeJson(path, value) {
  await writeFile(path, `${JSON.stringify(value, null, 2)}\n`);
}

export function isMain(importMetaUrl) {
  return process.argv[1] && pathToFileURL(process.argv[1]).href === importMetaUrl;
}

export const root = fileURLToPath(new URL("../../", import.meta.url));

export function githubClient(token = process.env.GH_TOKEN, fetcher = fetch) {
  if (!token) throw new Error("GitHub access requires GH_TOKEN.");
  const request = async (path, { method = "GET", body, missing = false, binary = false } = {}) => {
    const response = await fetcher(`https://api.github.com/repos/${repository}${path}`, {
      method,
      headers: {
        Authorization: `Bearer ${token}`,
        Accept: binary ? "application/octet-stream" : "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        ...(body === undefined ? {} : { "Content-Type": "application/json" }),
      },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      signal: AbortSignal.timeout(120_000),
    });
    if (missing && response.status === 404) return null;
    if (!response.ok) throw new Error(`GitHub ${method} ${path} failed (${response.status}).`);
    if (binary) return Buffer.from(await response.arrayBuffer());
    return response.status === 204 ? null : response.json();
  };
  const list = async (path) => {
    const entries = [];
    for (let page = 1; ; page++) {
      const batch = await request(`${path}${path.includes("?") ? "&" : "?"}per_page=100&page=${page}`);
      entries.push(...batch);
      if (batch.length < 100) return entries;
    }
  };
  return { request, list };
}

export function publishedReleases(releases, channel) {
  return releases.filter((release) => {
    if (release.draft || release.prerelease !== (channel === "nightly")) return false;
    try {
      const version = parseVersion(release.tag_name.slice(1));
      return release.tag_name.startsWith("v") && (version.nightly !== null) === (channel === "nightly");
    } catch {
      return false;
    }
  }).sort((a, b) => compareVersions(b.tag_name.slice(1), a.tag_name.slice(1)));
}

export async function releaseAsset(client, release, name) {
  const assets = await client.list(`/releases/${release.id}/assets`);
  const asset = assets.find((entry) => entry.name === name);
  if (!asset) throw new Error(`Release ${release.tag_name} is missing ${name}.`);
  return client.request(`/releases/assets/${asset.id}`, { binary: true });
}
