// @vitest-environment node
import { mkdir, mkdtemp, readFile, readdir, rm, stat, unlink, utimes, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { PNG } from "pngjs";
import { composeIconArtwork, iconArtwork, universalArtwork } from "./icon-artwork.mjs";
import { canonicalizeIcns } from "./icns.mjs";
import { generateIcons } from "./generate-icons.mjs";

async function filesIn(directory, prefix = "") {
  const files = [];
  for (const entry of await readdir(path.join(directory, prefix), { withFileTypes: true })) {
    const relative = path.join(prefix, entry.name);
    if (entry.isDirectory()) files.push(...await filesIn(directory, relative));
    else files.push(relative);
  }
  return files;
}

it("exports a fresh desktop set, leaves matching files untouched, and detects and repairs stale exports", async () => {
  const outputRoot = await mkdtemp(path.join(tmpdir(), "repola-icon-export-test-"));
  // Exercise orchestration and filesystem behavior without invoking an expensive external CLI.
  // The explicit pnpm icons command invokes the real native exporter.
  const renderIcons = async (source, directory, sizes) => {
    await mkdir(directory, { recursive: true });
    const contents = await readFile(source);
    const files = sizes ? sizes.split(",").map((size) => `${size}x${size}.png`)
      : ["32x32.png", "128x128.png", "128x128@2x.png", "icon.ico"];
    for (const file of files) await writeFile(path.join(directory, file), contents);
    if (!sizes) {
      const icns = Buffer.alloc(8);
      icns.write("icns");
      icns.writeUInt32BE(8, 4);
      await writeFile(path.join(directory, "icon.icns"), icns);
    }
  };
  const options = { outputRoot, renderIcons };
  try {
    await generateIcons(options);
    const files = await filesIn(outputRoot);
    expect(files).toHaveLength(22);
    expect(files.some((file) => /android|ios|Square|StoreLogo|64x64|logo\.png|app-icon-macos/.test(file))).toBe(false);
    const stable = await readFile(path.join(outputRoot, "src-tauri", "assets", "icons", "32x32.png"));
    const nightly = await readFile(path.join(outputRoot, "src-tauri", "assets", "icons", "nightly", "32x32.png"));
    const development = await readFile(path.join(outputRoot, "src-tauri", "assets", "icons", "development", "32x32.png"));
    expect(nightly.equals(stable)).toBe(true);
    expect(development.equals(stable)).toBe(false);
    const baseline = new Map();
    for (const file of files) {
      const destination = path.join(outputRoot, file);
      // A fixed old timestamp detects rewrites even on filesystems with coarse timestamps.
      await utimes(destination, new Date("2000-01-01"), new Date("2000-01-01"));
      baseline.set(file, { contents: await readFile(destination), mtime: (await stat(destination)).mtimeMs });
    }
    await generateIcons(options);
    for (const [file, original] of baseline) {
      const destination = path.join(outputRoot, file);
      expect((await stat(destination)).mtimeMs, file).toBe(original.mtime);
      expect((await readFile(destination)).equals(original.contents), file).toBe(true);
    }

    await generateIcons({ ...options, check: true });
    for (const [file, original] of baseline) {
      expect((await stat(path.join(outputRoot, file))).mtimeMs, file).toBe(original.mtime);
    }

    const damaged = path.join("src-tauri", "assets", "icons", "development", "icon.ico");
    const destination = path.join(outputRoot, damaged);
    await writeFile(destination, "damaged export");
    await expect(generateIcons({ ...options, check: true })).rejects.toThrow(damaged);
    expect(await readFile(destination, "utf8")).toBe("damaged export");
    await generateIcons(options);
    expect((await readFile(destination)).equals(baseline.get(damaged).contents)).toBe(true);
    await unlink(destination);
    await generateIcons(options);
    expect((await readFile(destination)).equals(baseline.get(damaged).contents)).toBe(true);
    // Repairing one export must not rewrite the other correctly generated files.
    for (const [file, original] of baseline) {
      if (file !== damaged) expect((await stat(path.join(outputRoot, file))).mtimeMs, file).toBe(original.mtime);
    }
  } finally {
    await rm(outputRoot, { recursive: true, force: true });
  }
}, 60_000);

const brand = new URL("../src-tauri/assets/brand/", import.meta.url);

describe("shared icon artwork", () => {
  it("uses the original foreground bounds and opaque colors in every build", async () => {
    const foreground = PNG.sync.read(await readFile(new URL(iconArtwork.foreground, brand)));
    const bounds = [foreground.width, foreground.height, 0, 0];
    const opaqueOffsets = [];
    for (let y = 0; y < foreground.height; y++) {
      for (let x = 0; x < foreground.width; x++) {
        const offset = (y * foreground.width + x) * 4;
        if (foreground.data[offset + 3] > 128) {
          bounds[0] = Math.min(bounds[0], x);
          bounds[1] = Math.min(bounds[1], y);
          bounds[2] = Math.max(bounds[2], x + 1);
          bounds[3] = Math.max(bounds[3], y + 1);
        }
        if (foreground.data[offset + 3] === 255) opaqueOffsets.push(offset);
      }
    }
    expect(bounds).toEqual([279, 219, 763, 800]);
    const originalColors = Buffer.concat(opaqueOffsets.map((offset) => foreground.data.subarray(offset, offset + 4)));
    for (const channel of Object.keys(iconArtwork.backgrounds)) {
      const image = await composeIconArtwork(channel);
      const sharedColors = Buffer.concat(opaqueOffsets.map((offset) => image.data.subarray(offset, offset + 4)));
      expect(sharedColors.equals(originalColors)).toBe(true);
      const cropped = universalArtwork(image);
      expect([cropped.width, cropped.height]).toEqual([824, 824]);
      // Every opaque R pixel retains its color at the corresponding cropped coordinate.
      const croppedColors = Buffer.concat(opaqueOffsets.map((offset) => {
        const pixel = offset / 4;
        const x = pixel % foreground.width - iconArtwork.tile.x;
        const y = Math.floor(pixel / foreground.width) - iconArtwork.tile.y;
        const croppedOffset = (y * cropped.width + x) * 4;
        return cropped.data.subarray(croppedOffset, croppedOffset + 4);
      }));
      expect(croppedColors.equals(originalColors)).toBe(true);
    }
  });

});

function element(type, payload) {
  const header = Buffer.alloc(8);
  header.write(type);
  header.writeUInt32BE(8 + payload.length, 4);
  return Buffer.concat([header, payload]);
}

function container(elements) {
  const header = Buffer.alloc(8);
  header.write("icns");
  header.writeUInt32BE(8 + elements.reduce((length, element) => length + element.length, 0), 4);
  return Buffer.concat([header, ...elements]);
}

describe("reproducible ICNS containers", () => {
  it("makes unordered exports identical without changing any rendition bytes", () => {
    const small = element("ic07", Buffer.from([0, 12, 37, 255]));
    const large = element("ic10", Buffer.from([255, 7, 33]));
    const expected = container([small, large]);
    expect(canonicalizeIcns(container([large, small]))).toEqual(expected);
    expect(canonicalizeIcns(expected)).toEqual(expected);
  });

  it("rejects damaged lengths instead of silently dropping or looping over image data", () => {
    const damaged = container([element("ic07", Buffer.from([1]))]);
    damaged.writeUInt32BE(0, 12);
    expect(() => canonicalizeIcns(damaged)).toThrow(/element length/);
    expect(() => canonicalizeIcns(Buffer.from("icns"))).toThrow(/container/);
  });
});
