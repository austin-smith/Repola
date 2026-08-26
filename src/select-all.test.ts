import { describe, expect, it } from "vitest";
import { performFocusedSelectAll, SELECT_ALL_EVENT } from "./select-all";

describe("contextual Select All", () => {
  it("preserves native text-input selection", () => {
    const input = document.createElement("input");
    input.value = "commit summary";
    document.body.append(input);
    input.focus();

    performFocusedSelectAll();

    expect(input.selectionStart).toBe(0);
    expect(input.selectionEnd).toBe(input.value.length);
    input.remove();
  });

  it("allows a focused composite control to handle Select All", () => {
    const list = document.createElement("div");
    const row = document.createElement("button");
    list.append(row);
    document.body.append(list);
    row.focus();
    let handled = false;
    list.addEventListener(SELECT_ALL_EVENT, (event) => {
      handled = true;
      event.preventDefault();
    });

    performFocusedSelectAll();

    expect(handled).toBe(true);
    list.remove();
  });
});
