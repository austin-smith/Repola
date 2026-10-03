// @vitest-environment node
import { generateKeyPairSync } from "node:crypto";
import { mkdtemp, mkdir, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanupMacosSigning, decodeSecret, developerIdIdentity, prepareMacosSigning, runAppleTool, signingCredentials, verifyMacosRelease } from "./macos-signing.mjs";

const fingerprint = "A".repeat(40);
const identity = `  1) ${fingerprint} "Developer ID Application: Example (ABCDEFGHIJ)"\n     1 valid identities found\n`;
const privateKey = generateKeyPairSync("ec", { namedCurve: "prime256v1", privateKeyEncoding: { type: "pkcs8", format: "pem" }, publicKeyEncoding: { type: "spki", format: "pem" } }).privateKey;
const secrets = {
  APPLE_CERT_P12_BASE64: Buffer.from("certificate-fixture").toString("base64"),
  APPLE_CERT_PASSWORD: "  password with spaces $() `literal`  ",
  APPLE_API_KEY_ID: "0123456789",
  APPLE_API_ISSUER_ID: "12345678-1234-1234-1234-123456789abc",
  APPLE_API_PRIVATE_KEY_BASE64: Buffer.from(privateKey).toString("base64"),
};
const directories = [];
afterEach(async () => {
  for (const directory of directories.splice(0)) await rm(directory, { recursive: true, force: true });
});

async function fixture() {
  const temporaryDirectory = await mkdtemp(join(tmpdir(), "repola signing test "));
  directories.push(temporaryDirectory);
  const githubEnv = join(temporaryDirectory, "github-env");
  await writeFile(githubEnv, "EXISTING=value\n");
  const environment = { ...secrets, RUNNER_TEMP: temporaryDirectory, GITHUB_ENV: githubEnv };
  const run = vi.fn(async (program, args) => {
    if (program === "security" && args[0] === "list-keychains" && !args.includes("-s")) return '    "/original keychains/login.keychain-db"\n';
    if (args[0] === "find-identity") return identity;
    if (args[0] === "notarytool") return JSON.stringify({ status: "Accepted", id: "submission" });
    return "";
  });
  return { environment, platform: "darwin", run, directory: join(temporaryDirectory, "repola-macos-signing") };
}

describe("Apple signing credentials", () => {
  it("uses the shared secret convention, preserves password bytes, and accepts wrapped base64", () => {
    expect(signingCredentials(secrets)).toMatchObject({ password: secrets.APPLE_CERT_PASSWORD, keyId: secrets.APPLE_API_KEY_ID, issuer: secrets.APPLE_API_ISSUER_ID, privateKey: Buffer.from(privateKey) });
    expect(decodeSecret("YW\nJj\r\n", "certificate").toString()).toBe("abc");
    expect(decodeSecret("YQ==", "certificate").toString()).toBe("a");
  });

  it.each(Object.keys(secrets))("requires %s", (name) => {
    expect(() => signingCredentials({ ...secrets, [name]: "" })).toThrow(name);
  });

  it("rejects malformed base64, API identifiers, and private keys without exposing values", () => {
    for (const encoded of ["", "YQ", "YR==", "YQ==!!!"]) expect(() => decodeSecret(encoded, "secret")).toThrow(/canonical base64/);
    expect(() => signingCredentials({ ...secrets, APPLE_API_KEY_ID: "bad\ninjection" })).toThrow(/team App Store Connect/);
    expect(() => signingCredentials({ ...secrets, APPLE_API_ISSUER_ID: "not-an-issuer" })).toThrow(/issuer UUID/);
    expect(() => signingCredentials({ ...secrets, APPLE_API_PRIVATE_KEY_BASE64: Buffer.from("not a private key").toString("base64") })).toThrow(/P-256 private key/);
    const wrongCurve = generateKeyPairSync("ec", { namedCurve: "secp384r1", privateKeyEncoding: { type: "pkcs8", format: "pem" }, publicKeyEncoding: { type: "spki", format: "pem" } }).privateKey;
    expect(() => signingCredentials({ ...secrets, APPLE_API_PRIVATE_KEY_BASE64: Buffer.from(wrongCurve).toString("base64") })).toThrow(/P-256 private key/);
  });

  it("selects exactly one valid Developer ID identity and derives the certificate fingerprint and team", () => {
    expect(developerIdIdentity(identity)).toEqual({ fingerprint, name: "Developer ID Application: Example (ABCDEFGHIJ)", teamId: "ABCDEFGHIJ" });
    for (const output of ["0 valid identities found", identity.replace("Developer ID Application", "Apple Development"), identity.replace("ABCDEFGHIJ", "bad"), identity + identity]) {
      expect(() => developerIdIdentity(output)).toThrow(/exactly one valid Developer ID/);
    }
  });

  it("suppresses subprocess errors that would otherwise disclose password arguments", async () => {
    const { environment } = await fixture();
    await expect(runAppleTool(join(environment.RUNNER_TEMP, "missing-tool"), ["import", "private-password"])).rejects.toThrow(/credential arguments and output were suppressed/);
    await runAppleTool(join(environment.RUNNER_TEMP, "missing-tool"), ["import", "private-password"]).catch((error) => expect(error.message).not.toContain("private-password"));
  });
});

