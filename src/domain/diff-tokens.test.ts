import { describe, expect, it } from "vitest";
import { lineTokens, type SyntaxNode } from "./diff-tokens";

const span = (style: string | null, ...children: SyntaxNode[]): SyntaxNode => ({
  type: "element",
  properties: style === null ? {} : { style },
  children,
});
const text = (value: string): SyntaxNode => ({ type: "text", value });
const line = (...children: SyntaxNode[]): SyntaxNode => ({ type: "element", properties: { "data-line": 1 }, children });

describe("lineTokens", () => {
  it("keeps each theme's color and font style", () => {
    expect(lineTokens(line(
      span("--diffs-token-light:#D73A49;--diffs-token-dark:#F97583", text("let")),
      span("--diffs-token-light:#24292E;--diffs-token-light-font-style:italic;--diffs-token-dark:#E1E4E8;--diffs-token-dark-font-weight:bold;--diffs-token-dark-text-decoration:underline", text(" b")),
    ))).toEqual([
      { text: "let", light: { color: "#D73A49" }, dark: { color: "#F97583" } },
      { text: " b", light: { color: "#24292E", italic: true }, dark: { color: "#E1E4E8", bold: true, underline: true } },
    ]);
  });

  it("marks the text inside a changed-word span", () => {
    expect(lineTokens(line(
      span("--diffs-token-light:#032F62;--diffs-token-dark:#9ECBFF", text(" \"")),
      { type: "element", properties: { "data-diff-span": "" }, children: [span("--diffs-token-light:#032F62;--diffs-token-dark:#9ECBFF", text("y"))] },
      span("--diffs-token-light:#032F62;--diffs-token-dark:#9ECBFF", text("\"")),
    ))).toEqual([
      { text: " \"", light: { color: "#032F62" }, dark: { color: "#9ECBFF" } },
      { text: "y", light: { color: "#032F62" }, dark: { color: "#9ECBFF" }, changed: true },
      { text: "\"", light: { color: "#032F62" }, dark: { color: "#9ECBFF" } },
    ]);
  });

  it("merges neighbouring runs that look the same", () => {
    expect(lineTokens(line(
      span("--diffs-token-light:#24292E;--diffs-token-dark:#E1E4E8", text("a")),
      span("--diffs-token-light:#24292E;--diffs-token-dark:#E1E4E8", text("b")),
      span(null, text("c")),
      span(null, text("d")),
    ))).toEqual([
      { text: "ab", light: { color: "#24292E" }, dark: { color: "#E1E4E8" } },
      { text: "cd" },
    ]);
  });

  it("keeps unstyled text and ignores empty text and non-element nodes", () => {
    expect(lineTokens(line(text("plain"), text(""), { type: "comment", value: "note" }))).toEqual([{ text: "plain" }]);
    expect(lineTokens(line())).toEqual([]);
  });
});
