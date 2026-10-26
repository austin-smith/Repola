import { execFile } from "node:child_process";
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { promisify } from "node:util";
import { fileURLToPath, pathToFileURL } from "node:url";
import { canonicalizeIcns } from "./icns.mjs";
import { composeIconArtwork, encodeArtwork, iconArtwork, universalArtwork } from "./icon-artwork.mjs";
import { buildIdentities } from "../src/domain/build-identity.ts";
import { buildIdentityConfig } from "./build-identity.mjs";

const execute = promisify(execFile);
const root = fileURLToPath(new URL("../", import.meta.url));
const cli = path.join(root, "node_modules", "@tauri-apps", "cli", "tauri.js");
const variants = Object.entries(buildIdentities).map(([channel, identity]) => ({
  channel, directory: path.join("src-tauri", identity.iconDirectory),
}));

async function readIfExists(file) {
  return readFile(file).catch((error) => {
    if (error.code === "ENOENT") return null;
    throw error;
  });
}

async function render(source, directory, sizes) {
  await execute(process.execPath, [cli, "icon", source, "--output", directory, ...(sizes ? ["--png", sizes] : [])], { cwd: root });
}

export async function generateIcons({ check = false, outputRoot = root, renderIcons = render } = {}) {
  const staging = await mkdtemp(path.join(tmpdir(), "repola-icons-"));
  const stale = [];
  const rendered = new Map();
  try {
    for (const variant of variants) {
      const background = iconArtwork.backgrounds[variant.channel];
      if (!rendered.has(background)) {
        const stage = path.join(staging, variant.channel);
        const universal = path.join(stage, "universal");
        const mac = path.join(stage, "mac");
        await mkdir(stage, { recursive: true });
        const nativeArtwork = await composeIconArtwork(variant.channel);
        const source = path.join(stage, "universal-source.png");
        const macSource = path.join(stage, "macos-source.png");
        await writeFile(source, encodeArtwork(universalArtwork(nativeArtwork)));
        await writeFile(macSource, encodeArtwork(nativeArtwork));
        await renderIcons(source, universal);
        await renderIcons(source, universal, "16,1024");
        await renderIcons(macSource, mac);
        const icns = path.join(mac, "icon.icns");
        await writeFile(icns, canonicalizeIcns(await readFile(icns)));
        rendered.set(background, { universal, mac });
      }
      const { universal, mac } = rendered.get(background);
      // The native bundle configuration owns the desktop export list.
      const exports = [
        ...buildIdentityConfig(variant.channel, {}).bundle.icon.map((icon) => [
          path.join(icon.endsWith(".icns") ? mac : universal, path.basename(icon)), path.join("src-tauri", icon),
        ]),
        [path.join(universal, "16x16.png"), path.join("public", "icons", `${variant.channel}-16.png`)],
        [path.join(universal, "32x32.png"), path.join("public", "icons", `${variant.channel}-32.png`)],
      ];
      if (variant.channel === "stable") exports.push([
        path.join(universal, "1024x1024.png"), path.join(variant.directory, "app-icon.png"),
      ]);
      for (const [source, relative] of exports) {
        const destination = path.join(outputRoot, relative);
        const expected = await readFile(source);
        const actual = await readIfExists(destination);
        if (actual?.equals(expected)) continue;
        if (check) stale.push(relative);
        else {
          await mkdir(path.dirname(destination), { recursive: true });
          await copyFile(source, destination);
        }
      }
      process.stdout.write(`${variant.channel}: ${check ? "checked" : "exported"} desktop icons and favicons\n`);
    }
    if (stale.length) throw new Error(`Icon exports are stale. Run pnpm icons:\n${stale.join("\n")}`);
  } finally {
    await rm(staging, { recursive: true, force: true });
  }
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) {
  const args = process.argv.slice(2);
  if (args.some((arg) => arg !== "--check")) throw new Error("Usage: pnpm icons [--check]");
  await generateIcons({ check: args.includes("--check") });
}
