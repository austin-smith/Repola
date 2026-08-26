import { readFile, writeFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";

const repository = "austin-smith/repola";

function normalizedBase64(value) {
  return value.replace(/=+$/, "");
}

export function buildReleaseConfig({ environment, packageJson, tauriConfig, cargoManifest }) {
  const required = (name) => {
    const value = environment[name]?.trim();
    if (!value) throw new Error(`Release configuration requires ${name}.`);
    return value;
  };

  const cargoVersion = cargoManifest.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  const version = packageJson.version;
  if (!version || version !== tauriConfig.version || version !== cargoVersion) {
    throw new Error(
      `Release versions must match (package=${version ?? "missing"}, Tauri=${tauriConfig.version ?? "missing"}, Cargo=${cargoVersion ?? "missing"}).`,
    );
  }

  const githubRepository = required("GITHUB_REPOSITORY");
  if (githubRepository !== repository) {
    throw new Error(`Release builds are pinned to ${repository}; received ${githubRepository}.`);
  }
  if (required("GITHUB_REF_TYPE") !== "tag") {
    throw new Error("Release builds must run from an existing version tag.");
  }
  const refName = required("GITHUB_REF_NAME");
  if (refName !== `v${version}`) {
    throw new Error(`Release tag ${refName} does not match application version v${version}.`);
  }

  const publicKey = required("REPOLA_SIGNING_PUBLIC_KEY");
  required("TAURI_SIGNING_PRIVATE_KEY");
  required("TAURI_SIGNING_PRIVATE_KEY_PASSWORD");
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(publicKey)) {
    throw new Error("REPOLA_SIGNING_PUBLIC_KEY must be the one-line base64 public key emitted by the Tauri signer.");
  }
  const decodedPublicKeyDocument = Buffer.from(publicKey, "base64");
  if (
    normalizedBase64(decodedPublicKeyDocument.toString("base64")) !== normalizedBase64(publicKey)
  ) {
    throw new Error("REPOLA_SIGNING_PUBLIC_KEY is not canonical base64.");
  }
  const publicKeyLines = decodedPublicKeyDocument.toString("utf8").trim().split(/\r?\n/);
  const rawPublicKey = publicKeyLines.length === 2 ? Buffer.from(publicKeyLines[1], "base64") : Buffer.alloc(0);
  if (
    publicKeyLines.length !== 2
    || !publicKeyLines[0].startsWith("untrusted comment:")
    || rawPublicKey.length !== 42
    || rawPublicKey.subarray(0, 2).toString("ascii") !== "Ed"
    || normalizedBase64(rawPublicKey.toString("base64")) !== normalizedBase64(publicKeyLines[1] ?? "")
  ) {
    throw new Error("REPOLA_SIGNING_PUBLIC_KEY is not a Tauri-encoded modern Minisign public key.");
  }

  const releaseConfig = {
    bundle: {
      createUpdaterArtifacts: true,
    },
    plugins: {
      updater: {
        pubkey: publicKey,
        endpoints: [`https://github.com/${repository}/releases/latest/download/latest.json`],
      },
    },
  };

  const runnerOs = required("RUNNER_OS");
  if (runnerOs === "macOS") {
    const signingIdentity = required("APPLE_SIGNING_IDENTITY");
    required("APPLE_CERTIFICATE");
    required("APPLE_CERTIFICATE_PASSWORD");
    const hasAppleId = Boolean(
      environment.APPLE_ID?.trim() && environment.APPLE_PASSWORD?.trim() && environment.APPLE_TEAM_ID?.trim(),
    );
    const hasApiKey = Boolean(
      environment.APPLE_API_KEY?.trim() && environment.APPLE_API_ISSUER?.trim() && environment.APPLE_API_KEY_PATH?.trim(),
    );
    if (!hasAppleId && !hasApiKey) {
      throw new Error("macOS release builds require either Apple ID notarization credentials or App Store Connect API credentials.");
    }
    releaseConfig.bundle.macOS = { signingIdentity };
  } else if (runnerOs === "Windows") {
    releaseConfig.bundle.windows = { signCommand: required("REPOLA_WINDOWS_SIGN_COMMAND") };
  } else if (runnerOs !== "Linux") {
    throw new Error(`Unsupported release runner operating system: ${runnerOs}.`);
  }

  return releaseConfig;
}

export async function prepareRelease(environment = process.env) {
  const packageJson = JSON.parse(await readFile(new URL("../package.json", import.meta.url), "utf8"));
  const tauriConfig = JSON.parse(await readFile(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"));
  const cargoManifest = await readFile(new URL("../src-tauri/Cargo.toml", import.meta.url), "utf8");
  const releaseConfig = buildReleaseConfig({ environment, packageJson, tauriConfig, cargoManifest });
  await writeFile(
    new URL("../src-tauri/tauri.release.conf.json", import.meta.url),
    `${JSON.stringify(releaseConfig, null, 2)}\n`,
    { mode: 0o600 },
  );
  return releaseConfig;
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) {
  await prepareRelease();
}
