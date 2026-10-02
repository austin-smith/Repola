import { appendFile, mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { githubClient, isMain, parseVersion, publishedReleases, readJson, releaseAsset, repository, root, validateRelease, writeJson, compareVersions } from "./release-utils.mjs";

export function planRelease({ version, sha, runId, runNumber, pubDate, sourceRef, tag }) {
  if (parseVersion(version).nightly !== null) throw new Error("Source manifests must contain the upcoming stable version.");
  const channel = tag ? "stable" : "nightly";
  if (tag && tag !== `v${version}`) throw new Error("Stable tag must match the source version.");
  if (!tag && !/^[1-9]\d*$/.test(runNumber)) throw new Error("Nightly releases require a positive run number.");
  const releaseVersion = tag ? version : `${version}-nightly.${runNumber}`;
  return validateRelease({ schema: 1, repository, channel, version: releaseVersion, tag: `v${releaseVersion}`, sha, runId, pubDate, sourceRef });
}

export function stampVersions({ packageJson, tauriConfig, cargoManifest, cargoLock }, release) {
  validateRelease(release);
  const sourceVersion = packageJson.version;
  const workspace = /^\[workspace\.package\]\s*\n([\s\S]*?)(?=^\[|(?![\s\S]))/m.exec(cargoManifest);
  if (!workspace || workspace[1].match(/^version\s*=\s*"([^"]+)"/m)?.[1] !== sourceVersion || tauriConfig.version !== sourceVersion) {
    throw new Error("Package, Cargo workspace, and Tauri source versions must agree.");
  }
  const baseVersion = release.version.split("-")[0];
  if (sourceVersion !== baseVersion && sourceVersion !== release.version) throw new Error("Release version does not match its source manifests.");
  const stampedCargo = cargoManifest.replace(workspace[0], workspace[0].replace(/^version\s*=\s*"[^"]+"/m, `version = "${release.version}"`));
  let matched = 0;
  const stampedLock = cargoLock.replace(/(\[\[package\]\]\s*\nname = "(repola|repola-engine)"\s*\nversion = ")[^"]+("\s*\n)/g, (_match, before, _name, after) => {
    matched++;
    return `${before}${release.version}${after}`;
  });
  if (matched !== 2) throw new Error("Cargo.lock must contain both Repola workspace packages.");
  return {
    packageJson: { ...packageJson, version: release.version },
    tauriConfig: { ...tauriConfig, version: release.version },
    cargoManifest: stampedCargo,
    cargoLock: stampedLock,
  };
}

export async function resolveMetadata(environment = process.env, client = githubClient()) {
  if (environment.GITHUB_REPOSITORY?.toLowerCase() !== repository.toLowerCase()) throw new Error("Release workflow is restricted to the Repola repository.");
  const stableTag = environment.GITHUB_REF_TYPE === "tag" ? environment.GITHUB_REF_NAME : null;
  if (!stableTag && environment.GITHUB_REF !== "refs/heads/main") throw new Error("Nightly builds must run from main.");
  if (environment.GITHUB_REF_TYPE !== "tag" && environment.GITHUB_REF !== "refs/heads/main") throw new Error("Manual release workflows must run from main.");
  if (stableTag && !/^v\d+\.\d+\.\d+$/.test(stableTag)) throw new Error("Stable release requires an exact vX.Y.Z tag.");
  const source = await client.request(`/commits/${encodeURIComponent(stableTag || environment.GITHUB_SHA)}`);
  const comparison = await client.request(`/compare/${source.sha}...main`);
  if (!["identical", "ahead"].includes(comparison.status)) throw new Error("Release commit must belong to main.");
  const runs = await client.request(`/actions/workflows/ci.yml/runs?head_sha=${source.sha}&event=push&per_page=100`);
  if (!runs.workflow_runs.some((run) => run.head_sha === source.sha && run.head_branch === "main" && run.status === "completed" && run.conclusion === "success")) {
    throw new Error("Release source requires successful three-platform CI at its exact commit.");
  }
  const manifest = await client.request(`/contents/package.json?ref=${source.sha}`);
  const version = JSON.parse(Buffer.from(manifest.content, "base64").toString("utf8")).version;
  const run = await client.request(`/actions/runs/${environment.GITHUB_RUN_ID}`);
  const release = planRelease({ version, sha: source.sha, tag: stableTag, runId: environment.GITHUB_RUN_ID, runNumber: environment.GITHUB_RUN_NUMBER, pubDate: run.created_at, sourceRef: environment.GITHUB_REF });
  const releases = await client.list("/releases");
  const latestStable = publishedReleases(releases, "stable")[0];
  if (release.channel === "nightly" && latestStable && compareVersions(version, latestStable.tag_name.slice(1)) <= 0) {
    throw new Error("Bump main's upcoming stable version before building another nightly.");
  }
  const existing = releases.find((entry) => entry.tag_name === release.tag);
  if (existing && !existing.draft) throw new Error("Published releases cannot be rebuilt or overwritten. Rerun publication to repair the feed.");
  const latest = publishedReleases(releases, release.channel)[0];
  if (release.channel === "nightly" && latest) {
    const previous = JSON.parse((await releaseAsset(client, latest, "release.json")).toString("utf8"));
    validateRelease(previous);
    if (previous.tag !== latest.tag_name || previous.channel !== "nightly") throw new Error("Published nightly metadata does not match its release.");
    if (previous.sha === source.sha) return null;
  }
  return release;
}

export async function stampRelease(metadataPath) {
  const release = validateRelease(await readJson(metadataPath));
  const files = { packageJson: "package.json", tauriConfig: "src-tauri/tauri.conf.json", cargoManifest: "src-tauri/Cargo.toml", cargoLock: "src-tauri/Cargo.lock" };
  const input = {};
  for (const [key, file] of Object.entries(files)) {
    const content = await readFile(path.join(root, file), "utf8");
    input[key] = key.endsWith("Json") || key === "tauriConfig" ? JSON.parse(content) : content;
  }
  const stamped = stampVersions(input, release);
  for (const [key, file] of Object.entries(files)) {
    if (typeof stamped[key] === "string") await writeFile(path.join(root, file), stamped[key]);
    else await writeJson(path.join(root, file), stamped[key]);
  }
}

if (isMain(import.meta.url)) {
  if (process.argv[2] === "stamp") {
    await stampRelease(process.argv[3]);
  } else if (process.argv[2] === "resolve") {
    const release = await resolveMetadata();
    if (release) {
      const metadataPath = process.argv[3] ?? path.join(root, ".release", "release.json");
      await mkdir(path.dirname(metadataPath), { recursive: true });
      await writeJson(metadataPath, release);
    }
    const outputs = release ? { ...release, skip: "false" } : { skip: "true" };
    for (const [key, value] of Object.entries(outputs)) await appendFile(process.env.GITHUB_OUTPUT, `${key}=${value}\n`);
  } else {
    throw new Error("Usage: release-metadata.mjs resolve [output.json] | stamp <release.json>");
  }
}
