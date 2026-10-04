import { lazy } from "react";

// Dialogs, diff viewers, and the History view are code-split so the startup bundle stays inside the
// budget in vite.config.ts.
export const DiffDialog = lazy(() =>
  import("../dialogs/DiffDialog").then((module) => ({ default: module.DiffDialog })),
);
export const InlineFileDiff = lazy(() =>
  import("./InlineFileDiff").then((module) => ({ default: module.InlineFileDiff })),
);
export const CommitFileDiffView = lazy(() =>
  import("./CommitFileDiffView").then((module) => ({ default: module.CommitFileDiffView })),
);
export const HistoryWorkbench = lazy(() =>
  import("./HistoryWorkbench").then((module) => ({ default: module.HistoryWorkbench })),
);
export const HistoryMutationDialog = lazy(() => import("../dialogs/HistoryMutationDialog"));
export const TagsDialog = lazy(() => import("../dialogs/TagsDialog"));
export const ReflogDialog = lazy(() => import("../dialogs/ReflogDialog"));
export const ConflictResolutionDialog = lazy(() => import("../dialogs/ConflictResolutionDialog"));
export const PullRequestDialog = lazy(() => import("../dialogs/PullRequestDialog"));
