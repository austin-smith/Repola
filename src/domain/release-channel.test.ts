import { describe, expect, it } from "vitest";
import { acceptsReleaseVersion } from "./release-channel";

describe("update channel isolation", () => {
  it("keeps stable and nightly updates in their installed channel", () => {
    expect(acceptsReleaseVersion("stable", "0.2.0")).toBe(true);
    expect(acceptsReleaseVersion("stable", "0.2.0-nightly.123")).toBe(false);
    expect(acceptsReleaseVersion("nightly", "0.2.0-nightly.123")).toBe(true);
    expect(acceptsReleaseVersion("nightly", "0.2.0")).toBe(false);
    expect(acceptsReleaseVersion("development", "0.2.0")).toBe(false);
    expect(acceptsReleaseVersion("development", "0.2.0-nightly.123")).toBe(false);
  });

  it.each(["0.2.0-beta.1", "0.2.0+other", "0.2.0-nightly.0", "0.2.0-nightly.01", "0.2.0\n"])("rejects unsupported release offer %s", (version) => {
    expect(acceptsReleaseVersion("stable", version)).toBe(false);
    expect(acceptsReleaseVersion("nightly", version)).toBe(false);
  });
});
