/** How a token looks in one theme. Absent fields keep the surrounding text's look. */
export interface TokenAppearance {
  readonly color?: string;
  readonly italic?: boolean;
  readonly bold?: boolean;
  readonly underline?: boolean;
}

/** A run of one line's text that is styled the same way throughout. */
export interface DiffToken {
  readonly text: string;
  readonly light?: TokenAppearance;
  readonly dark?: TokenAppearance;
  /** Whether this run is the part of a changed line that differs from its counterpart. */
  readonly changed?: boolean;
}

/**
 * Syntax tokens for both sides of a diff, indexed like the parsed diff's
 * `deletionLines` and `additionLines`.
 */
export interface DiffTokens {
  readonly deletions: readonly (readonly DiffToken[])[];
  readonly additions: readonly (readonly DiffToken[])[];
}

/** The parts of a syntax-tree node the highlighter produces for each line. */
export interface SyntaxNode {
  readonly type: string;
  readonly value?: string;
  readonly properties?: Readonly<Record<string, unknown>>;
  readonly children?: readonly SyntaxNode[];
}

const appearancePrefix = { light: "--diffs-token-light", dark: "--diffs-token-dark" } as const;

/**
 * Reduces one highlighted line, as a syntax tree, to its styled runs of text.
 * Neighbouring runs that look the same are merged so the result stays small
 * enough to pass between threads for large diffs.
 */
export function lineTokens(line: SyntaxNode): DiffToken[] {
  const tokens: DiffToken[] = [];
  const visit = (node: SyntaxNode, inherited: Omit<DiffToken, "text">) => {
    if (node.type === "text") {
      if (node.value) push(tokens, { ...inherited, text: node.value });
      return;
    }
    if (node.type !== "element") return;
    const properties = node.properties ?? {};
    const style = typeof properties.style === "string" ? parseStyle(properties.style) : null;
    const next: Omit<DiffToken, "text"> = {
      ...inherited,
      ...(style ? appearances(style) : {}),
      ...("data-diff-span" in properties ? { changed: true } : {}),
    };
    node.children?.forEach((child) => visit(child, next));
  };
  line.children?.forEach((child) => visit(child, {}));
  return tokens;
}

function push(tokens: DiffToken[], token: DiffToken) {
  const previous = tokens[tokens.length - 1];
  if (previous && sameLook(previous, token)) tokens[tokens.length - 1] = { ...previous, text: previous.text + token.text };
  else tokens.push(token);
}

function sameLook(left: DiffToken, right: DiffToken): boolean {
  return left.changed === right.changed && sameAppearance(left.light, right.light) && sameAppearance(left.dark, right.dark);
}

function sameAppearance(left: TokenAppearance | undefined, right: TokenAppearance | undefined): boolean {
  return left?.color === right?.color && left?.italic === right?.italic && left?.bold === right?.bold && left?.underline === right?.underline;
}

function appearances(style: ReadonlyMap<string, string>): Pick<DiffToken, "light" | "dark"> {
  const result: { light?: TokenAppearance; dark?: TokenAppearance } = {};
  for (const theme of ["light", "dark"] as const) {
    const prefix = appearancePrefix[theme];
    const color = style.get(prefix);
    const appearance: TokenAppearance = {
      ...(color ? { color } : {}),
      ...(style.get(`${prefix}-font-style`) === "italic" ? { italic: true } : {}),
      ...(style.get(`${prefix}-font-weight`) === "bold" ? { bold: true } : {}),
      ...(style.get(`${prefix}-text-decoration`)?.includes("underline") ? { underline: true } : {}),
    };
    if (Object.keys(appearance).length > 0) result[theme] = appearance;
  }
  return result;
}

function parseStyle(style: string): Map<string, string> {
  const declarations = new Map<string, string>();
  for (const declaration of style.split(";")) {
    const separator = declaration.indexOf(":");
    if (separator < 0) continue;
    declarations.set(declaration.slice(0, separator).trim().toLowerCase(), declaration.slice(separator + 1).trim());
  }
  return declarations;
}
