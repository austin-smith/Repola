import {
  getFiletypeFromFileName,
  resolveLanguages,
  resolveThemes,
  type DiffsTheme,
  type HighlighterTypes,
  type SupportedLanguages,
} from "@pierre/diffs";
import type { ResolvedLanguage } from "@pierre/diffs/worker";

/** The themes every diff is highlighted with; tokens carry both, so switching appearance never re-highlights. */
export const diffThemes = { light: "github-light", dark: "github-dark" } as const;

/**
 * The highlighter both threads use. Its JavaScript regex engine needs no
 * WebAssembly, which the app's content security policy does not allow, and
 * a thread can only run one highlighter type at a time.
 */
export const diffHighlighterType: HighlighterTypes = "shiki-js";

/**
 * What the highlighting worker needs for one file: its language, with the
 * grammars and themes already loaded. They are loaded on the UI thread,
 * which can fetch them; a worker cannot.
 */
export interface HighlightGrammar {
  readonly lang: SupportedLanguages;
  readonly themes: readonly DiffsTheme[];
  readonly languages: readonly ResolvedLanguage[];
}

/**
 * Loads the grammar for the file at `path`, its real path, and the diff
 * themes. Each is fetched once and reused for later files.
 */
export async function resolveGrammar(path: string): Promise<HighlightGrammar> {
  const lang = getFiletypeFromFileName(path);
  const [themes, languages] = await Promise.all([
    resolveThemes([diffThemes.light, diffThemes.dark], diffHighlighterType),
    resolveLanguages([lang]),
  ]);
  return { lang, themes, languages };
}
