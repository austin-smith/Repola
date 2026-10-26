import { createBundledHighlighter } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";
import {
  attachResolvedLanguages,
  attachResolvedThemes,
  processPatch,
  renderDiffWithHighlighter,
} from "@pierre/diffs";
import { lineTokens, type DiffTokens } from "../domain/diff-tokens";
import { diffThemes, type HighlightGrammar } from "./highlight-grammar";

/**
 * Lines longer than this are shown as plain text. Matches the other diff
 * views, and keeps one minified line from stalling the whole diff.
 */
const TOKENIZE_MAX_LINE_LENGTH = 1_000;
/** Word-level change marking is skipped for lines longer than this, as in the other diff views. */
const MAX_LINE_DIFF_LENGTH = 1_000;

/**
 * A highlighter that bundles no grammars or themes of its own: the worker
 * never loads them itself, so it only ever uses the ones attached from a
 * request, and none of the language catalogue ships in the worker. The
 * JavaScript regex engine needs no WebAssembly, which the app's content
 * security policy does not allow.
 */
const createHighlighter = createBundledHighlighter<string, string>({
  langs: {},
  themes: {},
  engine: createJavaScriptRegexEngine,
});

let highlighter: ReturnType<typeof createHighlighter> | null = null;

function sharedHighlighter() {
  highlighter ??= createHighlighter({ langs: [], themes: [] });
  return highlighter;
}

/**
 * Highlights a single-file patch. This is the slow part of showing a diff,
 * so it runs on the highlighting worker, never on the UI thread.
 */
export async function tokenizeDiff(patch: string, grammar: HighlightGrammar): Promise<DiffTokens> {
  const files = processPatch(patch).files;
  const [file] = files;
  if (!file || files.length !== 1) throw new Error("Expected exactly one file in the patch.");
  const highlighter = await sharedHighlighter();
  attachResolvedThemes([...grammar.themes], highlighter);
  attachResolvedLanguages([...grammar.languages], highlighter);
  const { code } = renderDiffWithHighlighter({ ...file, lang: grammar.lang }, highlighter, {
    theme: diffThemes,
    useTokenTransformer: false,
    tokenizeMaxLineLength: TOKENIZE_MAX_LINE_LENGTH,
    lineDiffType: "word-alt",
    maxLineDiffLength: MAX_LINE_DIFF_LENGTH,
  });
  return {
    deletions: code.deletionLines.map(lineTokens),
    additions: code.additionLines.map(lineTokens),
  };
}
