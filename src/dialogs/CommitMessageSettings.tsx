import { useCallback, useEffect, useRef, useState } from "react";
import { ChevronDownIcon, RefreshCwIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuSeparator, DropdownMenuSub, DropdownMenuSubContent, DropdownMenuSubTrigger, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Field, FieldError, FieldGroup, FieldLabel, FieldTitle } from "@/components/ui/field";
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Skeleton } from "@/components/ui/skeleton";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
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
}

function MachineCommitMessageSettings(props: Props) {
  const provider = props.selection?.provider ?? "codex";
  const [discovery, setDiscovery] = useState<Partial<Record<TextGenerationProvider, Discovery>>>({});
  const rememberDiscovery = useCallback((result: Discovery) => {
    setDiscovery((current) => ({ ...current, [provider]: result }));
  }, [provider]);
  return (
    <section aria-labelledby="settings-ai" className="flex flex-col gap-3">
      <h3 id="settings-ai" className="text-xs font-medium tracking-widest text-muted-foreground uppercase">AI</h3>
      <FieldGroup>
        <ProviderModels key={provider} {...props} provider={provider} initialDiscovery={discovery[provider]} onDiscovery={rememberDiscovery} />
      </FieldGroup>
      {props.saveError ? <FieldError role="alert">{props.saveError}</FieldError> : null}
    </section>
  );
}

