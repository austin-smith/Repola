import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";
import { assetName } from "./stage-release.mjs";
import { isMain, readJson, repository, root, targets, validateRelease, writeJson } from "./release-utils.mjs";

const execute = promisify(execFile);

function updaterEntries(release) {
  return targets.filter((target) => target.platform).flatMap((target) =>
    Object.entries({ [target.platform]: target.updater, ...target.updaterVariants })
      .map(([platform, extension]) => ({ platform, name: assetName(release.version, target.target, extension) })),
  );
}

export function expectedArtifacts(release) {
  validateRelease(release);
  return targets.flatMap((target) => {
    const agent = `repola-agent-${target.target}${target.target.includes("windows") ? ".exe" : ""}`;
    const bundles = target.platform ? [...new Set([target.installer, target.updater])].map((extension) => assetName(release.version, target.target, extension)) : [];
    return [{ name: agent, agent: true }, ...bundles.map((name) => ({ name, agent: false }))];
  });
}

export function releaseAssetNames(release) {
  return ["release.json", "latest.json", ...expectedArtifacts(release).flatMap((artifact) => [
    artifact.name, `${artifact.name}.sha256`, `${artifact.name}.sig`, ...(artifact.agent ? [`${artifact.name}.sigstore.json`] : []),
  ])];
}

export function validateChecksum(bytes, checksum, name) {
  const digest = createHash("sha256").update(bytes).digest("hex");
  if (checksum.trim() !== `${digest}  ${name}`) throw new Error(`Checksum failed for ${name}.`);
}

export function createUpdaterManifest(release, signatures, notes) {
  validateRelease(release);
  const platforms = {};
  for (const { platform, name } of updaterEntries(release)) {
    const signature = signatures[name]?.trim();
    if (!signature) throw new Error(`Missing updater signature for ${platform}.`);
    platforms[platform] = {
      signature,
      url: `https://github.com/${repository}/releases/download/${release.tag}/${encodeURIComponent(name)}`,
    };
  }
  return { version: release.version, notes, pub_date: release.pubDate, platforms };
}

export function validateUpdaterManifest(manifest, release, signatures) {
  const expected = createUpdaterManifest(release, signatures, manifest.notes);
  if (typeof manifest.notes !== "string" || manifest.version !== expected.version || manifest.pub_date !== expected.pub_date
    || Object.keys(manifest.platforms ?? {}).length !== Object.keys(expected.platforms).length) {
    throw new Error("Updater manifest does not describe this complete release.");
  }
  for (const [platform, entry] of Object.entries(expected.platforms)) {
    if (manifest.platforms[platform]?.url !== entry.url || manifest.platforms[platform]?.signature !== entry.signature) {
      throw new Error(`Updater manifest has an unexpected URL or signature for ${platform}.`);
    }
  }
}

