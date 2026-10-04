import { describe, expect, it } from "vitest";
import { toMessage } from "./errors";

describe("toMessage", () => {
  it("reads the message of errors and of objects an IPC command rejects with", () => {
    expect(toMessage(new Error("broken"))).toBe("broken");
    expect(toMessage({ message: "refused", outcomeKnown: true })).toBe("refused");
    expect(toMessage("plain text")).toBe("plain text");
    expect(toMessage({ code: 7 })).toBe("[object Object]");
  });
});