describe("isolated signing keychain", () => {
  it("selects the imported certificate by fingerprint, publishes only derived values, and cleans up", async () => {
    const options = await fixture();
    const variables = await prepareMacosSigning(options);
    expect(variables).toEqual({ APPLE_SIGNING_IDENTITY: fingerprint, APPLE_TEAM_ID: "ABCDEFGHIJ", APPLE_API_KEY: secrets.APPLE_API_KEY_ID, APPLE_API_ISSUER: secrets.APPLE_API_ISSUER_ID, APPLE_API_KEY_PATH: join(options.directory, "AuthKey.p8") });
    expect(await readFile(variables.APPLE_API_KEY_PATH, "utf8")).toBe(privateKey);
    await expect(stat(join(options.directory, "certificate.p12"))).rejects.toMatchObject({ code: "ENOENT" });
    if (process.platform !== "win32") {
      expect((await stat(options.directory)).mode & 0o777).toBe(0o700);
      expect((await stat(variables.APPLE_API_KEY_PATH)).mode & 0o777).toBe(0o600);
    }
    const published = await readFile(options.environment.GITHUB_ENV, "utf8");
    expect(published).toContain(`EXISTING=value\nAPPLE_SIGNING_IDENTITY=${fingerprint}\n`);
    expect(published).not.toContain("PRIVATE KEY");
    expect(published).not.toContain(secrets.APPLE_CERT_PASSWORD);
    const importArgs = options.run.mock.calls.find(([, args]) => args[0] === "import")[1];
    expect(importArgs[importArgs.indexOf("-P") + 1]).toBe(secrets.APPLE_CERT_PASSWORD);
    expect(options.run).toHaveBeenCalledWith("security", ["list-keychains", "-d", "user", "-s", join(options.directory, "signing.keychain-db"), "/original keychains/login.keychain-db"]);
    await cleanupMacosSigning(options);
    expect(options.run).toHaveBeenCalledWith("security", ["delete-keychain", join(options.directory, "signing.keychain-db")]);
    expect(options.run).toHaveBeenLastCalledWith("security", ["list-keychains", "-d", "user", "-s", "/original keychains/login.keychain-db"]);
    await expect(stat(options.directory)).rejects.toMatchObject({ code: "ENOENT" });
    await cleanupMacosSigning(options);
  });

  it.each(["import", "find-identity"])("restores the original keychains and removes credentials after %s fails", async (operation) => {
    const options = await fixture();
    const normalRun = options.run.getMockImplementation();
    options.run.mockImplementation(async (program, args) => {
      if (args[0] === operation) throw new Error("simulated signing failure");
      return normalRun(program, args);
    });
    await expect(prepareMacosSigning(options)).rejects.toThrow(/simulated signing failure/);
    expect(options.run).toHaveBeenCalledWith("security", ["delete-keychain", join(options.directory, "signing.keychain-db")]);
    expect(options.run).toHaveBeenLastCalledWith("security", ["list-keychains", "-d", "user", "-s", "/original keychains/login.keychain-db"]);
    await expect(stat(options.directory)).rejects.toMatchObject({ code: "ENOENT" });
    expect(await readFile(options.environment.GITHUB_ENV, "utf8")).toBe("EXISTING=value\n");
  });

  it("rejects missing credentials and unsupported hosts before changing the keychain", async () => {
    const options = await fixture();
    await expect(prepareMacosSigning({ ...options, platform: "linux" })).rejects.toThrow(/require macOS/);
    await expect(prepareMacosSigning({ ...options, environment: { ...options.environment, APPLE_CERT_PASSWORD: "" } })).rejects.toThrow(/APPLE_CERT_PASSWORD/);
    expect(options.run).not.toHaveBeenCalled();
    expect(await readdir(options.environment.RUNNER_TEMP)).toEqual(["github-env"]);
  });

  it("refuses to overwrite an existing signing directory", async () => {
    const options = await fixture();
    await mkdir(options.directory);
    await writeFile(join(options.directory, "sentinel"), "keep");
    await expect(prepareMacosSigning(options)).rejects.toMatchObject({ code: "EEXIST" });
    expect(await readFile(join(options.directory, "sentinel"), "utf8")).toBe("keep");
    expect(options.run.mock.calls.some(([, args]) => args[0] === "create-keychain")).toBe(false);
  });
});