export async function verifyArtifacts(directory, release, { staged = false } = {}) {
  const artifacts = expectedArtifacts(release);
  const allowed = new Set(["release.json", "latest.json", ...targets.map((entry) => `${entry.target}.json`)]);
  const signatures = {};
  for (const artifact of artifacts) {
    for (const name of [artifact.name, `${artifact.name}.sha256`, `${artifact.name}.sig`, ...(artifact.agent ? [`${artifact.name}.sigstore.json`] : [])]) allowed.add(name);
    const bytes = await readFile(path.join(directory, artifact.name));
    if (!bytes.length) throw new Error(`Empty release artifact: ${artifact.name}.`);
    if (artifact.agent && bytes.length > 64 * 1024 * 1024) throw new Error(`Agent exceeds bootstrap's download limit: ${artifact.name}.`);
    validateChecksum(bytes, await readFile(path.join(directory, `${artifact.name}.sha256`), "utf8"), artifact.name);
    signatures[artifact.name] = await readFile(path.join(directory, `${artifact.name}.sig`), "utf8");
    if (artifact.agent) {
      JSON.parse(await readFile(path.join(directory, `${artifact.name}.sigstore.json`), "utf8"));
    }
  }
  for (const name of await readdir(directory)) {
    if (!allowed.has(name)) throw new Error(`Unexpected release asset: ${name}.`);
  }
  if (staged) {
    for (const target of targets) {
      const metadata = await readJson(path.join(directory, `${target.target}.json`));
      const expected = artifacts.filter((entry) => entry.name.includes(target.target)).map((entry) => entry.name).sort();
      if (metadata.target !== target.target || JSON.stringify(metadata.release) !== JSON.stringify(release)
        || JSON.stringify(metadata.artifacts?.toSorted()) !== JSON.stringify(expected)) {
        throw new Error(`Artifact metadata does not match the release for ${target.target}.`);
      }
    }
  }
  await execute("cargo", ["run", "--locked", "--manifest-path", path.join(root, "src-tauri/Cargo.toml"), "--package", "repola-engine", "--example", "verify-release", "--", release.version, ...artifacts.map((entry) => path.resolve(directory, entry.name))], { timeout: 600_000, maxBuffer: 4 * 1024 * 1024 });
  for (const artifact of artifacts.filter((entry) => entry.agent)) {
    await execute("cosign", ["verify-blob", "--bundle", path.join(directory, `${artifact.name}.sigstore.json`), "--certificate-identity", `https://github.com/${repository}/.github/workflows/release.yml@${release.sourceRef}`, "--certificate-oidc-issuer", "https://token.actions.githubusercontent.com", path.join(directory, artifact.name)], { timeout: 120_000 });
  }
  return signatures;
}

export async function assembleRelease(directory, metadataPath) {
  const release = validateRelease(await readJson(metadataPath));
  const signatures = await verifyArtifacts(directory, release, { staged: true });
  await writeJson(path.join(directory, "release.json"), release);
  await writeJson(path.join(directory, "latest.json"), createUpdaterManifest(release, signatures, `Repola ${release.version}`));
}

async function readReleaseMetadata(client, githubRelease) {
  const assets = await client.list(`/releases/${githubRelease.id}/assets`);
  const metadata = assets.find((asset) => asset.name === "release.json");
  if (!metadata) throw new Error("Release has no verified build metadata.");
  const release = validateRelease(JSON.parse((await client.request(`/releases/assets/${metadata.id}`, { binary: true })).toString("utf8")));
  if (githubRelease.tag_name !== release.tag || githubRelease.prerelease !== (release.channel === "nightly")) throw new Error("GitHub release disagrees with its build metadata.");
  const expected = new Set(releaseAssetNames(release));
  if (assets.length !== expected.size || new Set(assets.map((asset) => asset.name)).size !== expected.size
    || assets.some((asset) => !expected.has(asset.name))) throw new Error("Release assets are incomplete or unexpected.");
  return { release, assets };
}

// Published assets were already verified; clients verify bytes when downloading.
export async function readPublishedManifest(client, githubRelease) {
  if (githubRelease.draft) throw new Error("Update feeds require a published release.");
  const { release, assets } = await readReleaseMetadata(client, githubRelease);
  const readAsset = (name) => client.request(`/releases/assets/${assets.find((asset) => asset.name === name).id}`, { binary: true });
  const manifest = JSON.parse((await readAsset("latest.json")).toString("utf8"));
  const signatures = Object.fromEntries(await Promise.all([...new Set(updaterEntries(release).map(({ name }) => name))].map(async (name) =>
    [name, (await readAsset(`${name}.sig`)).toString("utf8")],
  )));
  validateUpdaterManifest(manifest, release, signatures);
  return { ...manifest, notes: githubRelease.body || manifest.notes };
}

export async function downloadRelease(client, githubRelease, directory) {
  const { release, assets } = await readReleaseMetadata(client, githubRelease);
  await mkdir(directory, { recursive: true });
  for (const asset of assets) await writeFile(path.join(directory, asset.name), await client.request(`/releases/assets/${asset.id}`, { binary: true }));
  return release;
}

if (isMain(import.meta.url)) await assembleRelease(process.argv[2], process.argv[3]);
