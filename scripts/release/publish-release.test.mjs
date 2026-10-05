// @vitest-environment node
import { mkdir, mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { planRelease } from "./release-metadata.mjs";
import { createUpdaterManifest, expectedArtifacts, releaseAssetNames } from "./release-artifacts.mjs";
import { createDraft, publishRelease, writeFeeds } from "./publish-release.mjs";
import { readJson, writeJson } from "./release-utils.mjs";

const mocks = vi.hoisted(() => ({ verify: vi.fn(), execute: vi.fn() }));
vi.mock("./release-artifacts.mjs", async (original) => ({ ...await original(), verifyArtifacts: mocks.verify }));
vi.mock("node:child_process", () => ({ execFile: (...args) => mocks.execute(...args) }));

const source = { version: "0.1.0", sha: "a".repeat(40), runId: "123", pubDate: "2026-09-30T09:17:00Z" };
const stable = planRelease({ ...source, sourceRef: "refs/tags/v0.1.0", tag: "v0.1.0" });
const nightly = planRelease({ ...source, sourceRef: "refs/heads/main", sequence: "10" });
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
    list: vi.fn(async (url) => url.endsWith("/assets") ? assets : [...additional, githubRelease]),
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
    await publishRelease(client, stable.tag, "stable", directory, path.join(directory, "updates"));
    expect(mocks.verify).toHaveBeenCalledOnce();
    expect(client.request).toHaveBeenCalledWith("/releases/1", { method: "PATCH", body: { draft: false, make_latest: "true" } });
    expect(fetch).toHaveBeenCalledTimes(releaseAssetNames(stable).length);
    expect((await readJson(path.join(directory, "updates/stable.json"))).version).toBe(stable.version);
  });

  it("rejects missing publication arguments before contacting GitHub", async () => {
    const { client } = fakeGitHub();
    await expect(publishRelease(client, stable.tag, "stable", directory)).rejects.toThrow(/feed directory/);
    expect(client.request).not.toHaveBeenCalled();
  });

  it("refuses publication when only a different release tag exists", async () => {
    const { client } = fakeGitHub();
    await expect(publishRelease(client, "v0.1.1", "stable", directory, path.join(directory, "updates"))).rejects.toThrow("Release v0.1.1 was not found.");
    expect(client.request).not.toHaveBeenCalled();
    expect(mocks.verify).not.toHaveBeenCalled();
    expect(fetch).not.toHaveBeenCalled();
  });

  it("publishes nightly without becoming GitHub's latest stable release", async () => {
    const { client } = fakeGitHub(nightly);
    await publishRelease(client, nightly.tag, "nightly", directory, path.join(directory, "updates"));
    expect(client.request).toHaveBeenCalledWith("/releases/1", { method: "PATCH", body: { draft: false, make_latest: "false" } });
  });

  it.each(["v0.1.0", "v0.2.0"])("refuses a nightly draft when stable %s was published during its build", async (tag) => {
    const a = fakeGitHub(nightly, { id: 1 });
    const b = fakeGitHub(planRelease({ ...source, version: tag.slice(1), sourceRef: `refs/tags/${tag}`, tag }), { published: true, id: 2 });
    const client = {
      request: (url, options) => (url.startsWith("/releases/assets/") && Number(url.split("/").at(-1)) >= 200 ? b.client : a.client).request(url, options),
      list: (url) => url === "/releases" ? [a.githubRelease, b.githubRelease] : (url.includes("/releases/2/") ? b.client : a.client).list(url),
    };
    await expect(publishRelease(client, nightly.tag, "nightly", directory, path.join(directory, "updates"))).rejects.toThrow(/before publishing another nightly/);
    expect(a.client.request.mock.calls.some(([, options]) => options?.method === "PATCH")).toBe(false);
    expect(fetch).not.toHaveBeenCalled();
    await expect(readJson(path.join(directory, "updates/nightly.json"))).rejects.toThrow();
  });

  it.each([
    { version: "0.2.0", published: false },
    { version: "0.1.0", published: true },
  ])("allows a newer-base nightly or repairs an already-published nightly ($version, published=$published)", async ({ version, published }) => {
    const release = planRelease({ ...source, version, sourceRef: "refs/heads/main", sequence: "10" });
    const a = fakeGitHub(release, { published, id: 1 });
    const b = fakeGitHub(stable, { published: true, id: 2 });
    const client = {
      request: (url, options) => (url.startsWith("/releases/assets/") && Number(url.split("/").at(-1)) >= 200 ? b.client : a.client).request(url, options),
      list: (url) => url === "/releases" ? [a.githubRelease, b.githubRelease] : (url.includes("/releases/2/") ? b.client : a.client).list(url),
    };
    await publishRelease(client, release.tag, "nightly", directory, path.join(directory, "updates"));
    expect(a.client.request.mock.calls.some(([, options]) => options?.method === "PATCH")).toBe(!published);
    expect((await readJson(path.join(directory, "updates/nightly.json"))).version).toBe(release.version);
    expect((await readJson(path.join(directory, "updates/stable.json"))).version).toBe(stable.version);
  });

  it.each([
    { options: { missing: "repola-agent-aarch64-unknown-linux-gnu.sig" }, error: /incomplete/ },
    { options: { corruptManifest: true }, error: /unexpected URL/ },
    { options: { buildConclusion: "failure" }, error: /successful build workflow/ },
    { options: { additional: [{ id: 2, tag_name: "v0.2.0", draft: false, prerelease: false }] }, error: /newer release/ },
  ])("refuses publication when validation fails ($error)", async ({ options, error }) => {
    const { client } = fakeGitHub(stable, options);
    await expect(publishRelease(client, stable.tag, "stable", directory, path.join(directory, "updates"))).rejects.toThrow(error);
    expect(client.request.mock.calls.some(([, options]) => options?.method === "PATCH")).toBe(false);
  });

  it("refuses invalid signatures and channel mismatches", async () => {
    const { client } = fakeGitHub();
    mocks.verify.mockRejectedValue(new Error("invalid artifact signature"));
    await expect(publishRelease(client, stable.tag, "stable", directory, path.join(directory, "updates"))).rejects.toThrow(/invalid artifact signature/);
    expect(client.request.mock.calls.some(([, options]) => options?.method === "PATCH")).toBe(false);
    await expect(publishRelease(client, stable.tag, "nightly", directory, path.join(directory, "updates"))).rejects.toThrow(/channel/);
  });

  it("repairs publication without mutating published releases or uploading assets", async () => {
    const { client } = fakeGitHub(stable, { published: true });
    await publishRelease(client, stable.tag, "stable", directory, path.join(directory, "updates"));
    expect(client.request.mock.calls.some(([, options]) => options?.method === "PATCH")).toBe(false);
    expect(mocks.execute).not.toHaveBeenCalled();
    expect((await readJson(path.join(directory, "updates/stable.json"))).version).toBe(stable.version);
  });

  it("withholds feeds until every published artifact is available", async () => {
    const { client } = fakeGitHub();
    fetch.mockResolvedValueOnce({ ok: false, status: 404 });
    await expect(publishRelease(client, stable.tag, "stable", directory, path.join(directory, "updates"))).rejects.toThrow(/propagation/);
    await expect(readJson(path.join(directory, "updates/stable.json"))).rejects.toThrow();
  });

  it("rejects duplicated assets that conceal a missing artifact", async () => {
    const { client } = fakeGitHub();
    const assets = await client.list("/releases/1/assets");
    assets[assets.length - 1] = assets[assets.length - 2];
    await expect(publishRelease(client, stable.tag, "stable", directory, path.join(directory, "updates"))).rejects.toThrow(/incomplete/);
    expect(mocks.verify).not.toHaveBeenCalled();
    expect(client.request.mock.calls.some(([, options]) => options?.method === "PATCH")).toBe(false);
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

  it.each([stable, nightly])("resumes the matching $channel draft and uploads the exact expected asset set", async (release) => {
    const { client, contents } = fakeGitHub(release, { additional: [{ id: 2, tag_name: "v0.2.0", draft: true, prerelease: false }] });
    await writeJson(path.join(directory, "release.json"), release);
    await writeJson(path.join(directory, "latest.json"), JSON.parse(contents["latest.json"]));
    const draft = await createDraft(client, directory);
    expect(draft.id).toBe(1);
    const [command, args] = mocks.execute.mock.calls[0];
    expect(command).toBe("gh");
    expect(args.slice(0, 3)).toEqual(["release", "upload", release.tag]);
    expect(args.filter((value) => value.startsWith(directory))).toHaveLength(releaseAssetNames(release).length);
    expect(args.at(-1)).toBe("--clobber");
  });

  it("refuses to resume a draft from a different build", async () => {
    const { client, contents } = fakeGitHub();
    contents["release.json"] = JSON.stringify({ ...stable, runId: "124" });
    await writeJson(path.join(directory, "release.json"), stable);
    await expect(createDraft(client, directory)).rejects.toThrow(/different build/);
    expect(mocks.execute).not.toHaveBeenCalled();
  });
});

