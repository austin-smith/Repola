import { lazy } from "react";

// Dialogs and diff viewers are code-split so the startup bundle stays inside the budget in vite.config.ts.
export const DiffDialog = lazy(() =>
  import("../DiffDialog").then((module) => ({ default: module.DiffDialog })),
);
export const InlineFileDiff = lazy(() =>
  import("../InlineFileDiff").then((module) => ({ default: module.InlineFileDiff })),
);
export const CommitFileDiffView = lazy(() =>
  import("../CommitFileDiffView").then((module) => ({ default: module.CommitFileDiffView })),
);
export const HistoryMutationDialog = lazy(() => import("../HistoryMutationDialog"));
export const TagsDialog = lazy(() => import("../TagsDialog"));
export const ReflogDialog = lazy(() => import("../ReflogDialog"));
export const ConflictResolutionDialog = lazy(() => import("../ConflictResolutionDialog"));
export const PullRequestDialog = lazy(() => import("../PullRequestDialog"));
