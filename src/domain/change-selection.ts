export interface ChangeSelection {
  readonly selectedIds: ReadonlySet<string>;
  readonly activeId: string | null;
  readonly anchorId: string | null;
}

export interface ChangeSelectionModifiers {
  readonly additive: boolean;
  readonly range: boolean;
}

export interface SelectAllShortcutEvent {
  readonly key: string;
  readonly metaKey: boolean;
  readonly ctrlKey: boolean;
  readonly altKey: boolean;
  readonly shiftKey: boolean;
}

export const emptyChangeSelection: ChangeSelection = {
  selectedIds: new Set(),
  activeId: null,
  anchorId: null,
};

export function singleChangeSelection(id: string | null): ChangeSelection {
  return {
    selectedIds: id === null ? new Set() : new Set([id]),
    activeId: id,
    anchorId: id,
  };
}

export function isSelectAllChangesShortcut(event: SelectAllShortcutEvent): boolean {
  return (event.metaKey || event.ctrlKey)
    && !event.altKey
    && !event.shiftKey
    && event.key.toLowerCase() === "a";
}

export function isToggleSelectedChangesShortcut(event: SelectAllShortcutEvent): boolean {
  return !event.metaKey
    && !event.ctrlKey
    && !event.altKey
    && !event.shiftKey
    && event.key === " ";
}

/**
 * Limits a selection to the listed rows, so filtered-out rows are never acted
 * on. When the active row is hidden, the first listed selected row takes over,
 * or else the first listed row, keeping the preview on a visible change.
 */
export function restrictChangeSelection(ids: readonly string[], current: ChangeSelection): ChangeSelection {
  const listed = new Set(ids);
  const activeListed = current.activeId === null || listed.has(current.activeId);
  const anchorListed = current.anchorId === null || listed.has(current.anchorId);
  if (activeListed && anchorListed && [...current.selectedIds].every((id) => listed.has(id))) return current;

  const selectedIds = new Set(ids.filter((id) => current.selectedIds.has(id)));
  if (activeListed) {
    return { selectedIds, activeId: current.activeId, anchorId: anchorListed ? current.anchorId : current.activeId };
  }
  const activeId = ids.find((id) => selectedIds.has(id)) ?? null;
  if (activeId === null) return singleChangeSelection(ids[0] ?? null);
  return { selectedIds, activeId, anchorId: anchorListed ? current.anchorId : activeId };
}

export function selectAllChanges(ids: readonly string[], current: ChangeSelection): ChangeSelection {
  const selectedIds = new Set(ids);
  const activeId = current.activeId !== null && selectedIds.has(current.activeId)
    ? current.activeId
    : ids[0] ?? null;
  const anchorId = current.anchorId !== null && selectedIds.has(current.anchorId)
    ? current.anchorId
    : activeId;

  return { selectedIds, activeId, anchorId };
}

export function updateChangeSelection(
  ids: readonly string[],
  current: ChangeSelection,
  targetId: string,
  modifiers: ChangeSelectionModifiers,
): ChangeSelection {
  const targetIndex = ids.indexOf(targetId);
  if (targetIndex < 0) return current;

  if (modifiers.range) {
    const anchorIndex = current.anchorId === null ? -1 : ids.indexOf(current.anchorId);
    const effectiveAnchorIndex = anchorIndex >= 0 ? anchorIndex : targetIndex;
    const rangeIds = ids.slice(
      Math.min(effectiveAnchorIndex, targetIndex),
      Math.max(effectiveAnchorIndex, targetIndex) + 1,
    );
    const selectedIds = modifiers.additive ? new Set(current.selectedIds) : new Set<string>();
    rangeIds.forEach((id) => selectedIds.add(id));
    return {
      selectedIds,
      activeId: targetId,
      anchorId: ids[effectiveAnchorIndex] ?? targetId,
    };
  }

  if (modifiers.additive) {
    const selectedIds = new Set(current.selectedIds);
    if (selectedIds.has(targetId)) selectedIds.delete(targetId);
    else selectedIds.add(targetId);
    const activeId = selectedIds.has(targetId)
      ? targetId
      : current.activeId === targetId
        ? selectedIds.values().next().value ?? null
        : current.activeId;
    return { selectedIds, activeId, anchorId: targetId };
  }

  return singleChangeSelection(targetId);
}

/**
 * Resolves the row an arrow-key press should move the active selection to,
 * or null when the key is not a navigation key or the list is empty.
 */
export function arrowKeyChangeTarget(
  ids: readonly string[],
  current: ChangeSelection,
  event: SelectAllShortcutEvent,
): string | null {
  if (ids.length === 0 || event.altKey || event.ctrlKey) return null;
  const activeIndex = current.activeId === null ? -1 : ids.indexOf(current.activeId);
  const last = ids.length - 1;
  switch (event.key) {
    case "ArrowDown":
      return event.metaKey ? ids[last] : ids[Math.min(activeIndex + 1, last)];
    case "ArrowUp":
      return event.metaKey ? ids[0] : ids[activeIndex < 0 ? 0 : Math.max(activeIndex - 1, 0)];
    case "Home":
      return ids[0];
    case "End":
      return ids[last];
    default:
      return null;
  }
}