describe("channel feed reconstruction", () => {
  it("writes a verified published feed and excludes an unpublished channel", async () => {
    const { client } = fakeGitHub(stable, { published: true });
    await writeFeeds(client, path.join(directory, "updates"), stable, createUpdaterManifest(stable, Object.fromEntries(expectedArtifacts(stable).map((entry) => [entry.name, "signature"])), "Changes"));
    expect((await readJson(path.join(directory, "updates/stable.json"))).version).toBe(stable.version);
    await expect(readJson(path.join(directory, "updates/nightly.json"))).rejects.toThrow();
    expect(client.request).not.toHaveBeenCalled();
  });

  it("preserves both channels using only metadata and updater signatures for the other release", async () => {
    const a = fakeGitHub(stable, { published: true, id: 1 });
    const b = fakeGitHub(nightly, { published: true, id: 2 });
    const client = {
      request: (url, options) => (Number(url.split("/").at(-1)) < 200 ? a.client : b.client).request(url, options),
      list: (url) => url === "/releases" ? [a.githubRelease, b.githubRelease] : (url.includes("/releases/1/") ? a.client : b.client).list(url),
    };
    await writeFeeds(client, path.join(directory, "updates"), stable, createUpdaterManifest(stable, Object.fromEntries(expectedArtifacts(stable).map((entry) => [entry.name, "signature"])), "Changes"));
    expect((await readJson(path.join(directory, "updates/stable.json"))).version).toBe(stable.version);
    expect((await readJson(path.join(directory, "updates/nightly.json"))).version).toBe(nightly.version);
    expect(mocks.verify).not.toHaveBeenCalled();
    expect(a.client.request).not.toHaveBeenCalled();
    const assets = await b.client.list("/releases/2/assets");
    const downloaded = b.client.request.mock.calls.map(([url]) => assets.find((asset) => url === `/releases/assets/${asset.id}`)?.name);
    const manifest = JSON.parse(b.contents["latest.json"]);
    const updaterSignatures = [...new Set(Object.values(manifest.platforms).map(({ url }) => `${decodeURIComponent(url.split("/").at(-1))}.sig`))];
    expect(downloaded.toSorted()).toEqual(["release.json", "latest.json", ...updaterSignatures].toSorted());
  });

  it.each(["url", "signature"])("rejects an invalid other-channel %s before changing either feed", async (field) => {
    const b = fakeGitHub(nightly, { published: true, id: 2 });
    const manifest = JSON.parse(b.contents["latest.json"]);
    manifest.platforms["linux-x86_64"][field] = "invalid";
    b.contents["latest.json"] = JSON.stringify(manifest);
    const previous = { version: "previous" };
    await mkdir(path.join(directory, "updates"));
    await writeJson(path.join(directory, "updates/stable.json"), previous);
    await writeJson(path.join(directory, "updates/nightly.json"), previous);
    await expect(writeFeeds(b.client, path.join(directory, "updates"), stable, { version: stable.version })).rejects.toThrow(/unexpected URL or signature/);
    expect(await readJson(path.join(directory, "updates/stable.json"))).toEqual(previous);
    expect(await readJson(path.join(directory, "updates/nightly.json"))).toEqual(previous);
  });
});
