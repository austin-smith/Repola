// @vitest-environment node
import { describe, expect, it, vi } from "vitest";
import { planRelease, resolveMetadata, stampVersions } from "./release-metadata.mjs";
import { compareVersions, githubClient, parseVersion, publishedReleases, validateRelease } from "./release-utils.mjs";
import { createUpdaterManifest, expectedArtifacts, validateArtifactMetadata, validateChecksum, validateUpdaterManifest } from "./release-artifacts.mjs";

const sha = "a".repeat(40);
const source = { version: "0.1.0", sha, runId: "123", pubDate: "2026-09-30T09:17:00Z" };
const stable = planRelease({ ...source, sourceRef: "refs/tags/v0.1.0", tag: "v0.1.0" });
const nightly = planRelease({ ...source, sourceRef: "refs/heads/main", runNumber: "10" });
const signatures = Object.fromEntries(expectedArtifacts(stable).map((entry) => [entry.name, "signature\n"]));

describe("release versions and provenance", () => {
  it("orders numeric nightly sequences, upcoming releases, and stable promotion", () => {
    expect(compareVersions("0.1.0-nightly.10", "0.1.0-nightly.9")).toBe(1);
    expect(compareVersions("0.1.0", "0.1.0-nightly.100")).toBe(1);
    expect(compareVersions("0.2.0-nightly.1", "0.1.0")).toBe(1);
    expect(compareVersions("0.1.0-nightly.10", "0.1.0-nightly.10")).toBe(0);
  });

  it.each(["0.1.0+sha", "0.1.0-nightly.0", "0.1.0-nightly.01", "0.1.0-beta.1", "v0.1.0", "01.1.0", "0.1.0\n"])('rejects unsupported version "%s"', (version) => {
    expect(() => parseVersion(version)).toThrow();
  });

  it("requires exact stable tags and main-only nightly provenance", () => {
    expect(() => planRelease({ ...source, tag: "v0.2.0", sourceRef: "refs/tags/v0.2.0" })).toThrow(/match/);
    expect(() => planRelease({ ...source, runNumber: "10", sourceRef: "refs/heads/feature" })).toThrow(/workflow/);
    expect(() => validateRelease({ ...nightly, channel: "stable" })).toThrow(/channel/);
    expect(() => validateRelease({ ...stable, sha: "main" })).toThrow(/source commit/);
  });

  it("stamps both inherited Rust versions and their lock entries without changing dependencies", () => {
    const input = {
      packageJson: { version: "0.1.0", name: "repola" },
      tauriConfig: { version: "0.1.0", identifier: "net.crapshack.repola" },
      cargoManifest: '[workspace.package]\nversion = "0.1.0"\n\n[package]\nname = "repola"\nversion.workspace = true\n',
      cargoLock: '[[package]]\nname = "repola"\nversion = "0.1.0"\n\n[[package]]\nname = "repola-engine"\nversion = "0.1.0"\n\n[[package]]\nname = "serde"\nversion = "1.0.0"\n',
    };
    const stamped = stampVersions(input, nightly);
    expect(stamped.packageJson.version).toBe(nightly.version);
    expect(stamped.tauriConfig.version).toBe(nightly.version);
    expect(stamped.cargoManifest).toContain(`version = "${nightly.version}"`);
    expect(stamped.cargoManifest).toContain("version.workspace = true");
    expect(stamped.cargoLock.match(/version = "0.1.0-nightly.10"/g)).toHaveLength(2);
    expect(stamped.cargoLock).toContain('name = "serde"\nversion = "1.0.0"');
    expect(() => stampVersions({ ...input, tauriConfig: { version: "0.2.0" } }, nightly)).toThrow(/agree/);
    expect(() => stampVersions({ ...input, cargoLock: "" }, nightly)).toThrow(/both/);
  });
});

