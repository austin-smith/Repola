import { describe, expect, it } from "vitest";
import { ageInDays, formatAge, formatBytes, formatMeasuredBytes, shortPath, shortSha } from "./format";

describe("worktree formatting", () => {
  it("formats allocated bytes with binary units", () => {
    expect(formatBytes(14_412_230_656)).toBe("13.4 GB");
    expect(formatBytes(null)).toBe("—");
    expect(formatMeasuredBytes(1_048_576, true)).toBe("≥ 1 MB");
  });

  it("uses stable whole-day age buckets", () => {
    const now = Date.UTC(2026, 7, 19);
    expect(ageInDays(now - 90 * 86_400_000, now)).toBe(90);
    expect(formatAge(now - 90 * 86_400_000, now)).toBe("3 mo");
  });

  it("abbreviates the host-reported home directory on POSIX", () => {
    const mac = { homeDir: "/Users/austinsmith", separator: "/" };
    expect(shortPath("/Users/austinsmith/Developer/Repos/example", mac)).toBe("~/Developer/Repos/example");
    expect(shortPath("/Users/austinsmith", mac)).toBe("~");
    expect(shortPath("/Users/austinsmith/", { ...mac, homeDir: "/Users/austinsmith/" })).toBe("~");
    expect(shortPath("/Users/austinsmithy/code", mac)).toBe("/Users/austinsmithy/code");
    expect(shortPath("/Users/someone-else/Developer/example", mac)).toBe("/Users/someone-else/Developer/example");
    expect(shortPath("/home/linux-user/source/example", { homeDir: "/home/linux-user", separator: "/" })).toBe("~/source/example");
  });

  it("abbreviates Windows paths case-insensitively, including Git's forward slashes", () => {
    const windows = { homeDir: "C:\\Users\\Austin", separator: "\\" };
    expect(shortPath("C:\\Users\\Austin\\code\\repo", windows)).toBe("~\\code\\repo");
    expect(shortPath("c:/users/austin/code/repo", windows)).toBe("~\\code\\repo");
    expect(shortPath("C:\\Users\\Austin", windows)).toBe("~");
    expect(shortPath("D:\\Users\\Austin\\code", windows)).toBe("D:\\Users\\Austin\\code");
  });

  it("leaves paths untouched when the home directory is unknown", () => {
    expect(shortPath("/Users/austinsmith/code", { homeDir: null, separator: "/" })).toBe("/Users/austinsmith/code");
    expect(shortSha("1234567890abcdef")).toBe("12345678");
  });
});
