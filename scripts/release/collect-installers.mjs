import path from "node:path";
import { stageBundles } from "./stage-release.mjs";
import { isMain, parseVersion, readJson, root, targets } from "./release-utils.mjs";

export async function collectInstallers(targetName, destination) {
  const target = targets.find((entry) => entry.target === targetName && entry.platform);
  if (!target) throw new Error(`Unsupported installer target: ${targetName}.`);
  const { version } = await readJson(path.join(root, "package.json"));
  parseVersion(version);
  return stageBundles(target, version, path.join(root, "src-tauri", "target", targetName, "release", "bundle"), destination, { includeUpdater: false });
}

if (isMain(import.meta.url)) await collectInstallers(process.argv[2], process.argv[3]);
