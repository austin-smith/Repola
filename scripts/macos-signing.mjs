import { execFile } from "node:child_process";
import { createPrivateKey, randomBytes } from "node:crypto";
import { appendFile, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { isAbsolute, join } from "node:path";
import { promisify } from "node:util";
import { isMain, root } from "./release-utils.mjs";

const execute = promisify(execFile);

// execFile errors include the command arguments, which can contain passwords.
export async function runAppleTool(program, args) {
  try {
    const { stdout } = await execute(program, args, { timeout: args[0] === "notarytool" ? 1_860_000 : 120_000 });
    return stdout;
  } catch {
    throw new Error(`${program} ${args[0]} failed; credential arguments and output were suppressed.`);
  }
}

function required(environment, name) {
  const value = environment[name];
  if (!value?.trim()) throw new Error(`macOS signing requires ${name}.`);
  return value;
}

export function decodeSecret(value, name) {
  const encoded = value.replace(/\s/g, "");
  const decoded = Buffer.from(encoded, "base64");
  if (!encoded || decoded.toString("base64") !== encoded) {
    throw new Error(`${name} must contain canonical base64.`);
  }
  return decoded;
}

export function signingCredentials(environment) {
  const certificate = decodeSecret(required(environment, "APPLE_CERT_P12_BASE64"), "APPLE_CERT_P12_BASE64");
  const password = required(environment, "APPLE_CERT_PASSWORD");
  const keyId = required(environment, "APPLE_API_KEY_ID").trim();
  const issuer = required(environment, "APPLE_API_ISSUER_ID").trim();
  if (!/^[A-Z0-9]{10}$/.test(keyId) || !/^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/i.test(issuer)) {
    throw new Error("Notarization requires a team App Store Connect API key ID and issuer UUID.");
  }
  const privateKey = decodeSecret(required(environment, "APPLE_API_PRIVATE_KEY_BASE64"), "APPLE_API_PRIVATE_KEY_BASE64");
  try {
    if (!privateKey.toString("utf8").trim().startsWith("-----BEGIN PRIVATE KEY-----")) throw new Error("Expected PKCS#8 PEM.");
    const key = createPrivateKey({ key: privateKey, format: "pem", type: "pkcs8" });
    if (key.asymmetricKeyType !== "ec" || key.asymmetricKeyDetails?.namedCurve !== "prime256v1") {
      throw new Error("Unexpected key type.");
    }
  } catch {
    throw new Error("APPLE_API_PRIVATE_KEY_BASE64 must encode an App Store Connect P-256 private key (.p8).");
  }
  return { certificate, password, keyId, issuer, privateKey };
}

export function developerIdIdentity(output) {
  const identities = [...output.matchAll(/^\s*\d+\) ([A-F0-9]{40}) "(Developer ID Application: .+ \(([A-Z0-9]{10})\))"\s*$/gm)];
  if (identities.length !== 1) {
    throw new Error("The .p12 must contain exactly one valid Developer ID Application signing identity with its private key.");
  }
  const [, fingerprint, name, teamId] = identities[0];
  return { fingerprint, name, teamId };
}

function signingDirectory(environment, platform) {
  if (platform !== "darwin") throw new Error("Apple signing commands require macOS.");
  const temporaryDirectory = required(environment, "RUNNER_TEMP");
  if (!isAbsolute(temporaryDirectory)) throw new Error("RUNNER_TEMP must be an absolute path.");
  return join(temporaryDirectory, "repola-macos-signing");
}

export async function cleanupMacosSigning({ environment = process.env, platform = process.platform, run = runAppleTool } = {}) {
  const directory = signingDirectory(environment, platform);
  let state;
  try {
    state = JSON.parse(await readFile(join(directory, "state.json"), "utf8"));
  } catch (error) {
    if (error.code === "ENOENT") return;
    throw error;
  }
  try {
    if (state.keychainCreated) {
      await run("security", ["delete-keychain", join(directory, "signing.keychain-db")]);
    }
  } finally {
    try {
      await run("security", ["list-keychains", "-d", "user", "-s", ...state.keychains]);
    } finally {
      await rm(directory, { recursive: true });
    }
  }
}