describe("final macOS installer verification", () => {
  async function releaseFixture() {
    const options = await fixture();
    const bundleDirectory = join(options.environment.RUNNER_TEMP, "bundle");
    await mkdir(join(bundleDirectory, "dmg"), { recursive: true });
    await writeFile(join(bundleDirectory, "dmg", "Repola.dmg"), "dmg fixture");
    return { ...options, bundleDirectory, environment: { ...options.environment, REPOLA_TARGET: "aarch64-apple-darwin", APPLE_API_KEY_PATH: join(options.directory, "AuthKey.p8"), APPLE_API_KEY: secrets.APPLE_API_KEY_ID, APPLE_API_ISSUER: secrets.APPLE_API_ISSUER_ID } };
  }

  it("requires app verification and notarizes the final DMG before stapling and assessing it", async () => {
    const options = await releaseFixture();
    await verifyMacosRelease(options);
    const dmg = join(options.bundleDirectory, "dmg", "Repola.dmg");
    expect(options.run).toHaveBeenCalledWith("xcrun", ["stapler", "validate", join(options.bundleDirectory, "macos", "Repola.app")]);
    expect(options.run.mock.calls.find(([, args]) => args[0] === "notarytool")[1]).toContain(dmg);
    expect(options.run).toHaveBeenCalledWith("xcrun", ["stapler", "staple", dmg]);
    expect(options.run).toHaveBeenLastCalledWith("spctl", ["--assess", "--type", "open", "--context", "context:primary-signature", dmg]);
  });

  it("stops on rejected notarization without stapling or assessing the DMG", async () => {
    const options = await releaseFixture();
    const normalRun = options.run.getMockImplementation();
    options.run.mockImplementation(async (program, args) => args[0] === "notarytool" ? '{"status":"Invalid","id":"failed-submission"}' : normalRun(program, args));
    await expect(verifyMacosRelease(options)).rejects.toThrow(/not accepted.*failed-submission/);
    expect(options.run.mock.calls.some(([, args]) => args[0] === "stapler" && args[1] === "staple")).toBe(false);
  });

  it("stops before notarization when the app fails Gatekeeper assessment", async () => {
    const options = await releaseFixture();
    const normalRun = options.run.getMockImplementation();
    options.run.mockImplementation(async (program, args) => {
      if (program === "spctl") throw new Error("Gatekeeper rejected the app");
      return normalRun(program, args);
    });
    await expect(verifyMacosRelease(options)).rejects.toThrow(/Gatekeeper rejected/);
    expect(options.run.mock.calls.some(([, args]) => args[0] === "notarytool")).toBe(false);
  });
});
