import { execFile } from "node:child_process";
import { setTimeout as delay } from "node:timers/promises";
import { promisify } from "node:util";
import { isMain, parseVersion, repository, root } from "./release-utils.mjs";

const execute = promisify(execFile);

export function ghClient(run = execute) {
  return async (endpoint, { fields, missing = false, paginate = false } = {}) => {
    const args = ["api", `repos/${repository}${endpoint}`, "--hostname", "github.com", "--method", fields ? "POST" : "GET"];
    if (paginate) args.push("--paginate", "--slurp");
    for (const [key, value] of Object.entries(fields ?? {})) args.push("--raw-field", `${key}=${value}`);
    try {
      const { stdout } = await run("gh", args, { cwd: root, timeout: 120_000, maxBuffer: 8 * 1024 * 1024 });
      return JSON.parse(stdout);
    } catch (error) {
      if (missing && /\bHTTP 404\b/.test(error.stderr ?? "")) return null;
      throw error;
    }
  };
}

export function stableVersion(files) {
  const version = JSON.parse(files["package.json"]).version;
  if (parseVersion(version).nightly !== null) throw new Error("Main must contain a stable application version.");
  const workspace = /^\[workspace\.package\]\s*\n([\s\S]*?)(?=^\[|(?![\s\S]))/m.exec(files["src-tauri/Cargo.toml"]);
  const rustVersion = workspace?.[1].match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  const lockVersions = [...files["src-tauri/Cargo.lock"].matchAll(/\[\[package\]\]\s*\nname = "(repola|repola-engine)"\s*\nversion = "([^"]+)"/g)];
  if (JSON.parse(files["src-tauri/tauri.conf.json"]).version !== version || rustVersion !== version
    || lockVersions.length !== 2 || new Set(lockVersions.map((match) => match[1])).size !== 2
    || lockVersions.some((match) => match[2] !== version)) {
    throw new Error("Package, Tauri, Cargo workspace, and both Cargo.lock versions must agree on main.");
  }
  return version;
}

async function ciState(api, sha) {
  const pages = await api(`/actions/workflows/ci.yml/runs?head_sha=${sha}&branch=main&event=push&per_page=100`, { paginate: true });
  const runs = pages.flatMap((page) => page.workflow_runs)
    .filter((run) => run.head_sha === sha && run.head_branch === "main" && run.event === "push")
    .sort((a, b) => b.id - a.id);
  if (runs.some((run) => run.status === "completed" && run.conclusion === "success")) return "success";
  if (runs[0]?.status === "completed") throw new Error(`Main CI ${runs[0].conclusion}: ${runs[0].html_url}. Fix or rerun CI before starting the release.`);
  return "pending";
}

export async function startRelease({ api = ghClient(), dryRun = false, sleep = delay, now = Date.now, interval = 60_000, timeout = 3_600_000, log = console.log } = {}) {
  const { sha } = await api("/commits/main");
  if (!/^[a-f0-9]{40}$/.test(sha)) throw new Error("GitHub returned an invalid main commit.");
  const names = ["package.json", "src-tauri/tauri.conf.json", "src-tauri/Cargo.toml", "src-tauri/Cargo.lock"];
  const files = Object.fromEntries(await Promise.all(names.map(async (name) => {
    const file = await api(`/contents/${name}?ref=${sha}`);
    if (file.encoding !== "base64" || typeof file.content !== "string") throw new Error(`Cannot read ${name} at the release commit.`);
    return [name, Buffer.from(file.content, "base64").toString("utf8")];
  })));
  const version = stableVersion(files);
  const tag = `v${version}`;
  const tagEndpoint = `/git/ref/tags/${tag}`;
  if (await api(tagEndpoint, { missing: true })) throw new Error(`${tag} already exists. Rerun its failed Release workflow to resume the draft; use a new version for a new release.`);
  log(`Stable release ${tag} from main (${sha.slice(0, 7)}).`);
  let state = await ciState(api, sha);
  if (dryRun) {
    log(state === "success" ? "CI passed. Would create the tag and start the draft build." : "Would wait for main CI, then create the tag and start the draft build.");
    return { tag, sha, state };
  }
  const started = now();
  if (state !== "success") log("Waiting for main CI to pass…");
  while (state !== "success") {
    const remaining = timeout - (now() - started);
    if (remaining <= 0) throw new Error("Timed out waiting for main CI. No tag was created; rerun pnpm release when CI is ready.");
    await sleep(Math.min(interval, remaining));
    state = await ciState(api, sha);
  }
  const annotated = await api("/git/tags", { fields: { tag, message: `release ${version}`, object: sha, type: "commit" } });
  if (!/^[a-f0-9]{40}$/.test(annotated.sha) || annotated.tag !== tag || annotated.object?.type !== "commit" || annotated.object.sha !== sha) {
    throw new Error("GitHub returned an invalid annotated release tag. No tag reference was created.");
  }
  if ((await api("/commits/main")).sha !== sha) throw new Error("Main advanced while preparing the release. No tag was created; rerun pnpm release for the current main commit.");
  await api("/git/refs", { fields: { ref: `refs/tags/${tag}`, sha: annotated.sha } });
  log(`Created ${tag}. Follow the signed installer build at https://github.com/${repository}/actions/workflows/release.yml`);
  log("When the draft is ready, review it and run Publish release from main with channel stable.");
  return { tag, sha, state };
}

if (isMain(import.meta.url)) {
  const args = process.argv.slice(2);
  if (args.length === 1 && args[0] === "--help") {
    console.log("Usage: pnpm release [--dry-run]\nStart a stable draft from GitHub main's version, waiting for CI automatically. Requires an authenticated GitHub CLI (gh).");
  } else if (args.length > 1 || args.some((arg) => arg !== "--dry-run")) {
    console.error("Usage: pnpm release [--dry-run]");
    process.exitCode = 1;
  } else {
    try {
      await startRelease({ dryRun: args.includes("--dry-run") });
    } catch (error) {
      console.error(error.message);
      process.exitCode = 1;
    }
  }
}
