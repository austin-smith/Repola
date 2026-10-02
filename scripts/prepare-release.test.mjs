// @vitest-environment node
import { describe, expect, it } from "vitest";
import { buildReleaseConfig } from "./prepare-release.mjs";
import { planRelease } from "./release-metadata.mjs";

const rawPublicKey = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
const publicKey = Buffer.from(`untrusted comment: minisign public key\n${rawPublicKey}\n`).toString("base64");
const packageJson = { version: "0.1.0" };
const tauriConfig = { version: "0.1.0" };
const cargoManifest = '[workspace]\nmembers = ["crates/repola-engine"]\n\n[workspace.package]\nversion = "0.1.0"\n\n[package]\nname = "repola"\nversion.workspace = true\n';
const release = planRelease({ version: "0.1.0", sha: "a".repeat(40), runId: "123", pubDate: "2026-09-30T09:17:00Z", sourceRef: "refs/tags/v0.1.0", tag: "v0.1.0" });

function environment(overrides = {}) {
  return {
    GITHUB_REPOSITORY: "austin-smith/repola",
    GITHUB_REF_TYPE: "tag",
    GITHUB_REF_NAME: "v0.1.0",
    GITHUB_REF: release.sourceRef,
    REPOLA_RELEASE_SHA: release.sha,
    RUNNER_OS: "Linux",
    REPOLA_SIGNING_PUBLIC_KEY: publicKey,
    TAURI_SIGNING_PRIVATE_KEY: "protected-private-key",
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "protected-password",
    ...overrides,
  };
}

function build(overrides = {}) {
  return buildReleaseConfig({
    environment: environment(overrides),
    packageJson,
    tauriConfig,
    cargoManifest,
    release,
  });
}

describe("release trust preparation", () => {
  it("generates the pinned updater channel for a tagged Linux release", () => {
    expect(build()).toEqual({
      bundle: {
        createUpdaterArtifacts: true,
        targets: ["appimage", "deb"],
      },
      plugins: {
        updater: {
          pubkey: publicKey,
          requireSignedVersion: true,
          endpoints: ["https://austin-smith.github.io/Repola/updates/stable.json"],
        },
      },
    });
  });

  it("accepts canonical repository casing but rejects other repositories and unvalidated sources", () => {
    expect(build({ GITHUB_REPOSITORY: "austin-smith/Repola" }).bundle.createUpdaterArtifacts).toBe(true);
    expect(() => build({ GITHUB_REPOSITORY: "another/Repola" })).toThrow(/pinned/);
    expect(() => build({ GITHUB_REF: "refs/heads/main" })).toThrow(/validated workflow metadata/);
    expect(() => build({ REPOLA_RELEASE_SHA: "b".repeat(40) })).toThrow(/validated workflow metadata/);
  });

  it("rejects malformed or legacy signing public keys", () => {
    expect(() => build({ REPOLA_SIGNING_PUBLIC_KEY: "not-base64!" })).toThrow(/one-line base64/);
    expect(() => build({ REPOLA_SIGNING_PUBLIC_KEY: Buffer.alloc(42).toString("base64") })).toThrow(/Tauri-encoded modern Minisign/);
  });

  it("requires complete Apple signing and notarization trust", () => {
    expect(() => build({ RUNNER_OS: "macOS" })).toThrow(/APPLE_SIGNING_IDENTITY/);
    expect(() => build({
      RUNNER_OS: "macOS",
      APPLE_SIGNING_IDENTITY: "Developer ID Application: Example",
      APPLE_CERTIFICATE: "certificate",
      APPLE_CERTIFICATE_PASSWORD: "password",
    })).toThrow(/notarization credentials/);
    expect(() => build({ RUNNER_OS: "macOS", APPLE_SIGNING_IDENTITY: "-" })).toThrow(/ad-hoc/);
  });

  it("requires the protected Windows signing command", () => {
    expect(() => build({ RUNNER_OS: "Windows" })).toThrow(/REPOLA_WINDOWS_SIGN_COMMAND/);
    expect(build({ RUNNER_OS: "Windows", REPOLA_WINDOWS_SIGN_COMMAND: "trusted-signer %1" }).bundle)
      .toEqual({
        createUpdaterArtifacts: true,
        targets: ["nsis"],
        windows: { signCommand: "trusted-signer %1" },
      });
  });

  it("configures complete API-key notarization and explicit macOS bundles", () => {
    const config = build({ RUNNER_OS: "macOS", APPLE_SIGNING_IDENTITY: "Developer ID Application: Example", APPLE_CERTIFICATE: "certificate", APPLE_CERTIFICATE_PASSWORD: "password", APPLE_API_KEY: "key-id", APPLE_API_ISSUER: "issuer", APPLE_API_KEY_PATH: "/runner/key.p8" });
    expect(config.bundle).toEqual({ createUpdaterArtifacts: true, targets: ["app", "dmg"], macOS: { signingIdentity: "Developer ID Application: Example" } });
  });

  it("isolates nightly builds from the stable feed", () => {
    const nightly = planRelease({ ...release, version: "0.1.0", tag: null, runNumber: "10", sourceRef: "refs/heads/main" });
    const config = buildReleaseConfig({ environment: environment({ GITHUB_REF: nightly.sourceRef }), packageJson: { version: nightly.version }, tauriConfig: { version: nightly.version }, cargoManifest: cargoManifest.replace('version = "0.1.0"', `version = "${nightly.version}"`), release: nightly });
    expect(config.plugins.updater.endpoints).toEqual(["https://austin-smith.github.io/Repola/updates/nightly.json"]);
  });
});
