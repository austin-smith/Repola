import { useEffect, useState } from "react";
import { ChevronDownIcon, RefreshCwIcon } from "lucide-react";
import { ClaudeLogo, OpenAILogo } from "@/components/provider-logos";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuSeparator, DropdownMenuSub, DropdownMenuSubContent, DropdownMenuSubTrigger, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Field, FieldError, FieldGroup, FieldLabel, FieldTitle } from "@/components/ui/field";
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Skeleton } from "@/components/ui/skeleton";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { cn } from "@/lib/utils";
import { resolveGenerationPicker, selectionForModel } from "../domain/text-generation";
import { loadTextGenerationStatus } from "../ipc/worktrees";
import type { MachineProfile, TextGenerationPreferences, TextGenerationProvider, TextGenerationSelection, TextGenerationStatus } from "../ipc/types";
import { toMessage } from "../lib/errors";

interface Props {
  machine: MachineProfile;
  selection: TextGenerationSelection | null;
  providerSelections?: TextGenerationPreferences["selections"];
  saveError?: string | null;
  disabled: boolean;
  onChange: (selection: TextGenerationSelection) => void;
}

const labels = { codex: "Codex", claude: "Claude" };
const effortLabels: Record<string, string> = {
  none: "None", minimal: "Minimal", low: "Low", medium: "Medium", high: "High",
  xhigh: "Extra high", max: "Max", ultra: "Ultra",
};

export function CommitMessageSettings(props: Props) {
  return <MachineCommitMessageSettings key={props.machine.id} {...props} />;
}

interface Discovery {
  status: TextGenerationStatus | null;
  error: string | null;
  revision: number;
}
type DiscoveryKey = TextGenerationProvider | "automatic";

