import type { CommitSigning, HistoryMutationKind } from "../ipc/types";

export const sectionHeadingClass = "text-xs font-medium tracking-widest text-muted-foreground uppercase";

export const signingItems: Record<CommitSigning, string> = {
  default: "Use Git configuration",
  sign: "Sign this commit",
  doNotSign: "Do not sign this commit",
};

/** Success toast titles shared by every place that launches a history mutation. */
export const historyMutationTitles: Record<HistoryMutationKind, string> = {
  merge: "Branch merged",
  squashMerge: "Squashed changes staged",
  rebase: "Branch rebased",
  cherryPick: "Commit cherry-picked",
  revert: "Revert commit created",
  resetSoft: "Branch softly reset",
  resetMixed: "Branch reset",
  resetHard: "Branch hard reset",
};
