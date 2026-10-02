import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFile, mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";
import { isMain, readJson, root, targets, validateRelease, writeJson } from "./release-utils.mjs";

const execute = promisify(execFile);

export function assetName(version, target, extension) {
  return `Repola_${version}_${target}${extension}`;
}

async function filesBelow(directory) {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const fullPath = path.join(directory, entry.name);
    if (entry.isDirectory() && !entry.name.endsWith(".app")) files.push(...await filesBelow(fullPath));
    else if (entry.isFile()) files.push(fullPath);
  }
  return files;
}

export async function stageRelease(targetName, metadataPath, destination) {
  const target = targets.find((entry) => entry.target === targetName);
  if (!target) throw new Error(`Unsupported release target: ${targetName}.`);
  const release = validateRelease(await readJson(metadataPath));
  await mkdir(destination, { recursive: true });
  const binaryDirectory = path.join(root, "src-tauri", "target", targetName, "release");
  const agentName = `repola-agent-${targetName}${targetName.includes("windows") ? ".exe" : ""}`;
  const agentPath = path.join(binaryDirectory, targetName.includes("windows") ? "repola-agent.exe" : "repola-agent");
  const { stdout } = await execute(agentPath, ["--version"], { timeout: 20_000 });
  if (stdout.trim() !== `repola-agent ${release.version}`) throw new Error("Built agent does not match the desktop release version.");
  await copyFile(agentPath, path.join(destination, agentName));
  const artifacts = [agentName];
  if (target.platform) {
    const bundles = await filesBelow(path.join(binaryDirectory, "bundle"));
    for (const extension of new Set([target.installer, target.updater])) {
      const candidates = bundles.filter((file) => file.endsWith(extension));
      if (candidates.length !== 1) throw new Error(`Expected one ${extension} bundle for ${targetName}; found ${candidates.length}.`);
      const name = assetName(release.version, targetName, extension);
      await copyFile(candidates[0], path.join(destination, name));
      artifacts.push(name);
    }
  }
  // Sign the final bytes, after native code signing and notarization. Use Node to
  // launch the project's locked CLI, avoiding pnpm.cmd / shell differences.
  const signer = fileURLToPath(new URL("../node_modules/@tauri-apps/cli/tauri.js", import.meta.url));
  for (const name of artifacts) {
    const artifact = path.join(destination, name);
    await execute(process.execPath, [signer, "signer", "sign", "--app-version", release.version, artifact], { timeout: 60_000 });
    const digest = createHash("sha256").update(await readFile(artifact)).digest("hex");
    await writeFile(`${artifact}.sha256`, `${digest}  ${name}\n`);
  }
  await writeJson(path.join(destination, `${targetName}.json`), { release, target: targetName, artifacts });
}

if (isMain(import.meta.url)) await stageRelease(process.argv[2], process.argv[3], process.argv[4]);
