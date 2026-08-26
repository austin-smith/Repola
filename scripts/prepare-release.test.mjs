import { describe, expect, it } from "vitest";
import { buildReleaseConfig } from "./prepare-release.mjs";

const rawPublicKey = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
const publicKey = Buffer.from(`untrusted comment: minisign public key\n${rawPublicKey}\n`).toString("base64");
const packageJson = { version: "0.1.0" };
const tauriConfig = { version: "0.1.0" };
const cargoManifest = '[package]\nname = "repola"\nversion = "0.1.0"\n';

function environment(overrides = {}) {
  return {
    GITHUB_REPOSITORY: "austin-smith/repola",
    GITHUB_REF_TYPE: "tag",
    GITHUB_REF_NAME: "v0.1.0",
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
  });
}

describe("release trust preparation", () => {
  it("generates the pinned updater channel for a tagged Linux release", () => {
    expect(build()).toEqual({
      bundle: {
        createUpdaterArtifacts: true,
      },
      plugins: {
        updater: {
          pubkey: publicKey,
          endpoints: ["https://github.com/austin-smith/repola/releases/latest/download/latest.json"],
        },
      },
    });
  });

  it("rejects branch and mismatched-tag release attempts", () => {
    expect(() => build({ GITHUB_REF_TYPE: "branch", GITHUB_REF_NAME: "main" })).toThrow(/existing version tag/);
    expect(() => build({ GITHUB_REF_NAME: "v0.2.0" })).toThrow(/does not match/);
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
  });

  it("requires the protected Windows signing command", () => {
    expect(() => build({ RUNNER_OS: "Windows" })).toThrow(/REPOLA_WINDOWS_SIGN_COMMAND/);
    expect(build({ RUNNER_OS: "Windows", REPOLA_WINDOWS_SIGN_COMMAND: "trusted-signer %1" }).bundle)
      .toEqual({
        createUpdaterArtifacts: true,
        windows: { signCommand: "trusted-signer %1" },
      });
  });
});
