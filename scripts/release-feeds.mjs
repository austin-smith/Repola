import { mkdir } from "node:fs/promises";
import path from "node:path";
import { downloadRelease, validateUpdaterManifest, verifyArtifacts } from "./release-artifacts.mjs";
import { githubClient, isMain, publishedReleases, readJson, writeJson } from "./release-utils.mjs";

export async function buildFeeds(client, directory, verificationDirectory) {
  await mkdir(directory, { recursive: true });
  const releases = await client.list("/releases");
  for (const channel of ["stable", "nightly"]) {
    const latest = publishedReleases(releases, channel)[0];
    if (!latest) continue;
    const local = path.join(verificationDirectory, channel);
    const release = await downloadRelease(client, latest, local);
    const signatures = await verifyArtifacts(local, release);
    const manifest = await readJson(path.join(local, "latest.json"));
    validateUpdaterManifest(manifest, release, signatures);
    await writeJson(path.join(directory, `${channel}.json`), { ...manifest, notes: latest.body || manifest.notes });
  }
}

if (isMain(import.meta.url)) await buildFeeds(githubClient(), process.argv[2], process.argv[3]);
