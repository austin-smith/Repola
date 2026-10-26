import { describe, expect, it } from "vitest";
import {
  arrowKeyChangeTarget,
  isSelectAllChangesShortcut,
  isToggleSelectedChangesShortcut,
  restrictChangeSelection,
  selectAllChanges,
  singleChangeSelection,
  updateChangeSelection,
} from "./change-selection";

const ids = ["a", "b", "c", "d"];

describe("changed-file selection", () => {
  it("recognizes Command+A and Control+A without broader modifier chords", () => {
    expect(isSelectAllChangesShortcut({ key: "a", metaKey: true, ctrlKey: false, altKey: false, shiftKey: false })).toBe(true);
    expect(isSelectAllChangesShortcut({ key: "A", metaKey: false, ctrlKey: true, altKey: false, shiftKey: false })).toBe(true);
    expect(isSelectAllChangesShortcut({ key: "a", metaKey: true, ctrlKey: false, altKey: false, shiftKey: true })).toBe(false);
    expect(isSelectAllChangesShortcut({ key: "a", metaKey: false, ctrlKey: false, altKey: false, shiftKey: false })).toBe(false);
  });

  it("recognizes an unmodified Space key for toggling selected files", () => {
    expect(isToggleSelectedChangesShortcut({ key: " ", metaKey: false, ctrlKey: false, altKey: false, shiftKey: false })).toBe(true);
    expect(isToggleSelectedChangesShortcut({ key: " ", metaKey: true, ctrlKey: false, altKey: false, shiftKey: false })).toBe(false);
    expect(isToggleSelectedChangesShortcut({ key: "Enter", metaKey: false, ctrlKey: false, altKey: false, shiftKey: false })).toBe(false);
  });

  it("keeps a selection that is entirely listed unchanged", () => {
    const current = updateChangeSelection(ids, singleChangeSelection("b"), "c", { additive: true, range: false });

    expect(restrictChangeSelection(ids, current)).toBe(current);
  });

  it("drops hidden rows while keeping a listed active row", () => {
    const current = { selectedIds: new Set(["b", "d"]), activeId: "b", anchorId: "d" };
    const selection = restrictChangeSelection(["a", "b", "c"], current);

    expect([...selection.selectedIds]).toEqual(["b"]);
    expect(selection.activeId).toBe("b");
    expect(selection.anchorId).toBe("b");
  });

  it("moves a hidden active row to the first listed selected row", () => {
    const current = updateChangeSelection(ids, singleChangeSelection("a"), "c", { additive: true, range: false });
    const selection = restrictChangeSelection(["a", "b", "d"], current);

    expect([...selection.selectedIds]).toEqual(["a"]);
    expect(selection.activeId).toBe("a");
    expect(selection.anchorId).toBe("a");
  });

  it("falls back to the first listed row when every selected row is hidden", () => {
    const selection = restrictChangeSelection(["b", "d"], singleChangeSelection("c"));

    expect([...selection.selectedIds]).toEqual(["b"]);
    expect(selection.activeId).toBe("b");
  });

  it("selects nothing when no row is listed", () => {
    const selection = restrictChangeSelection([], singleChangeSelection("c"));

    expect(selection.selectedIds.size).toBe(0);
    expect(selection.activeId).toBeNull();
  });

  it("selects every visible change without changing the active preview", () => {
    const selection = selectAllChanges(ids, singleChangeSelection("c"));

    expect([...selection.selectedIds]).toEqual(ids);
    expect(selection.activeId).toBe("c");
    expect(selection.anchorId).toBe("c");
  });

  it("falls back to the first visible change when selecting all", () => {
    const selection = selectAllChanges(ids, singleChangeSelection("missing"));

    expect(selection.activeId).toBe("a");
    expect(selection.anchorId).toBe("a");
  });

  it("uses a plain click as a new single selection", () => {
    const selection = updateChangeSelection(ids, selectAllChanges(ids, singleChangeSelection("a")), "c", {
      additive: false,
      range: false,
    });

    expect([...selection.selectedIds]).toEqual(["c"]);
    expect(selection.activeId).toBe("c");
    expect(selection.anchorId).toBe("c");
  });

  it("toggles individual changes with the platform modifier", () => {
    const added = updateChangeSelection(ids, singleChangeSelection("a"), "c", {
      additive: true,
      range: false,
    });
    const removed = updateChangeSelection(ids, added, "c", {
      additive: true,
      range: false,
    });

    expect([...added.selectedIds]).toEqual(["a", "c"]);
    expect([...removed.selectedIds]).toEqual(["a"]);
    expect(removed.activeId).toBe("a");
  });

  it("selects an anchored range with Shift", () => {
    const selection = updateChangeSelection(ids, singleChangeSelection("b"), "d", {
      additive: false,
      range: true,
    });

    expect([...selection.selectedIds]).toEqual(["b", "c", "d"]);
    expect(selection.activeId).toBe("d");
    expect(selection.anchorId).toBe("b");
  });

  it("moves the active row with arrow keys and clamps at the list edges", () => {
    const key = (k: string, extra: Partial<{ metaKey: boolean; shiftKey: boolean; altKey: boolean; ctrlKey: boolean }> = {}) =>
      ({ key: k, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...extra });
    expect(arrowKeyChangeTarget(ids, singleChangeSelection(null), key("ArrowDown"))).toBe("a");
    expect(arrowKeyChangeTarget(ids, singleChangeSelection(null), key("ArrowUp"))).toBe("a");
    expect(arrowKeyChangeTarget(ids, singleChangeSelection("b"), key("ArrowDown"))).toBe("c");
    expect(arrowKeyChangeTarget(ids, singleChangeSelection("b"), key("ArrowUp"))).toBe("a");
    expect(arrowKeyChangeTarget(ids, singleChangeSelection("d"), key("ArrowDown"))).toBe("d");
    expect(arrowKeyChangeTarget(ids, singleChangeSelection("a"), key("ArrowUp"))).toBe("a");
    expect(arrowKeyChangeTarget(ids, singleChangeSelection("b"), key("ArrowDown", { metaKey: true }))).toBe("d");
    expect(arrowKeyChangeTarget(ids, singleChangeSelection("c"), key("Home"))).toBe("a");
    expect(arrowKeyChangeTarget(ids, singleChangeSelection("a"), key("End"))).toBe("d");
    expect(arrowKeyChangeTarget(ids, singleChangeSelection("b"), key("ArrowDown", { altKey: true }))).toBeNull();
    expect(arrowKeyChangeTarget(ids, singleChangeSelection("b"), key("Enter"))).toBeNull();
    expect(arrowKeyChangeTarget([], singleChangeSelection(null), key("ArrowDown"))).toBeNull();
  });
});
