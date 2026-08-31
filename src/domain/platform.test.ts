import { describe, expect, it } from "vitest";
import { fileManagerName } from "./platform";

describe("fileManagerName", () => {
  it("names the platform file browser from the webview user agent", () => {
    expect(fileManagerName("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15")).toBe("Finder");
    expect(fileManagerName("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")).toBe("File Explorer");
    expect(fileManagerName("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36")).toBe("file manager");
    expect(fileManagerName("")).toBe("file manager");
  });
});
