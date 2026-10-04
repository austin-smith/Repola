// @vitest-environment node
import { describe, expect, it, vi } from "vitest";
import { ghClient, stableVersion, startRelease } from "./start-release.mjs";

const sha = "a".repeat(40);
const tagSha = "c".repeat(40);
const annotation = ["/git/tags", { fields: { tag: "v0.1.0", message: "release 0.1.0", object: sha, type: "commit" } }];
const reference = ["/git/refs", { fields: { ref: "refs/tags/v0.1.0", sha: tagSha } }];
const files = {
  "package.json": '{"version":"0.1.0"}',
  "src-tauri/tauri.conf.json": '{"version":"0.1.0"}',
  "src-tauri/Cargo.toml": '[workspace.package]\nversion = "0.1.0"\n\n[package]\nname = "repola"\nversion.workspace = true\n',
  "src-tauri/Cargo.lock": '[[package]]\nname = "repola"\nversion = "0.1.0"\n\n[[package]]\nname = "repola-engine"\nversion = "0.1.0"\n',
};
const success = { id: 1, head_sha: sha, head_branch: "main", event: "push", status: "completed", conclusion: "success" };

function fixture({ contents = files, existingTag = false, advance = false, ci = [[success]] } = {}) {
  let reads = 0;
  let polls = 0;
  let time = 0;
  const api = vi.fn(async (endpoint, options) => {
    if (endpoint === "/commits/main") return { sha: advance && reads++ > 0 ? "b".repeat(40) : sha };
    if (endpoint.startsWith("/contents/")) {
      const [name, ref] = endpoint.slice("/contents/".length).split("?ref=");
      expect(ref).toBe(sha);
      return { encoding: "base64", content: Buffer.from(contents[name]).toString("base64") };
    }
    if (endpoint.startsWith("/git/ref/tags/")) return existingTag ? { ref: "refs/tags/v0.1.0" } : null;
    if (endpoint.startsWith("/actions/workflows/ci.yml/runs?")) {
      expect(endpoint).toContain(`head_sha=${sha}&branch=main&event=push`);
      expect(options.paginate).toBe(true);
      return [{ workflow_runs: ci[Math.min(polls++, ci.length - 1)] }];
    }
    if (endpoint === "/git/tags") return { sha: tagSha, tag: options.fields.tag, object: { sha: options.fields.object, type: options.fields.type } };
    if (endpoint === "/git/refs") return { ref: options.fields.ref, object: { sha: options.fields.sha } };
    throw new Error(`Unexpected request: ${endpoint}`);
  });
  const sleep = vi.fn(async (ms) => { time += ms; });
  const log = vi.fn();
  const options = { api, sleep, now: () => time, interval: 10, timeout: 20, log };
  const mutations = () => api.mock.calls.filter(([, options]) => options?.fields);
  return { api, sleep, log, options, mutations };
}

