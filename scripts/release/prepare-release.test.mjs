// @vitest-environment node
import { describe, expect, it } from "vitest";
import { buildReleaseConfig, buildInstallerConfig } from "./prepare-release.mjs";
import { planRelease } from "./release-metadata.mjs";
import { fileURLToPath } from "node:url";

const rawPublicKey = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
const publicKey = Buffer.from(`untrusted comment: minisign public key\n${rawPublicKey}\n`).toString("base64");
const packageJson = { version: "0.1.0" };
const tauriConfig = { version: "0.1.0" };
const cargoManifest = '[workspace]\nmembers = ["crates/repola-engine"]\n\n[workspace.package]\nversion = "0.1.0"\n\n[package]\nname = "repola"\nversion.workspace = true\n';
const release = planRelease({ version: "0.1.0", sha: "a".repeat(40), runId: "123", pubDate: "2026-09-30T09:17:00Z", sourceRef: "refs/tags/v0.1.0", tag: "v0.1.0" });
const windowsSigning = {
  RUNNER_OS: "Windows",
  AZURE_CLIENT_ID: "client-id",
  AZURE_TENANT_ID: "tenant-id",
  AZURE_SUBSCRIPTION_ID: "subscription-id",
  AZURE_TRUSTED_SIGNING_ENDPOINT: "https://wus2.codesigning.azure.net",
  AZURE_TRUSTED_SIGNING_ACCOUNT_NAME: "signing-account",
  AZURE_TRUSTED_SIGNING_CERTIFICATE_PROFILE_NAME: "public-trust",
  REPOLA_WINDOWS_SIGNING_DIRECTORY: "C:\\runner temp\\signing",
};

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
  it("permits a manual installer build from a branch while keeping release source restrictions", () => {
    const branchEnvironment = environment({ GITHUB_REF: "refs/heads/release-channels", ...windowsSigning });
    const config = buildInstallerConfig(branchEnvironment);
    expect(config.bundle.createUpdaterArtifacts).toBe(true);
    expect(config.bundle.windows.signCommand.cmd).toBe("pwsh");
    expect(config.plugins.updater).toEqual({ pubkey: publicKey, requireSignedVersion: true, endpoints: [] });
    expect(() => build({ GITHUB_REF: branchEnvironment.GITHUB_REF })).toThrow(/validated workflow metadata/);
  });

  it("requires the configured signing trust for installer tests", () => {
    expect(() => buildInstallerConfig(environment({ GITHUB_REPOSITORY: "another/repola" }))).toThrow(/pinned/);
    expect(() => buildInstallerConfig(environment({ TAURI_SIGNING_PRIVATE_KEY: "" }))).toThrow(/TAURI_SIGNING_PRIVATE_KEY/);
    expect(() => buildInstallerConfig(environment({ RUNNER_OS: "macOS" }))).toThrow(/APPLE_SIGNING_IDENTITY/);
    expect(() => buildInstallerConfig(environment({ RUNNER_OS: "Windows" }))).toThrow(/AZURE_CLIENT_ID/);
  });

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
      APPLE_SIGNING_IDENTITY: "A".repeat(40),
      APPLE_TEAM_ID: "ABCDEFGHIJ",
    })).toThrow(/APPLE_API_KEY/);
    expect(() => build({ RUNNER_OS: "macOS", APPLE_SIGNING_IDENTITY: "-" })).toThrow(/ad-hoc/);
    expect(() => build({ RUNNER_OS: "macOS", APPLE_SIGNING_IDENTITY: "Apple Development: Example" })).toThrow(/validated Developer ID/);
    expect(() => build({ RUNNER_OS: "macOS", APPLE_SIGNING_IDENTITY: "A".repeat(40), APPLE_TEAM_ID: "bad" })).toThrow(/validated Developer ID/);
  });

  it("requires native Windows signing and preserves signing command argument boundaries", () => {
    expect(() => build({ RUNNER_OS: "Windows" })).toThrow(/AZURE_CLIENT_ID/);
    expect(() => build({ ...windowsSigning, REPOLA_WINDOWS_SIGNING_DIRECTORY: "" })).toThrow(/REPOLA_WINDOWS_SIGNING_DIRECTORY/);
    expect(build(windowsSigning).bundle)
      .toEqual({
        createUpdaterArtifacts: true,
        targets: ["nsis"],
        windows: {
          signCommand: {
            cmd: "pwsh",
            args: ["-NoProfile", "-NonInteractive", "-File", fileURLToPath(new URL("./windows-signing.ps1", import.meta.url)), "-Operation", "sign", "-FilePath", "%1"],
          },
        },
      });
  });

  it.each(["http://wus2.codesigning.azure.net", "https://codesigning.azure.net.example.com", "https://wus2.codesigning.azure.net/other", "https://user@wus2.codesigning.azure.net", "https://wus2.codesigning.azure.net:8443"])("rejects an untrusted signing endpoint: %s", (endpoint) => {
    expect(() => build({ ...windowsSigning, AZURE_TRUSTED_SIGNING_ENDPOINT: endpoint })).toThrow(/regional HTTPS endpoint/);
  });

  it("configures complete API-key notarization and explicit macOS bundles", () => {
    const config = build({ RUNNER_OS: "macOS", APPLE_SIGNING_IDENTITY: "A".repeat(40), APPLE_TEAM_ID: "ABCDEFGHIJ", APPLE_API_KEY: "key-id", APPLE_API_ISSUER: "issuer", APPLE_API_KEY_PATH: "/runner/key.p8" });
    expect(config.bundle).toEqual({ createUpdaterArtifacts: true, targets: ["app", "dmg"], macOS: { signingIdentity: "A".repeat(40), hardenedRuntime: true } });
  });

  it("isolates nightly builds from the stable feed", () => {
    const nightly = planRelease({ ...release, version: "0.1.0", tag: null, runNumber: "10", sourceRef: "refs/heads/main" });
    const config = buildReleaseConfig({ environment: environment({ GITHUB_REF: nightly.sourceRef }), packageJson: { version: nightly.version }, tauriConfig: { version: nightly.version }, cargoManifest: cargoManifest.replace('version = "0.1.0"', `version = "${nightly.version}"`), release: nightly });
    expect(config.plugins.updater.endpoints).toEqual(["https://austin-smith.github.io/Repola/updates/nightly.json"]);
  });
});
