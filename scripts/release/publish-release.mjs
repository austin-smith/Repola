import { execFile } from "node:child_process";
import { mkdir } from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";
import { downloadRelease, readPublishedManifest, releaseAssetNames, validateUpdaterManifest, verifyArtifacts } from "./release-artifacts.mjs";
import { compareVersions, githubClient, isMain, publishedReleases, readJson, repository, validateRelease, writeJson } from "./release-utils.mjs";

const execute = promisify(execFile);

export async function createDraft(client, directory) {
  const release = validateRelease(await readJson(path.join(directory, "release.json")));
  const tagRef = await client.request(`/git/ref/tags/${encodeURIComponent(release.tag)}`, { missing: true });
  if (!tagRef) await client.request("/git/refs", { method: "POST", body: { ref: `refs/tags/${release.tag}`, sha: release.sha } });
  const tagCommit = await client.request(`/commits/${release.tag}`);
  if (tagCommit.sha !== release.sha) throw new Error("Release tag points at a different commit; it will not be moved.");
  let draft = await client.request(`/releases/tags/${release.tag}`, { missing: true });
  if (draft && !draft.draft) throw new Error("Published release assets cannot be overwritten.");
  if (draft && draft.prerelease !== (release.channel === "nightly")) throw new Error("Existing draft has a different release channel.");
  if (draft) {
    const existing = (await client.list(`/releases/${draft.id}/assets`)).find((asset) => asset.name === "release.json");
    if (existing) {
      const previous = JSON.parse((await client.request(`/releases/assets/${existing.id}`, { binary: true })).toString("utf8"));
      if (JSON.stringify(previous) !== JSON.stringify(release)) throw new Error("Existing draft belongs to a different build. Publish it or use a new version.");
    }
  } else {
    const previous = publishedReleases(await client.list("/releases"), release.channel)[0];
    const notes = await client.request("/releases/generate-notes", { method: "POST", body: { tag_name: release.tag, target_commitish: release.sha, ...(previous ? { previous_tag_name: previous.tag_name } : {}) } });
    draft = await client.request("/releases", { method: "POST", body: { tag_name: release.tag, target_commitish: release.sha, name: `Repola ${release.version}`, body: notes.body, draft: true, prerelease: release.channel === "nightly" } });
  }
  const manifest = await readJson(path.join(directory, "latest.json"));
  await writeJson(path.join(directory, "latest.json"), { ...manifest, notes: draft.body || manifest.notes });
  const names = releaseAssetNames(release);
  await execute("gh", ["release", "upload", release.tag, ...names.map((name) => path.join(directory, name)), "--repo", repository, "--clobber"], { timeout: 600_000, maxBuffer: 1024 * 1024 });
  console.log(`Draft ready: ${draft.html_url}`);
  return draft;
}

export async function publishRelease(client, tag, channel, directory, feedDirectory) {
  if (!tag || !["stable", "nightly"].includes(channel) || !directory || !feedDirectory) {
    throw new Error("Publication requires a tag, stable/nightly channel, artifact directory, and feed directory.");
  }
  const githubRelease = await client.request(`/releases/tags/${encodeURIComponent(tag)}`);
  const release = await downloadRelease(client, githubRelease, directory);
  if (release.channel !== channel) throw new Error("Publication workflow channel does not match the release.");
  const source = await client.request(`/commits/${encodeURIComponent(tag)}`);
  if (source.sha !== release.sha) throw new Error("Release tag no longer matches its verified build commit.");
  const build = await client.request(`/actions/runs/${release.runId}`);
  // Nightly calls this workflow before its parent run completes. Stable drafts
  // must come from a completed successful release run.
  if (build.path !== ".github/workflows/release.yml" || build.head_sha !== release.sha
    || (channel === "stable" && (build.status !== "completed" || build.conclusion !== "success"))) {
    throw new Error("Release was not produced by the expected successful build workflow.");
  }
  const signatures = await verifyArtifacts(directory, release);
  const manifest = await readJson(path.join(directory, "latest.json"));
  validateUpdaterManifest(manifest, release, signatures);
  const releases = await client.list("/releases");
  const latest = publishedReleases(releases, channel)[0];
  if (latest && compareVersions(latest.tag_name.slice(1), release.version) > 0) throw new Error("A newer release is already published; channel regression is refused.");
  if (githubRelease.draft) await client.request(`/releases/${githubRelease.id}`, { method: "PATCH", body: { draft: false, make_latest: channel === "stable" ? "true" : "false" } });
  // Publication precedes feed deployment. A failed deploy can be retried without
  // changing the release or its signed assets.
  for (const name of releaseAssetNames(release)) {
    const url = `https://github.com/${repository}/releases/download/${release.tag}/${encodeURIComponent(name)}`;
    const response = await fetch(url, { method: "HEAD", signal: AbortSignal.timeout(120_000) });
    if (!response.ok) throw new Error(`Published artifact is unavailable: ${name} (${response.status}). Rerun publication after GitHub propagation.`);
  }
  await writeFeeds(client, feedDirectory, release, { ...manifest, notes: githubRelease.body || manifest.notes }, releases);
  return release;
}

export async function writeFeeds(client, directory, verifiedRelease, verifiedManifest, releases) {
  releases ??= await client.list("/releases");
  const otherChannel = verifiedRelease.channel === "stable" ? "nightly" : "stable";
  const latest = publishedReleases(releases, otherChannel)[0];
  const otherManifest = latest ? await readPublishedManifest(client, latest) : null;
  await mkdir(directory, { recursive: true });
  await writeJson(path.join(directory, `${verifiedRelease.channel}.json`), verifiedManifest);
  if (otherManifest) await writeJson(path.join(directory, `${otherChannel}.json`), otherManifest);
}

if (isMain(import.meta.url)) {
  const client = githubClient();
  if (process.argv[2] === "draft") await createDraft(client, process.argv[3]);
  else if (process.argv[2] === "publish") await publishRelease(client, process.argv[3], process.argv[4], process.argv[5], process.argv[6]);
  else throw new Error("Usage: publish-release.mjs draft <directory> | publish <tag> <channel> <artifacts-directory> <feeds-directory>");
}
