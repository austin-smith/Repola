import { describe, expect, it } from "vitest";
import { activeGenerationSelection, rememberGenerationSelection, resolveGenerationPicker, selectionForModel } from "./text-generation";
import type { TextGenerationModel, TextGenerationSelection, TextGenerationStatus } from "../ipc/types";

const model: TextGenerationModel = {
  model: "current-model", displayName: "Current model", description: "", isDefault: true,
  recommendedForCommitMessages: true, upgrade: null, defaultReasoningEffort: "low",
  supportedReasoningEfforts: [{ reasoningEffort: "low", description: "" }],
};
const recommended: TextGenerationSelection = { provider: "codex", model: model.model, reasoningEffort: "low" };
const status: TextGenerationStatus = { status: "ready", version: null, models: [model], recommendedSelection: recommended };

describe("generation model selection", () => {
  it("remembers separate model and reasoning choices through provider switches and serialization", () => {
    const codex = { ...recommended, reasoningEffort: "high" };
    const claude = { provider: "claude" as const, model: "claude-model", reasoningEffort: null };
    const saved = rememberGenerationSelection(rememberGenerationSelection(undefined, codex), claude);
    expect(activeGenerationSelection(saved)).toEqual(claude);
    const reloaded = JSON.parse(JSON.stringify(saved));
    expect(activeGenerationSelection({ ...reloaded, provider: "codex" })).toEqual(codex);
    expect(activeGenerationSelection(undefined)).toEqual({ provider: "codex", model: null, reasoningEffort: null });
  });
  it("resolves a provider default without replacing explicit unavailable choices", () => {
    expect(resolveGenerationPicker(null, status, false).selection).toEqual(recommended);
    const saved = { ...recommended, model: "removed-model" };
    const picker = resolveGenerationPicker(saved, status, false);
    expect(picker.selection).toEqual(saved);
    expect(picker.invalidModel).toBe(true);
  });

  it("keeps saved unsupported reasoning visible for correction", () => {
    const saved = { ...recommended, reasoningEffort: "unsupported" };
    const picker = resolveGenerationPicker(saved, status, false);
    expect(picker.selection).toEqual(saved);
    expect(picker.invalidReasoning).toBe(true);
  });

  it("keeps an explicit older model in the collapsed picker", () => {
    const older = { ...model, model: "older-model", recommendedForCommitMessages: false, isDefault: false };
    const picker = resolveGenerationPicker({ ...recommended, model: older.model }, { ...status, models: [model, older] }, false);
    expect(picker.visible).toContainEqual(older);
    expect(picker.invalidModel).toBe(false);
  });

  it("does not invent reasoning options for a model without them", () => {
    const simple = { ...model, defaultReasoningEffort: null, supportedReasoningEfforts: [] };
    const selection = selectionForModel("claude", simple);
    expect(selection.reasoningEffort).toBeNull();
    expect(selection.provider).toBe("claude");
    expect(resolveGenerationPicker(selection, { ...status, models: [simple] }, false).invalidReasoning).toBe(false);
  });
});
