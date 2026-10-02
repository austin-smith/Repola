// @vitest-environment node
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { planRelease } from "./release-metadata.mjs";
import { createUpdaterManifest, expectedArtifacts, releaseAssetNames } from "./release-artifacts.mjs";
import { createDraft, publishRelease } from "./publish-release.mjs";
import { buildFeeds } from "./release-feeds.mjs";
import { readJson, writeJson } from "./release-utils.mjs";

const mocks = vi.hoisted(() => ({ verify: vi.fn(), execute: vi.fn() }));
vi.mock("./release-artifacts.mjs", async (original) => ({ ...await original(), verifyArtifacts: mocks.verify }));
vi.mock("node:child_process", () => ({ execFile: (...args) => mocks.execute(...args) }));

const source = { version: "0.1.0", sha: "a".repeat(40), runId: "123", pubDate: "2026-09-30T09:17:00Z" };
const stable = planRelease({ ...source, sourceRef: "refs/tags/v0.1.0", tag: "v0.1.0" });
const nightly = planRelease({ ...source, sourceRef: "refs/heads/main", runNumber: "10" });
let directory;

function fakeGitHub(release = stable, { published = false, additional = [], missing, corruptManifest = false, buildConclusion = "success", id = 1 } = {}) {
  const githubRelease = { id, tag_name: release.tag, prerelease: release.channel === "nightly", draft: !published, html_url: "https://github.com/austin-smith/Repola/releases/tag/v0.1.0" };
  const signatures = Object.fromEntries(expectedArtifacts(release).map((entry) => [entry.name, "signature"]));
  const manifest = createUpdaterManifest(release, signatures, "Changes");
  if (corruptManifest) manifest.platforms["linux-x86_64"].url = "https://example.com/untrusted";
  const contents = { "release.json": JSON.stringify(release), "latest.json": JSON.stringify(manifest) };
  for (const artifact of expectedArtifacts(release)) {
    contents[artifact.name] = "bytes";
    contents[`${artifact.name}.sig`] = "signature";
    contents[`${artifact.name}.sha256`] = "checksum";
    if (artifact.agent) contents[`${artifact.name}.sigstore.json`] = "{}";
  }
  delete contents[missing];
  const assets = Object.keys(contents).map((name, index) => ({ name, id: id * 100 + index + 1 }));
  const client = {
    request: vi.fn(async (url, options) => {
      if (url === `/releases/tags/${release.tag}`) return githubRelease;
      if (url === `/commits/${release.tag}`) return { sha: release.sha };
      if (url.startsWith("/actions/runs/")) return { path: ".github/workflows/release.yml", head_sha: release.sha, status: "completed", conclusion: buildConclusion };
      if (url.startsWith("/releases/assets/")) return Buffer.from(contents[assets.find((asset) => asset.id === Number(url.split("/").at(-1))).name]);
      if (options?.method === "PATCH") {
        githubRelease.draft = options.body.draft;
        return githubRelease;
      }
      if (url.startsWith("/git/ref/")) return { object: { sha: release.sha } };
      throw new Error(`Unexpected request: ${url}`);
    }),
    list: vi.fn(async (url) => url.endsWith("/assets") ? assets : [githubRelease, ...additional]),
  };
  mocks.verify.mockImplementation(async (_directory, release) => Object.fromEntries(expectedArtifacts(release).map((entry) => [entry.name, "signature"])));
  return { client, githubRelease, signatures, contents };
}

beforeEach(async () => {
  directory = await mkdtemp(path.join(os.tmpdir(), "repola-release-test-"));
  mocks.verify.mockReset();
  mocks.execute.mockReset().mockImplementation((...args) => args.at(-1)(null, "", ""));
  vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: true }));
});

afterEach(async () => {
  vi.unstubAllGlobals();
  await rm(directory, { recursive: true, force: true });
});