export async function prepareMacosSigning({ environment = process.env, platform = process.platform, run = runAppleTool } = {}) {
  const directory = signingDirectory(environment, platform);
  const credentials = signingCredentials(environment);
  const githubEnv = required(environment, "GITHUB_ENV");
  const keychains = (await run("security", ["list-keychains", "-d", "user"]))
    .split(/\r?\n/).map((line) => line.trim().replace(/^"(.*)"$/, "$1")).filter(Boolean);
  await mkdir(directory, { mode: 0o700 });
  const state = { keychains, keychainCreated: false };
  const saveState = () => writeFile(join(directory, "state.json"), JSON.stringify(state), { mode: 0o600 });
  const keychain = join(directory, "signing.keychain-db");
  const certificate = join(directory, "certificate.p12");
  const privateKey = join(directory, "AuthKey.p8");
  const keychainPassword = randomBytes(32).toString("hex");
  try {
    await saveState();
    await writeFile(certificate, credentials.certificate, { mode: 0o600 });
    await writeFile(privateKey, credentials.privateKey, { mode: 0o600 });
    await run("security", ["create-keychain", "-p", keychainPassword, keychain]);
    state.keychainCreated = true;
    await saveState();
    await run("security", ["set-keychain-settings", "-lut", "21600", keychain]);
    await run("security", ["unlock-keychain", "-p", keychainPassword, keychain]);
    await run("security", ["import", certificate, "-k", keychain, "-P", credentials.password, "-T", "/usr/bin/codesign"]);
    await run("security", ["set-key-partition-list", "-S", "apple-tool:,apple:,codesign:", "-s", "-k", keychainPassword, keychain]);
    const identity = developerIdIdentity(await run("security", ["find-identity", "-v", "-p", "codesigning", keychain]));
    // Preserve access to intermediate certificates; the exact fingerprint selects this job's signing key.
    await run("security", ["list-keychains", "-d", "user", "-s", keychain, ...keychains]);
    await rm(certificate);
    const variables = {
      APPLE_SIGNING_IDENTITY: identity.fingerprint,
      APPLE_TEAM_ID: identity.teamId,
      APPLE_API_KEY: credentials.keyId,
      APPLE_API_ISSUER: credentials.issuer,
      APPLE_API_KEY_PATH: privateKey,
    };
    if (Object.values(variables).some((value) => /[\r\n]/.test(value))) throw new Error("Signing environment values must be single-line.");
    await appendFile(githubEnv, Object.entries(variables).map(([name, value]) => `${name}=${value}\n`).join(""));
    return variables;
  } catch (error) {
    await cleanupMacosSigning({ environment, platform, run });
    throw error;
  }
}

export async function verifyMacosRelease({ environment = process.env, platform = process.platform, run = runAppleTool, bundleDirectory } = {}) {
  signingDirectory(environment, platform);
  const target = required(environment, "REPOLA_TARGET");
  if (!["aarch64-apple-darwin", "x86_64-apple-darwin"].includes(target)) throw new Error("Unsupported macOS release target.");
  const bundle = bundleDirectory ?? join(root, "src-tauri", "target", target, "release", "bundle");
  const app = join(bundle, "macos", "Repola.app");
  await run("codesign", ["--verify", "--deep", "--strict", app]);
  await run("xcrun", ["stapler", "validate", app]);
  await run("spctl", ["--assess", "--type", "execute", app]);
  const dmgs = (await readdir(join(bundle, "dmg"))).filter((name) => name.endsWith(".dmg"));
  if (dmgs.length !== 1) throw new Error("Expected exactly one macOS release DMG.");
  const dmg = join(bundle, "dmg", dmgs[0]);
  await run("codesign", ["--verify", "--strict", dmg]);
  // Tauri notarizes the app, but only signs the DMG. Notarize and staple the final installer too.
  const result = JSON.parse(await run("xcrun", ["notarytool", "submit", dmg,
    "--key", required(environment, "APPLE_API_KEY_PATH"), "--key-id", required(environment, "APPLE_API_KEY"),
    "--issuer", required(environment, "APPLE_API_ISSUER"), "--wait", "--timeout", "30m", "--output-format", "json"]));
  if (result.status !== "Accepted") throw new Error(`DMG notarization was not accepted (submission ${result.id}). Inspect it with notarytool log.`);
  await run("xcrun", ["stapler", "staple", dmg]);
  await run("xcrun", ["stapler", "validate", dmg]);
  await run("spctl", ["--assess", "--type", "open", "--context", "context:primary-signature", dmg]);
}

if (isMain(import.meta.url)) {
  const commands = { prepare: prepareMacosSigning, verify: verifyMacosRelease, cleanup: cleanupMacosSigning };
  const command = commands[process.argv[2]];
  if (!command) throw new Error("Usage: node scripts/macos-signing.mjs prepare|verify|cleanup");
  await command();
}