describe("complete updater manifests", () => {
  it("pins supported platforms to immutable matching release assets", () => {
    const manifest = createUpdaterManifest(stable, signatures, "Changes");
    expect(Object.keys(manifest.platforms)).toEqual(["darwin-aarch64", "linux-x86_64", "linux-x86_64-deb", "windows-x86_64"]);
    expect(manifest.platforms["linux-x86_64-deb"].url).toMatch(/\.deb$/);
    expect(manifest.platforms["linux-x86_64"].url).toMatch(/\.AppImage$/);
    expect(manifest.platforms["darwin-aarch64"].url).toContain("/releases/download/v0.1.0/Repola-0.1.0-arm64.app.tar.gz");
    expect(manifest.platforms["windows-x86_64"].url).toContain("/releases/download/v0.1.0/Repola-0.1.0-x64-setup.exe");
    expect(() => validateUpdaterManifest(manifest, stable, signatures)).not.toThrow();
  });

  it("matches staged metadata by target and rejects omitted or foreign installers", () => {
    const target = "aarch64-apple-darwin";
    const artifacts = ["repola-agent-aarch64-apple-darwin", "Repola-0.1.0-arm64.dmg", "Repola-0.1.0-arm64.app.tar.gz"];
    const metadata = { target, release: stable, artifacts };
    expect(() => validateArtifactMetadata(metadata, stable, target)).not.toThrow();
    expect(() => validateArtifactMetadata({ ...metadata, artifacts: artifacts.slice(0, 1) }, stable, target)).toThrow(/metadata/);
    expect(() => validateArtifactMetadata({ ...metadata, artifacts: [...artifacts, "Repola-0.1.0-x64.deb"] }, stable, target)).toThrow(/metadata/);
    expect(() => validateArtifactMetadata(metadata, nightly, target)).toThrow(/metadata/);
    expect(() => validateArtifactMetadata(metadata, stable, "x86_64-pc-windows-msvc")).toThrow(/metadata/);
  });

  it("rejects missing platforms, foreign URLs, versions, and mismatched signatures", () => {
    const manifest = createUpdaterManifest(stable, signatures, "Changes");
    const missing = structuredClone(manifest);
    delete missing.platforms["linux-x86_64"];
    expect(() => validateUpdaterManifest(missing, stable, signatures)).toThrow(/complete/);
    const foreign = structuredClone(manifest);
    foreign.platforms["linux-x86_64"].url = "https://example.com/app";
    expect(() => validateUpdaterManifest(foreign, stable, signatures)).toThrow(/unexpected URL/);
    expect(() => validateUpdaterManifest({ ...manifest, version: "0.2.0" }, stable, signatures)).toThrow(/complete/);
    expect(() => validateUpdaterManifest(manifest, stable, {})).toThrow(/Missing updater signature/);
  });

  it("requires named SHA-256 checksums and rejects altered bytes", () => {
    const checksum = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08  agent\n";
    expect(() => validateChecksum(Buffer.from("test"), checksum, "agent")).not.toThrow();
    expect(() => validateChecksum(Buffer.from("altered"), checksum, "agent")).toThrow();
    expect(() => validateChecksum(Buffer.from("test"), checksum, "other-agent")).toThrow();
  });

  it("selects each published channel by version, excluding drafts and unrelated tags", () => {
    const releases = [
      { tag_name: "v0.1.0", prerelease: false, draft: false },
      { tag_name: "v0.2.0", prerelease: false, draft: true },
      { tag_name: "v0.2.0-nightly.9", prerelease: true, draft: false },
      { tag_name: "v0.2.0-nightly.10", prerelease: true, draft: false },
      { tag_name: "nightly", prerelease: true, draft: false },
    ];
    expect(publishedReleases(releases, "stable").map((entry) => entry.tag_name)).toEqual(["v0.1.0"]);
    expect(publishedReleases(releases, "nightly").map((entry) => entry.tag_name)).toEqual(["v0.2.0-nightly.10", "v0.2.0-nightly.9"]);
  });
});

function metadataClient({ runs, comparison = "identical", releases = [], previous = nightly } = {}) {
  return {
    request: vi.fn(async (url) => {
      if (url.startsWith("/commits/")) return { sha };
      if (url.startsWith("/compare/")) return { status: comparison };
      if (url.startsWith("/actions/workflows/")) return { workflow_runs: runs ?? [{ head_sha: sha, head_branch: "main", status: "completed", conclusion: "success" }] };
      if (url.startsWith("/contents/")) return { content: Buffer.from('{"version":"0.1.0"}').toString("base64") };
      if (url.startsWith("/actions/runs/")) return { created_at: source.pubDate };
      if (url.startsWith("/releases/assets/")) return Buffer.from(JSON.stringify(previous));
      throw new Error(`Unexpected API request: ${url}`);
    }),
    list: vi.fn(async (url) => url === "/releases" ? releases : [{ id: 1, name: "release.json" }]),
  };
}

const environment = { GITHUB_REPOSITORY: "austin-smith/Repola", GITHUB_REF_TYPE: "branch", GITHUB_REF: "refs/heads/main", GITHUB_SHA: sha, GITHUB_RUN_ID: "123", GITHUB_RUN_NUMBER: "10" };

describe("release source gates", () => {
  it("resolves canonical repository and freezes run metadata", async () => {
    expect(await resolveMetadata(environment, metadataClient())).toEqual(nightly);
  });

  it("requires successful CI at the exact main commit", async () => {
    await expect(resolveMetadata(environment, metadataClient({ runs: [{ head_sha: "b".repeat(40), head_branch: "main", status: "completed", conclusion: "success" }] }))).rejects.toThrow(/exact commit/);
    await expect(resolveMetadata(environment, metadataClient({ runs: [] }))).rejects.toThrow(/exact commit/);
    await expect(resolveMetadata(environment, metadataClient({ comparison: "diverged" }))).rejects.toThrow(/belong to main/);
  });

  it("rejects fork workflows and non-main dispatch", async () => {
    await expect(resolveMetadata({ ...environment, GITHUB_REPOSITORY: "fork/Repola" }, metadataClient())).rejects.toThrow(/restricted/);
    await expect(resolveMetadata({ ...environment, GITHUB_REF: "refs/heads/feature" }, metadataClient())).rejects.toThrow(/main/);
  });

  it("skips an unchanged nightly and requires the next base version after stable publication", async () => {
    const releases = [{ id: 1, tag_name: "v0.1.0-nightly.9", prerelease: true, draft: false }];
    await expect(resolveMetadata(environment, metadataClient({ releases, previous: { ...nightly, version: "0.1.0-nightly.9", tag: "v0.1.0-nightly.9" } }))).resolves.toBeNull();
    await expect(resolveMetadata(environment, metadataClient({ releases: [{ id: 2, tag_name: "v0.1.0", prerelease: false, draft: false }] }))).rejects.toThrow(/Bump main/);
  });
});

describe("GitHub API handling", () => {
  it("paginates release inventories and fails closed on API errors", async () => {
    const fetcher = vi.fn().mockResolvedValueOnce({ ok: true, json: async () => Array.from({ length: 100 }, (_, id) => ({ id })) }).mockResolvedValueOnce({ ok: true, json: async () => [{ id: 100 }] });
    const client = githubClient("token", fetcher);
    expect(await client.list("/releases")).toHaveLength(101);
    expect(fetcher.mock.calls[1][0]).toContain("page=2");
    await expect(githubClient("token", async () => ({ ok: false, status: 403 })).request("/releases")).rejects.toThrow(/403/);
  });
});