describe("verified release publication", () => {
  it("publishes stable only after complete asset, signature, manifest, and build checks", async () => {
    const { client } = fakeGitHub();
    await publishRelease(client, stable.tag, "stable", directory);
    expect(mocks.verify).toHaveBeenCalledOnce();
    expect(client.request).toHaveBeenCalledWith("/releases/1", { method: "PATCH", body: { draft: false, make_latest: "true" } });
    expect(fetch).toHaveBeenCalledTimes(releaseAssetNames(stable).length);
  });

  it("publishes nightly without becoming GitHub's latest stable release", async () => {
    const { client } = fakeGitHub(nightly);
    await publishRelease(client, nightly.tag, "nightly", directory);
    expect(client.request).toHaveBeenCalledWith("/releases/1", { method: "PATCH", body: { draft: false, make_latest: "false" } });
  });

  it.each([
    { options: { missing: "repola-agent-aarch64-unknown-linux-gnu.sig" }, error: /incomplete/ },
    { options: { corruptManifest: true }, error: /unexpected URL/ },
    { options: { buildConclusion: "failure" }, error: /successful build workflow/ },
    { options: { additional: [{ id: 2, tag_name: "v0.2.0", draft: false, prerelease: false }] }, error: /newer release/ },
  ])("refuses publication when validation fails ($error)", async ({ options, error }) => {
    const { client } = fakeGitHub(stable, options);
    await expect(publishRelease(client, stable.tag, "stable", directory)).rejects.toThrow(error);
    expect(client.request.mock.calls.some(([, options]) => options?.method === "PATCH")).toBe(false);
  });

  it("refuses invalid signatures and channel mismatches", async () => {
    const { client } = fakeGitHub();
    mocks.verify.mockRejectedValue(new Error("invalid artifact signature"));
    await expect(publishRelease(client, stable.tag, "stable", directory)).rejects.toThrow(/invalid artifact signature/);
    expect(client.request.mock.calls.some(([, options]) => options?.method === "PATCH")).toBe(false);
    await expect(publishRelease(client, stable.tag, "nightly", directory)).rejects.toThrow(/channel/);
  });

  it("repairs publication without mutating published releases or uploading assets", async () => {
    const { client } = fakeGitHub(stable, { published: true });
    await publishRelease(client, stable.tag, "stable", directory);
    expect(client.request.mock.calls.some(([, options]) => options?.method === "PATCH")).toBe(false);
    expect(mocks.execute).not.toHaveBeenCalled();
  });

  it("never replaces assets in a published release or moves a mismatched tag", async () => {
    await writeJson(path.join(directory, "release.json"), stable);
    const { client } = fakeGitHub(stable, { published: true });
    await expect(createDraft(client, directory)).rejects.toThrow(/cannot be overwritten/);
    const moved = fakeGitHub();
    const request = moved.client.request;
    moved.client.request = vi.fn((url, options) => url.startsWith("/commits/") ? { sha: "b".repeat(40) } : request(url, options));
    await expect(createDraft(moved.client, directory)).rejects.toThrow(/will not be moved/);
    expect(mocks.execute).not.toHaveBeenCalled();
  });

  it("resumes only the matching draft and uploads the exact expected asset set", async () => {
    const { client, contents } = fakeGitHub();
    await writeJson(path.join(directory, "release.json"), stable);
    await writeJson(path.join(directory, "latest.json"), JSON.parse(contents["latest.json"]));
    await createDraft(client, directory);
    const [command, args] = mocks.execute.mock.calls[0];
    expect(command).toBe("gh");
    expect(args.slice(0, 3)).toEqual(["release", "upload", stable.tag]);
    expect(args.filter((value) => value.startsWith(directory))).toHaveLength(releaseAssetNames(stable).length);
    expect(args.at(-1)).toBe("--clobber");
  });
});

describe("channel feed reconstruction", () => {
  it("writes a verified published feed and excludes an unpublished channel", async () => {
    const { client } = fakeGitHub(stable, { published: true });
    await buildFeeds(client, path.join(directory, "updates"), path.join(directory, "verified"));
    expect((await readJson(path.join(directory, "updates/stable.json"))).version).toBe(stable.version);
    await expect(readJson(path.join(directory, "updates/nightly.json"))).rejects.toThrow();
  });

  it("preserves both channels in one deployment and rechecks the other channel's signatures", async () => {
    const a = fakeGitHub(stable, { published: true, id: 1 });
    const b = fakeGitHub(nightly, { published: true, id: 2 });
    const client = {
      request: (url, options) => (Number(url.split("/").at(-1)) < 200 ? a.client : b.client).request(url, options),
      list: (url) => url === "/releases" ? [a.githubRelease, b.githubRelease] : (url.includes("/releases/1/") ? a.client : b.client).list(url),
    };
    await buildFeeds(client, path.join(directory, "updates"), path.join(directory, "verified"));
    expect((await readJson(path.join(directory, "updates/stable.json"))).version).toBe(stable.version);
    expect((await readJson(path.join(directory, "updates/nightly.json"))).version).toBe(nightly.version);
    expect(mocks.verify).toHaveBeenCalledTimes(2);
  });
});