describe("stable release command", () => {
  it("derives the version and creates an annotated tag for the verified main commit", async () => {
    const f = fixture();
    await expect(startRelease(f.options)).resolves.toEqual({ tag: "v0.1.0", sha, state: "success" });
    expect(f.mutations()).toEqual([annotation, reference]);
    expect(f.sleep).not.toHaveBeenCalled();
  });

  it("waits for CI to start and pass before creating the tag", async () => {
    const f = fixture({ ci: [[], [{ ...success, status: "in_progress", conclusion: null }], [success]] });
    f.sleep.mockImplementation(async () => { expect(f.mutations()).toEqual([]); });
    await startRelease(f.options);
    expect(f.sleep).toHaveBeenCalledTimes(2);
    expect(f.mutations()).toEqual([annotation, reference]);
    expect(f.log.mock.calls.filter(([text]) => text.startsWith("Waiting"))).toHaveLength(1);
  });

  it.each(["failure", "cancelled", "timed_out"])("stops after %s CI without creating a tag", async (conclusion) => {
    const f = fixture({ ci: [[{ ...success, conclusion, html_url: "https://github.com/run/1" }]] });
    await expect(startRelease(f.options)).rejects.toThrow(/Fix or rerun CI/);
    expect(f.mutations()).toEqual([]);
  });

  it("times out without accepting green CI for another commit, branch, or event", async () => {
    const f = fixture({ ci: [[{ ...success, head_sha: "b".repeat(40) }, { ...success, head_branch: "other" }, { ...success, event: "pull_request" }]] });
    await expect(startRelease(f.options)).rejects.toThrow(/Timed out/);
    expect(f.mutations()).toEqual([]);
    expect(f.sleep).toHaveBeenCalledTimes(2);
  });

  it("checks every CI page for the exact source commit", async () => {
    const f = fixture();
    const request = f.api.getMockImplementation();
    f.api.mockImplementation((endpoint, options) => endpoint.startsWith("/actions/workflows/")
      ? [{ workflow_runs: [{ ...success, head_sha: "b".repeat(40) }] }, { workflow_runs: [success] }]
      : request(endpoint, options));
    await startRelease(f.options);
    expect(f.mutations()).toEqual([annotation, reference]);
  });

  it.each([true, false])("previews without creating a tag or waiting (CI passed=%s)", async (passed) => {
    const f = fixture({ ci: [passed ? [success] : []] });
    await expect(startRelease({ ...f.options, dryRun: true })).resolves.toMatchObject({ state: passed ? "success" : "pending" });
    expect(f.mutations()).toEqual([]);
    expect(f.sleep).not.toHaveBeenCalled();
  });

  it("refuses an existing tag without moving or deleting it", async () => {
    const f = fixture({ existingTag: true });
    await expect(startRelease(f.options)).rejects.toThrow(/already exists/);
    expect(f.mutations()).toEqual([]);
  });

  it("stops when main advances during preparation", async () => {
    const f = fixture({ advance: true });
    await expect(startRelease(f.options)).rejects.toThrow(/Main advanced/);
    expect(f.mutations()).toEqual([annotation]);
  });

  it.each([
    { sha: "invalid", tag: "v0.1.0", object: { sha, type: "commit" } },
    { sha: tagSha, tag: "v0.2.0", object: { sha, type: "commit" } },
    { sha: tagSha, tag: "v0.1.0", object: { sha: "b".repeat(40), type: "commit" } },
    { sha: tagSha, tag: "v0.1.0", object: { sha, type: "tree" } },
  ])("refuses to create a reference for an incorrect tag object: %j", async (object) => {
    const f = fixture();
    const request = f.api.getMockImplementation();
    f.api.mockImplementation((endpoint, options) => endpoint === "/git/tags" ? object : request(endpoint, options));
    await expect(startRelease(f.options)).rejects.toThrow(/invalid annotated/);
    expect(f.mutations()).toEqual([annotation]);
  });

  it("propagates tag creation failure without attempting an overwrite", async () => {
    const f = fixture();
    const request = f.api.getMockImplementation();
    f.api.mockImplementation((endpoint, options) => {
      if (endpoint === "/git/refs") throw new Error("HTTP 422: tag created concurrently");
      return request(endpoint, options);
    });
    await expect(startRelease(f.options)).rejects.toThrow(/422/);
    expect(f.mutations()).toEqual([annotation, reference]);
  });
});

describe("source version checks", () => {
  it("reads the stable version from consistent remote manifests", () => {
    expect(stableVersion(files)).toBe("0.1.0");
  });

  it.each(Object.keys(files))("rejects mismatched %s", (name) => {
    expect(() => stableVersion({ ...files, [name]: files[name].replaceAll("0.1.0", "0.2.0") })).toThrow(/agree/);
  });

  it("requires both distinct workspace lock entries and a stable version", () => {
    expect(() => stableVersion({ ...files, "src-tauri/Cargo.lock": files["src-tauri/Cargo.lock"].replace("repola-engine", "repola") })).toThrow(/agree/);
    expect(() => stableVersion({ ...files, "package.json": '{"version":"0.1.0-nightly.1"}' })).toThrow(/stable/);
  });
});

describe("GitHub CLI adapter", () => {
  it("passes tag fields as arguments without invoking a shell", async () => {
    const run = vi.fn().mockResolvedValue({ stdout: '{"ref":"refs/tags/v0.1.0"}' });
    await ghClient(run)("/git/refs", { fields: { ref: "refs/tags/v0.1.0", sha } });
    expect(run).toHaveBeenCalledWith("gh", ["api", "repos/austin-smith/Repola/git/refs", "--hostname", "github.com", "--method", "POST", "--raw-field", "ref=refs/tags/v0.1.0", "--raw-field", `sha=${sha}`], expect.not.objectContaining({ shell: true }));
  });

  it("treats only HTTP 404 as an absent tag and fails on authorization errors", async () => {
    const run = vi.fn().mockRejectedValueOnce(Object.assign(new Error("not found"), { stderr: "gh: Not Found (HTTP 404)" }))
      .mockRejectedValueOnce(Object.assign(new Error("forbidden"), { stderr: "gh: Forbidden (HTTP 403)" }));
    const api = ghClient(run);
    await expect(api("/git/ref/tags/v0.1.0", { missing: true })).resolves.toBeNull();
    await expect(api("/git/ref/tags/v0.1.0", { missing: true })).rejects.toThrow(/forbidden/);
  });
});
