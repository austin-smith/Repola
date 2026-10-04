import { describe, expect, it } from "vitest";
import { buildIdentities, resolveBuildChannel } from "./build-identity";

describe("build identity", () => {
  it("isolates every channel's identity and preserves the existing stable profile", () => {
    expect(buildIdentities.stable).toMatchObject({ name: "Repola", channelLabel: null, identifier: "net.crapshack.repola", binaryName: "repola" });
    const identities = Object.values(buildIdentities);
    expect(new Set(identities.map(({ identifier }) => identifier)).size).toBe(identities.length);
    expect(new Set(identities.map(({ binaryName }) => binaryName)).size).toBe(identities.length);
  });

  it("selects native identity without needing a separate frontend override", () => {
    expect(resolveBuildChannel("net.crapshack.repola.nightly", undefined)).toBe("nightly");
    expect(resolveBuildChannel("net.crapshack.repola", "stable")).toBe("stable");
    expect(resolveBuildChannel(undefined, undefined)).toBe("development");
    expect(resolveBuildChannel(undefined, "nightly")).toBe("nightly");
  });

  it("rejects ambiguous or contradictory metadata instead of shipping mismatched branding", () => {
    expect(() => resolveBuildChannel("net.crapshack.repola", "nightly")).toThrow(/does not match/);
    expect(() => resolveBuildChannel("net.crapshack.repola.dev", "stable")).toThrow(/does not match/);
    expect(() => resolveBuildChannel(undefined, "preview")).toThrow(/Unknown/);
    expect(() => resolveBuildChannel("other.app", undefined)).toThrow(/Unknown/);
  });
});
