import type { TextGenerationModel, TextGenerationPreferences, TextGenerationProvider, TextGenerationSelection, TextGenerationStatus } from "../ipc/types";

export function activeGenerationSelection(preferences: TextGenerationPreferences | undefined): TextGenerationSelection {
  const provider = preferences?.provider ?? "codex";
  return { provider, model: null, reasoningEffort: null, ...preferences?.selections[provider] };
}

export function rememberGenerationSelection(preferences: TextGenerationPreferences | undefined, selection: TextGenerationSelection): TextGenerationPreferences {
  const { provider, model, reasoningEffort } = selection;
  return { provider, selections: { ...preferences?.selections, [provider]: { model, reasoningEffort } } };
}

export function selectionForModel(provider: TextGenerationProvider, model: TextGenerationModel): TextGenerationSelection {
  return { provider, model: model.model, reasoningEffort: model.defaultReasoningEffort };
}

export function resolveGenerationPicker(saved: TextGenerationSelection | null, status: TextGenerationStatus, expanded: boolean) {
  const selection = saved?.model ? saved : status.recommendedSelection;
  const selectedModel = status.models.find((model) => model.model === selection?.model) ?? null;
  const invalidModel = Boolean(selection?.model && !selectedModel);
  const invalidReasoning = Boolean(selectedModel && selection && (
    selection.reasoningEffort === null
      ? selectedModel.supportedReasoningEfforts.length > 0
      : !selectedModel.supportedReasoningEfforts.some((option) => option.reasoningEffort === selection.reasoningEffort)
  ));
  const recommended = status.models.filter((model) => model.recommendedForCommitMessages || model.model === status.recommendedSelection?.model);
  const recommendedIds = new Set(recommended.map((model) => model.model));
  const other = status.models.filter((model) => !recommendedIds.has(model.model));
  const visible = expanded ? status.models : status.models.filter((model) => recommendedIds.has(model.model) || model.model === selection?.model);
  return { selection, selectedModel, invalidModel, invalidReasoning, visible, other };
}