function ProviderModels({ machine, selection: saved, providerSelections, disabled, onChange, provider, initialDiscovery, onDiscovery }: Props & {
  provider: TextGenerationProvider;
  initialDiscovery?: Discovery;
  onDiscovery: (result: Discovery) => void;
}) {
  const [initial] = useState(initialDiscovery);
  const [status, setStatus] = useState<TextGenerationStatus | null>(initial?.status ?? null);
  const lastStatus = useRef(initial?.status ?? null);
  const [error, setError] = useState<string | null>(initial?.error ?? null);
  const [revision, setRevision] = useState(0);
  const [loading, setLoading] = useState(!initial);
  useEffect(() => {
    if (revision === 0 && initial) return;
    const controller = new AbortController();
    void loadTextGenerationStatus(machine.id, provider, controller.signal)
      .then((result) => {
        if (controller.signal.aborted) return;
        lastStatus.current = result;
        setStatus(result);
        onDiscovery({ status: result, error: null });
      })
      .catch((cause: unknown) => {
        if (controller.signal.aborted) return;
        const message = toMessage(cause);
        setError(message);
        onDiscovery({ status: lastStatus.current, error: message });
      })
      .finally(() => { if (!controller.signal.aborted) setLoading(false); });
    return () => controller.abort();
  }, [machine.id, provider, revision, initial, onDiscovery]);

  const issue = error ?? status?.detail ?? (!status ? null : {
    ready: null,
    notInstalled: `Install ${provider === "claude" ? "Claude Code" : "the Codex CLI"} on ${machine.name}.`,
    signedOut: `Run ${provider === "claude" ? "claude auth login" : "codex login"} on ${machine.name}.`,
    updateRequired: `Update ${labels[provider]} on ${machine.name}.`,
    unavailable: `${labels[provider]} could not be checked on ${machine.name}.`,
  }[status.status]);
  const picker = status ? resolveGenerationPicker(saved, status, false) : null;
  const selection = picker?.selection;
  const recommendedModels = picker?.visible.filter((model) => !picker.other.some((other) => other.model === model.model)) ?? [];
  const chooseModel = (value: unknown) => {
    const model = status?.models.find((candidate) => candidate.model === value);
    if (model) onChange(selectionForModel(provider, model));
  };
  const efforts = picker?.selectedModel?.supportedReasoningEfforts ?? [];
  const effortItems = efforts.map(({ reasoningEffort }) => ({ value: reasoningEffort, label: effortLabels[reasoningEffort] ?? reasoningEffort }));
  if (picker?.invalidReasoning && selection?.reasoningEffort) effortItems.push({ value: selection.reasoningEffort, label: selection.reasoningEffort });
  return (
    <>
      <Field orientation="horizontal" data-disabled={disabled}>
        <FieldTitle id="settings-generation-provider">Provider</FieldTitle>
        <div className="flex items-center gap-2">
          <ToggleGroup aria-labelledby="settings-generation-provider" variant="outline" value={[provider]} disabled={disabled}
            onValueChange={(values) => {
              const value = values[0];
              if ((value === "codex" || value === "claude") && value !== provider) {
                onChange({ provider: value, model: null, reasoningEffort: null, ...providerSelections?.[value] });
              }
            }}>
            <ToggleGroupItem value="codex">Codex</ToggleGroupItem>
            <ToggleGroupItem value="claude">Claude</ToggleGroupItem>
          </ToggleGroup>
          <Button variant="ghost" size="icon-sm" aria-label={`Check ${labels[provider]} again`} disabled={loading || disabled} onClick={() => {
            setLoading(true);
            setError(null);
            setRevision((value) => value + 1);
          }}>
            {loading ? <Spinner /> : <RefreshCwIcon />}
          </Button>
        </div>
      </Field>
      {loading && !status ? (
        <FieldGroup role="status" aria-label={`Loading ${labels[provider]} settings`} aria-busy="true" className="grid grid-cols-1 items-start @sm/field-group:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
          <Field aria-hidden="true" className="min-w-0">
            <FieldTitle>Model</FieldTitle>
            <Skeleton className="h-8 w-full motion-reduce:animate-none" />
          </Field>
          <Field aria-hidden="true" className="min-w-0">
            <FieldTitle>Reasoning</FieldTitle>
            <Skeleton className="h-8 w-full motion-reduce:animate-none" />
          </Field>
        </FieldGroup>
      ) : null}
      {status?.status === "ready" && picker ? (
        <FieldGroup className="grid grid-cols-1 items-start @sm/field-group:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
          <Field className="min-w-0" data-invalid={picker.invalidModel} data-disabled={disabled}>
            <FieldLabel id="settings-generation-model-label" htmlFor="settings-generation-model">Model</FieldLabel>
            <DropdownMenu>
              <DropdownMenuTrigger id="settings-generation-model" aria-labelledby="settings-generation-model-label settings-generation-model-value" aria-invalid={picker.invalidModel} disabled={disabled} render={<Button variant="outline" className="w-full justify-between" />}>
                <span id="settings-generation-model-value" className="truncate">{picker.selectedModel?.displayName ?? selection?.model ?? "Choose a model"}</span>
                <ChevronDownIcon data-icon="inline-end" />
              </DropdownMenuTrigger>
              <DropdownMenuContent>
                <DropdownMenuRadioGroup value={selection?.model ?? ""} onValueChange={chooseModel}>
                  {recommendedModels.map((model) => <DropdownMenuRadioItem key={model.model} value={model.model} closeOnClick>{model.displayName}</DropdownMenuRadioItem>)}
                </DropdownMenuRadioGroup>
                {picker.other.length > 0 ? (
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
            {picker.invalidModel ? <FieldError>This saved model is unavailable. Choose another model.</FieldError> : null}
          </Field>
          {efforts.length > 0 && selection ? (
            <Field className="min-w-0" data-invalid={picker.invalidReasoning} data-disabled={disabled}>
              <FieldLabel htmlFor="settings-generation-reasoning">Reasoning</FieldLabel>
              <Select items={effortItems} value={selection.reasoningEffort} disabled={disabled} onValueChange={(value) => {
                if (value) onChange({ ...selection, provider, reasoningEffort: value });
              }}>
                <SelectTrigger id="settings-generation-reasoning" aria-invalid={picker.invalidReasoning} className="w-full"><SelectValue placeholder="Choose reasoning" /></SelectTrigger>
                <SelectContent><SelectGroup>
                  {effortItems.map((item) => <SelectItem key={item.value} value={item.value} label={item.label}>{item.label}</SelectItem>)}
                </SelectGroup></SelectContent>
              </Select>
              {picker.invalidReasoning ? <FieldError>This saved reasoning level is unsupported. Choose another level.</FieldError> : null}
            </Field>
          ) : (
            <Field className="min-w-0" data-disabled>
              <FieldLabel htmlFor="settings-generation-reasoning">Reasoning</FieldLabel>
              <Select disabled value={null}>
                <SelectTrigger id="settings-generation-reasoning" className="w-full"><SelectValue placeholder="Not applicable" /></SelectTrigger>
              </Select>
              {picker.invalidReasoning && picker.selectedModel ? (
                <>
                  <FieldError>This model does not support the saved reasoning level.</FieldError>
                  <Button variant="outline" size="sm" disabled={disabled} onClick={() => {
                    if (picker.selectedModel) onChange(selectionForModel(provider, picker.selectedModel));
                  }}>Use model defaults</Button>
                </>
              ) : null}
            </Field>
          )}
        </FieldGroup>
      ) : !loading ? (
        <FieldGroup className="grid grid-cols-1 items-start @sm/field-group:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
          {(["Model", "Reasoning"] as const).map((label) => (
            <Field key={label} className="min-w-0" data-disabled>
              <FieldLabel htmlFor={`settings-generation-${label.toLowerCase()}`}>{label}</FieldLabel>
              <Select disabled value={null}>
                <SelectTrigger id={`settings-generation-${label.toLowerCase()}`} className="w-full"><SelectValue placeholder="Unavailable" /></SelectTrigger>
              </Select>
            </Field>
          ))}
        </FieldGroup>
      ) : null}
      {issue ? <p role={error ? "alert" : "status"} className="text-sm text-muted-foreground">{issue}</p> : null}
    </>
  );
}
