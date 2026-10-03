// @vitest-environment node
import { spawnSync } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

const pwsh = process.env.REPOLA_TEST_PWSH ?? "pwsh";
const available = !spawnSync(pwsh, ["-NoProfile", "-NonInteractive", "-Command", "$PSVersionTable.PSVersion.ToString()"], { encoding: "utf8" }).error;
if (!available && process.platform === "win32") throw new Error("Windows signing tests require PowerShell.");
const quote = (value) => `'${value.replaceAll("'", "''")}'`;
const script = fileURLToPath(new URL("./windows-signing.ps1", import.meta.url));

describe.skipIf(!available)("Windows release signing", () => {
  let directory;
  let file;
  beforeEach(async () => {
    directory = await mkdtemp(join(tmpdir(), "repola signing "));
    file = join(directory, "unsigned [fixture], 'file'.exe");
    await writeFile(file, "unsigned fixture");
  });
  afterEach(async () => { await rm(directory, { recursive: true, force: true }); });

  function run(command, environment = {}) {
    return spawnSync(pwsh, ["-NoProfile", "-NonInteractive", "-Command", `. ${quote(script)}; ${command}`], {
      encoding: "utf8",
      env: { ...process.env, REPOLA_WINDOWS_SIGNING_DIRECTORY: directory, ...environment },
      timeout: 30_000,
    });
  }

  it("passes literal file paths intact to native signing and verification", () => {
    const result = run(`
      $script:calls = [System.Collections.Generic.List[object]]::new()
      function Invoke-SigningTool([string[]]$Arguments) { $script:calls.Add($Arguments) }
      function Get-AuthenticodeSignature { param($LiteralPath); return [pscustomobject]@{ Status = 'Valid'; SignatureType = 'Authenticode'; TimeStamperCertificate = 'timestamp' } }
      Sign-WindowsFile ${quote(file)}
      ConvertTo-Json -InputObject $script:calls -Compress
    `);
    expect(result.status, result.stderr).toBe(0);
    const [sign, verify] = JSON.parse(result.stdout.trim());
    expect(sign).toEqual(["sign", "/fd", "SHA256", "/tr", "http://timestamp.acs.microsoft.com", "/td", "SHA256", "/dlib", join(directory, "microsoft.artifactsigning.client", "bin", "x64", "Azure.CodeSigning.Dlib.dll"), "/dmdf", join(directory, "metadata.json"), "/d", "Repola", file]);
    expect(verify).toEqual(["verify", "/pa", "/all", "/tw", file]);
  });

  it.each([
    ["HashMismatch", "Authenticode", "timestamp", /Invalid embedded/],
    ["NotTrusted", "Authenticode", "timestamp", /Invalid embedded/],
    ["NotSigned", "None", "timestamp", /Invalid embedded/],
    ["Valid", "Catalog", "timestamp", /Invalid embedded/],
    ["Valid", "Authenticode", null, /Missing Authenticode timestamp/],
  ])("rejects signature status %s, type %s, timestamp %s", (status, type, timestamp, message) => {
    const result = run(`
      function Invoke-SigningTool { throw 'Unexpected SignTool invocation' }
      function Get-AuthenticodeSignature { param($LiteralPath); return [pscustomobject]@{ Status = ${quote(status)}; SignatureType = ${quote(type)}; TimeStamperCertificate = ${timestamp === null ? "$null" : quote(timestamp)} } }
      Assert-WindowsSignature ${quote(file)}
    `);
    expect(result.status).not.toBe(0);
    expect(result.stderr).toMatch(message);
  });

  it("rejects native verification warnings and failures", () => {
    const result = run(`
      function Get-SigningToolPath { return ${quote(pwsh)} }
      Invoke-SigningTool -Arguments @('-NoProfile', '-NonInteractive', '-Command', 'exit 2')
    `);
    expect(result.status).not.toBe(0);
    expect(result.stderr).toMatch(/SignTool failed with exit code 2/);
  });

  it("rejects an altered signing package before extraction", () => {
    const result = run(`
      function Invoke-WebRequest { param($Uri, $OutFile); [IO.File]::WriteAllText($OutFile, 'tampered package') }
      function Expand-Archive { throw 'Unexpected extraction' }
      Install-SigningPackage 'package' '1.0' ${quote("0".repeat(64))} ${quote(directory)}
    `);
    expect(result.status).not.toBe(0);
    expect(result.stderr).toMatch(/unexpected SHA256 digest/);
  });

  it.runIf(process.platform === "win32")("rejects a real unsigned file using Windows Authenticode validation", () => {
    const result = run(`Assert-WindowsSignature ${quote(file)}`);
    expect(result.status).not.toBe(0);
    expect(result.stderr).toMatch(/Invalid embedded Authenticode signature/);
  });
});
