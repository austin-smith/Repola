export const SELECT_ALL_EVENT = "repola:select-all";

const selectableInputTypes = new Set(["", "email", "password", "search", "tel", "text", "url"]);

export function performFocusedSelectAll(): void {
  const activeElement = document.activeElement;
  if (!(activeElement instanceof HTMLElement)) return;

  if (activeElement instanceof HTMLTextAreaElement) {
    activeElement.select();
    return;
  }
  if (activeElement instanceof HTMLInputElement && selectableInputTypes.has(activeElement.type)) {
    activeElement.select();
    return;
  }
  if (activeElement.isContentEditable) {
    const selection = window.getSelection();
    if (!selection) return;
    const range = document.createRange();
    range.selectNodeContents(activeElement);
    selection.removeAllRanges();
    selection.addRange(range);
    return;
  }

  const event = new CustomEvent(SELECT_ALL_EVENT, { bubbles: true, cancelable: true });
  if (activeElement.dispatchEvent(event)) return;

  // AppKit can apply its native Select All action to WKWebView before the
  // asynchronous Tauri menu event arrives. Remove that transient document
  // selection after a contextual surface claims the command.
  window.getSelection()?.removeAllRanges();
}
