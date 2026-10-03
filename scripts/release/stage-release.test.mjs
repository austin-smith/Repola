// @vitest-environment node
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { stageBundles } from "./stage-release.mjs";
import { targets } from "./release-utils.mjs";

async function withBundles(files, run) {
  const directory = await mkdtemp(path.join(os.tmpdir(), "repola bundles "));
  const bundle = path.join(directory, "bundle");
  const destination = path.join(directory, "installers");
  try {
    await mkdir(bundle);
    for (const [name, bytes] of Object.entries(files)) {
      const file = path.join(bundle, name);
      await mkdir(path.dirname(file), { recursive: true });
      await writeFile(file, bytes);
    }
    await run(bundle, destination);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

describe("installer downloads", () => {
  it.each([
    { target: "aarch64-apple-darwin", installers: { "dmg/Repola_0.1.0_aarch64.dmg": "Repola-0.1.0-arm64.dmg" }, extra: "macos/Repola.app.tar.gz" },
    { target: "x86_64-pc-windows-msvc", installers: { "nsis/Repola_0.1.0_x64-setup.exe": "Repola-0.1.0-x64-setup.exe" }, extra: "nsis/Repola_0.1.0_x64-setup.exe.sig" },
    { target: "x86_64-unknown-linux-gnu", installers: { "deb/Repola_0.1.0_amd64.deb": "Repola-0.1.0-x64.deb", "appimage/Repola_0.1.0_amd64.AppImage": "Repola-0.1.0-x64.AppImage" }, extra: "appimage/Repola_0.1.0_amd64.AppImage.sig" },
  ])("collects only flat $target installers without changing signed bytes", async ({ target, installers, extra }) => {
    const bytes = Buffer.from([0, 255, 1, 128, 42]);
    const files = { ...Object.fromEntries(Object.keys(installers).map((name) => [name, bytes])), [extra]: "updater data", "repola-agent": "agent", ".DS_Store": "metadata" };
    await withBundles(files, async (bundle, destination) => {
      const names = await stageBundles(targets.find((entry) => entry.target === target), "0.1.0", bundle, destination, { includeUpdater: false });
      expect(names.toSorted()).toEqual(Object.values(installers).toSorted());
      expect((await readdir(destination)).toSorted()).toEqual(names.toSorted());
      for (const name of names) expect(await readFile(path.join(destination, name))).toEqual(bytes);
    });
  });

  it("retains the macOS updater archive for release staging", async () => {
    await withBundles({ "dmg/Repola.dmg": "installer", "macos/Repola.app.tar.gz": "updater" }, async (bundle, destination) => {
      const names = await stageBundles(targets[0], "0.1.0-nightly.10", bundle, destination);
      expect(names).toEqual(["Repola-0.1.0-nightly.10-arm64.dmg", "Repola-0.1.0-nightly.10-arm64.app.tar.gz"]);
    });
  });

  it.each([{}, { "dmg/one.dmg": "one", "dmg/two.dmg": "two" }])("rejects missing or ambiguous installers before copying", async (files) => {
    await withBundles(files, async (bundle, destination) => {
      await expect(stageBundles(targets[0], "0.1.0", bundle, destination, { includeUpdater: false })).rejects.toThrow(/Expected one \.dmg/);
      await expect(readdir(destination)).rejects.toMatchObject({ code: "ENOENT" });
    });
  });
});
