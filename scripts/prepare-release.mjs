import { readFile, writeFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { readJson, repository, updateBaseUrl, validateRelease } from "./release-utils.mjs";

function normalizedBase64(value) {
  return value.replace(/=+$/, "");
}

export function buildReleaseConfig({ environment, packageJson, tauriConfig, cargoManifest, release }) {
  validateRelease(release);
  const required = (name) => {
    const value = environment[name]?.trim();
    if (!value) throw new Error(`Release configuration requires ${name}.`);
    return value;
  };

  const workspacePackage = cargoManifest.match(/^\[workspace\.package\]\s*\n([\s\S]*?)(?=^\[|(?![\s\S]))/m)?.[1] ?? "";
  const cargoVersion = workspacePackage.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  const version = packageJson.version;
  if (!version || version !== tauriConfig.version || version !== cargoVersion || version !== release.version) {
    throw new Error(
      `Release versions must match (package=${version ?? "missing"}, Tauri=${tauriConfig.version ?? "missing"}, Cargo=${cargoVersion ?? "missing"}).`,
    );
  }

  const githubRepository = required("GITHUB_REPOSITORY");
  if (githubRepository.toLowerCase() !== repository.toLowerCase()) {
    throw new Error(`Release builds are pinned to ${repository}; received ${githubRepository}.`);
  }
  if (required("GITHUB_REF") !== release.sourceRef || required("REPOLA_RELEASE_SHA") !== release.sha) {
    throw new Error("Release source must match the validated workflow metadata.");
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
        requireSignedVersion: true,
        endpoints: [`${updateBaseUrl}/${release.channel}.json`],
      },
    },
  };

  const runnerOs = required("RUNNER_OS");
  if (runnerOs === "macOS") {
    const signingIdentity = required("APPLE_SIGNING_IDENTITY");
    if (!/^[A-F0-9]{40}$/.test(signingIdentity) || !/^[A-Z0-9]{10}$/.test(required("APPLE_TEAM_ID"))) {
      throw new Error("macOS releases require the validated Developer ID certificate fingerprint and team, not ad-hoc signing.");
    }
    required("APPLE_API_KEY");
    required("APPLE_API_ISSUER");
    required("APPLE_API_KEY_PATH");
    releaseConfig.bundle.macOS = { signingIdentity, hardenedRuntime: true };
    releaseConfig.bundle.targets = ["app", "dmg"];
  } else if (runnerOs === "Windows") {
    releaseConfig.bundle.targets = ["nsis"];
  } else if (runnerOs === "Linux") {
    releaseConfig.bundle.targets = ["appimage", "deb"];
  } else {
    throw new Error(`Unsupported release runner operating system: ${runnerOs}.`);
  }

  return releaseConfig;
}

export async function prepareRelease(environment = process.env, metadataPath = new URL("../.release/release.json", import.meta.url)) {
  const release = await readJson(metadataPath);
  const packageJson = JSON.parse(await readFile(new URL("../package.json", import.meta.url), "utf8"));
  const tauriConfig = JSON.parse(await readFile(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"));
  const cargoManifest = await readFile(new URL("../src-tauri/Cargo.toml", import.meta.url), "utf8");
  const releaseConfig = buildReleaseConfig({ environment, packageJson, tauriConfig, cargoManifest, release });
  await writeFile(
    new URL("../src-tauri/tauri.release.conf.json", import.meta.url),
    `${JSON.stringify(releaseConfig, null, 2)}\n`,
    { mode: 0o600 },
  );
  return releaseConfig;
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) {
  await prepareRelease(process.env, process.argv[2]);
}
