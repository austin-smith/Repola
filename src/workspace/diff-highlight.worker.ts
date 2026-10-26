import { toMessage } from "../lib/errors";
import type { HighlightRequest, HighlightResponse } from "./diff-highlighter";
import { tokenizeDiff } from "./tokenize-diff";

// The highlighter hands this worker one diff at a time (see DiffHighlighter),
// so requests never overlap.
self.addEventListener("message", (event: MessageEvent<HighlightRequest>) => {
  const { id, patch, grammar } = event.data;
  const reply = (response: HighlightResponse) => self.postMessage(response);
  tokenizeDiff(patch, grammar).then(
    (tokens) => reply({ id, tokens }),
    (cause: unknown) => reply({ id, error: toMessage(cause) }),
  );
});
