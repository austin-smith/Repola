import {
  attachResolvedLanguages,
  attachResolvedThemes,
  createHighlighter,
  processPatch,
  renderDiffWithHighlighter,
  type DiffsHighlighter,
} from "@pierre/diffs";
import { lineTokens, type DiffTokens } from "../domain/diff-tokens";
import { diffHighlighterType, diffThemes, type HighlightGrammar } from "./highlight-grammar";

/**
 * Lines longer than this are shown as plain text. Matches the other diff
 * views, and keeps one minified line from stalling the whole diff.
 */
const TOKENIZE_MAX_LINE_LENGTH = 1_000;
/** Word-level change marking is skipped for lines longer than this, as in the other diff views. */
const MAX_LINE_DIFF_LENGTH = 1_000;

/**
 * The worker's highlighter. It never loads grammars or themes itself; it only
 * uses the ones attached from each request.
 */
let highlighter: Promise<DiffsHighlighter> | null = null;

function sharedHighlighter() {
  highlighter ??= createHighlighter(diffHighlighterType);
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