function MachineCommitMessageSettings({ machine, selection: saved, providerSelections, disabled, onChange, saveError }: Props) {
  const requestedProvider = saved?.provider ?? null;
  const key = requestedProvider ?? "automatic";
  const [discovery, setDiscovery] = useState<Partial<Record<DiscoveryKey, Discovery>>>({});
  const [revisions, setRevisions] = useState({ automatic: 0, codex: 0, claude: 0 });
  const revision = revisions[key];
  const result = discovery[key];
  const status = result?.status ?? null;
  const provider = requestedProvider ?? status?.recommendedSelection?.provider ?? null;
  const providerLabel = provider ? labels[provider] : "AI provider";
  const loading = result?.revision !== revision;
  const error = loading ? null : result?.error;

  useEffect(() => {
    if (disabled || result?.revision === revision) return;
    const controller = new AbortController();
    void loadTextGenerationStatus(machine.id, requestedProvider, controller.signal)
      .then((status) => {
        if (!controller.signal.aborted) {
          setDiscovery((current) => {
            const next = { ...current, [key]: { status, error: null, revision } };
            // An explicit choice of the detected provider reuses its loaded controls.
            const detected = status.recommendedSelection?.provider;
            if (key === "automatic" && detected) {
              next[detected] = { status, error: null, revision: revisions[detected] };
            }
            return next;
          });
        }
      })
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) {
          setDiscovery((current) => ({ ...current, [key]: {
            status: current[key]?.status ?? null, error: toMessage(cause), revision,
          } }));
        }
      });
    return () => controller.abort();
  }, [machine.id, requestedProvider, key, revision, revisions, result, disabled]);

  const issue = error ?? status?.detail ?? (!status ? null : {
    ready: null,
    notInstalled: `Install ${provider === "claude" ? "Claude Code" : "the Codex CLI"} on ${machine.name}.`,
    signedOut: `Run ${provider === "claude" ? "claude auth login" : "codex login"} on ${machine.name}.`,
    updateRequired: `Update ${providerLabel} on ${machine.name}.`,
    unavailable: `${providerLabel} could not be checked on ${machine.name}.`,
  }[status.status]);
  const picker = status ? resolveGenerationPicker(saved, status, false) : null;
  const selection = picker?.selection;
  const recommendedModels = picker?.visible.filter((model) => !picker.other.some((other) => other.model === model.model)) ?? [];
  const chooseModel = (value: unknown) => {
    const model = status?.models.find((candidate) => candidate.model === value);
    if (model && provider) onChange(selectionForModel(provider, model));
  };
  const efforts = picker?.selectedModel?.supportedReasoningEfforts ?? [];
  const effortItems = efforts.map(({ reasoningEffort }) => ({ value: reasoningEffort, label: effortLabels[reasoningEffort] ?? reasoningEffort }));
  if (picker?.invalidReasoning && selection?.reasoningEffort) effortItems.push({ value: selection.reasoningEffort, label: selection.reasoningEffort });
  const ready = status?.status === "ready";
  const initialLoading = loading && !status;
  const modelDisabled = disabled || !ready;
  const reasoningDisabled = modelDisabled || efforts.length === 0;
  return (
    <section aria-labelledby="settings-ai" className="flex flex-col gap-3">
      <h3 id="settings-ai" className="text-xs font-medium tracking-widest text-muted-foreground uppercase">AI</h3>
      <FieldGroup>
        <Field orientation="horizontal" data-disabled={disabled}>
          <FieldTitle id="settings-generation-provider">Provider</FieldTitle>
          <div className="flex items-center gap-2">
            <ToggleGroup aria-labelledby="settings-generation-provider" variant="default" value={provider ? [provider] : []} disabled={disabled}
              onValueChange={(values) => {
                const value = values[0] ?? (!saved ? provider : null);
                if ((value === "codex" || value === "claude") && (value !== provider || !saved)) {
                  onChange({ provider: value, model: null, reasoningEffort: null, ...providerSelections?.[value] });
                }
              }}>
              <ToggleGroupItem value="codex"><OpenAILogo />Codex</ToggleGroupItem>
              <ToggleGroupItem value="claude"><ClaudeLogo />Claude</ToggleGroupItem>
            </ToggleGroup>
            <Button variant="ghost" size="icon-sm" aria-label={provider ? `Check ${providerLabel} again` : "Check AI providers again"} disabled={loading || disabled} onClick={() => {
              setRevisions((current) => ({ ...current, [key]: current[key] + 1 }));
            }}>
              {loading ? <Spinner /> : <RefreshCwIcon />}
            </Button>
          </div>
        </Field>
        <FieldGroup role={initialLoading ? "status" : undefined} aria-label={initialLoading ? `Loading ${providerLabel} settings` : undefined} aria-busy={initialLoading || undefined} className="grid grid-cols-1 items-start @sm/field-group:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
          <Field className="min-w-0" data-invalid={picker?.invalidModel} data-disabled={disabled || (!ready && !initialLoading)}>
            <FieldLabel id="settings-generation-model-label" htmlFor="settings-generation-model">Model</FieldLabel>
            <div className="relative">
              <DropdownMenu>
                <DropdownMenuTrigger id="settings-generation-model" aria-labelledby="settings-generation-model-label settings-generation-model-value" aria-invalid={picker?.invalidModel} disabled={modelDisabled} render={<Button variant="outline" className={cn("w-full justify-between", initialLoading && "invisible")} />}>
                  <span id="settings-generation-model-value" className="truncate">
                    {ready ? picker?.selectedModel?.displayName ?? selection?.model ?? "Choose a model" : "Unavailable"}
                  </span>
                  <ChevronDownIcon data-icon="inline-end" />
                </DropdownMenuTrigger>
                <DropdownMenuContent>
                  <DropdownMenuRadioGroup value={selection?.model ?? ""} onValueChange={chooseModel}>
                    {recommendedModels.map((model) => <DropdownMenuRadioItem key={model.model} value={model.model} closeOnClick>{model.displayName}</DropdownMenuRadioItem>)}
                  </DropdownMenuRadioGroup>
                  {picker && picker.other.length > 0 ? (
                    <>
                      <DropdownMenuSeparator />
                      <DropdownMenuGroup>
                        <DropdownMenuSub>
                          <DropdownMenuSubTrigger>More models…</DropdownMenuSubTrigger>
                          <DropdownMenuSubContent>
                            <DropdownMenuRadioGroup value={selection?.model ?? ""} onValueChange={chooseModel}>
                              {picker.other.map((model) => <DropdownMenuRadioItem key={model.model} value={model.model} closeOnClick>{model.displayName}</DropdownMenuRadioItem>)}
                            </DropdownMenuRadioGroup>
                          </DropdownMenuSubContent>
                        </DropdownMenuSub>
                      </DropdownMenuGroup>
                    </>
                  ) : null}
                </DropdownMenuContent>
              </DropdownMenu>
              {initialLoading ? <Skeleton aria-hidden="true" className="absolute inset-0 animate-none" /> : null}
            </div>
            {picker?.invalidModel ? <FieldError>This saved model is unavailable. Choose another model.</FieldError> : null}
          </Field>
          <Field className="min-w-0" data-invalid={picker?.invalidReasoning} data-disabled={disabled || (!initialLoading && reasoningDisabled)}>
            <FieldLabel htmlFor="settings-generation-reasoning">Reasoning</FieldLabel>
            <div className="relative">
              <Select items={effortItems} value={ready && efforts.length > 0 ? selection?.reasoningEffort ?? null : null} disabled={reasoningDisabled} onValueChange={(value) => {
                if (value && selection && ready && provider) onChange({ ...selection, provider, reasoningEffort: value });
              }}>
                <SelectTrigger id="settings-generation-reasoning" aria-invalid={picker?.invalidReasoning} className={cn("w-full", initialLoading && "invisible")}>
                  <SelectValue placeholder={!ready ? "Unavailable" : efforts.length === 0 ? "Not applicable" : "Choose reasoning"} />
                </SelectTrigger>
                <SelectContent><SelectGroup>
                  {effortItems.map((item) => <SelectItem key={item.value} value={item.value} label={item.label}>{item.label}</SelectItem>)}
                </SelectGroup></SelectContent>
              </Select>
              {initialLoading ? <Skeleton aria-hidden="true" className="absolute inset-0 animate-none" /> : null}
            </div>
            {picker?.invalidReasoning ? <FieldError>{efforts.length > 0 ? "This saved reasoning level is unsupported. Choose another level." : "This model does not support the saved reasoning level."}</FieldError> : null}
            {picker?.invalidReasoning && picker.selectedModel && efforts.length === 0 ? (
              <Button variant="outline" size="sm" disabled={modelDisabled} onClick={() => {
                if (picker.selectedModel && provider) onChange(selectionForModel(provider, picker.selectedModel));
              }}>Use model defaults</Button>
            ) : null}
          </Field>
        </FieldGroup>
        {issue ? <p role={error ? "alert" : "status"} className="text-sm text-muted-foreground">{issue}</p> : null}
      </FieldGroup>
      {saveError ? <FieldError role="alert">{saveError}</FieldError> : null}
    </section>
  );
}
