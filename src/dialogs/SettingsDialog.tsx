import { useEffect, useState } from "react";
import { ArrowDownIcon, ArrowUpIcon, CheckCircle2Icon, LaptopIcon, PencilIcon, PlusIcon, ServerIcon, Trash2Icon, WifiIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { Spinner } from "@/components/ui/spinner";
import { ThemePicker } from "@/components/theme-picker";
import { TooltipButton } from "@/components/tooltip-button";
import { loadAppPreferences, loadExternalTools, saveAppPreferences } from "../ipc/app-preferences";
import { UpdateSettings } from "../app/AppUpdater";
import { resolveAvailableToolId, toolLabels } from "../domain/external-tools";
import type { AgentInfo, AppPreferences, ExternalToolAvailability, MachineProfile, MachineProfileInput } from "../ipc/types";

interface SettingsDialogProps {
  machines: MachineProfile[];
  busy: boolean;
  onClose: () => void;
  onRemoveMachine: (machineId: string) => Promise<boolean>;
  onMoveMachine: (machineId: string, delta: -1 | 1) => Promise<void>;
  onSaveMachine: (input: MachineProfileInput) => Promise<boolean>;
  onTestMachine: (machineId: string) => Promise<AgentInfo | null>;
}

const sectionTitleClass = "text-xs font-medium tracking-widest text-muted-foreground uppercase";

function inputFor(machine?: MachineProfile): MachineProfileInput {
  return {
    id: machine?.kind === "ssh" ? machine.id : null,
    name: machine?.kind === "ssh" ? machine.name : "",
    enabled: machine?.kind === "ssh" ? machine.enabled : true,
    host: machine?.ssh?.host ?? "",
    user: machine?.ssh?.user ?? null,
    port: machine?.ssh?.port ?? null,
  };
}

export function SettingsDialog({
  machines,
  busy,
  onClose,
  onRemoveMachine,
  onMoveMachine,
  onSaveMachine,
  onTestMachine,
}: SettingsDialogProps) {
  const [draft, setDraft] = useState<MachineProfileInput | null>(null);
  const [testingId, setTestingId] = useState<string | null>(null);
  const [tested, setTested] = useState<Record<string, AgentInfo>>({});
  const [preferences, setPreferences] = useState<AppPreferences | null>(null);
  const [externalTools, setExternalTools] = useState<ExternalToolAvailability | null>(null);
  const [preferencesBusy, setPreferencesBusy] = useState(false);
  const [preferencesError, setPreferencesError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    void Promise.all([loadAppPreferences(), loadExternalTools()])
      .then(([value, tools]) => {
        if (!active) return;
        setExternalTools(tools);
        setPreferences({
          ...value,
          editorId: resolveAvailableToolId(value.editorId, tools.editors),
          terminalId: resolveAvailableToolId(value.terminalId, tools.terminals),
        });
      })
      .catch((cause: unknown) => { if (active) setPreferencesError(cause instanceof Error ? cause.message : String(cause)); });
    return () => { active = false; };
  }, []);

  const persistPreferences = async () => {
    if (!preferences) return;
    setPreferencesBusy(true);
    setPreferencesError(null);
    try {
      setPreferences(await saveAppPreferences(preferences));
    } catch (cause) {
      setPreferencesError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setPreferencesBusy(false);
    }
  };

  const submitMachine = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!draft) return;
    if (await onSaveMachine(draft)) setDraft(null);
  };

  const testConnection = async (machineId: string) => {
    setTestingId(machineId);
    const agent = await onTestMachine(machineId);
    if (agent) setTested((current) => ({ ...current, [machineId]: agent }));
    setTestingId(null);
  };

  const editorOptions = externalTools?.editors ?? [];
  const terminalOptions = externalTools?.terminals ?? [];
  const editorItems = toolLabels(editorOptions);
  const terminalItems = toolLabels(terminalOptions);

  return (
    <Dialog open onOpenChange={(open) => { if (!open && !busy) onClose(); }}>
      <DialogContent className="max-h-[88vh] overflow-y-auto sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Settings</DialogTitle>
          <DialogDescription className="sr-only">Application settings</DialogDescription>
        </DialogHeader>

        <section aria-labelledby="settings-machines" className="flex flex-col gap-3">
          <div className="flex items-start justify-between gap-4">
            <div className="flex flex-col gap-1">
              <h3 id="settings-machines" className={sectionTitleClass}>Machines</h3>
              <p className="text-sm text-muted-foreground">
                Work with repositories here or through your system OpenSSH configuration.
              </p>
            </div>
            <Button variant="outline" size="sm" disabled={busy || draft !== null} onClick={() => setDraft(inputFor())}>
              <PlusIcon data-icon="inline-start" aria-hidden="true" />
              Add machine
            </Button>
          </div>

          <ul className="flex flex-col gap-1.5">
            {machines.map((machine, machineIndex) => (
              <li key={machine.id} className="flex min-h-12 items-center gap-3 border bg-card px-3 py-2">
                {machine.kind === "local"
                  ? <LaptopIcon className="size-4 text-muted-foreground" aria-hidden="true" />
                  : <ServerIcon className="size-4 text-muted-foreground" aria-hidden="true" />}
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="truncate text-sm font-medium">{machine.name}</span>
                    {machine.kind === "local" ? <Badge variant="outline">Built in</Badge> : null}
                    {!machine.enabled ? <Badge variant="secondary">Disabled</Badge> : null}
                  </div>
                  <p className="truncate font-mono text-xs text-muted-foreground">
                    {machine.kind === "local"
                      ? "Local agent"
                      : `${machine.ssh?.user ? `${machine.ssh.user}@` : ""}${machine.ssh?.host ?? ""}${machine.ssh?.port ? `:${machine.ssh.port}` : ""}`}
                  </p>
                  {tested[machine.id] ? (
                    <p className="mt-0.5 flex items-center gap-1 text-xs text-success">
                      <CheckCircle2Icon className="size-3" aria-hidden="true" />
                      Connected · agent {tested[machine.id].agentVersion} · {tested[machine.id].gitVersion ?? "Git unavailable"}
                    </p>
                  ) : null}
                </div>
                {machine.kind === "ssh" ? (
                  <>
                    <div className="flex flex-col">
                      <TooltipButton
                        variant="ghost"
                        size="icon-xs"
                        aria-label={`Move ${machine.name} up`}
                        tooltip="Move up"
                        disabled={busy || machineIndex <= 1}
                        onClick={() => void onMoveMachine(machine.id, -1)}
                      >
                        <ArrowUpIcon aria-hidden="true" />
                      </TooltipButton>
                      <TooltipButton
                        variant="ghost"
                        size="icon-xs"
                        aria-label={`Move ${machine.name} down`}
                        tooltip="Move down"
                        disabled={busy || machineIndex === machines.length - 1}
                        onClick={() => void onMoveMachine(machine.id, 1)}
                      >
                        <ArrowDownIcon aria-hidden="true" />
                      </TooltipButton>
                    </div>
                    <TooltipButton
                      variant="ghost"
                      size="icon-sm"
                      aria-label={`Test ${machine.name}`}
                      tooltip="Test connection"
                      disabled={busy || testingId !== null}
                      onClick={() => void testConnection(machine.id)}
                    >
                      {testingId === machine.id ? <Spinner /> : <WifiIcon aria-hidden="true" />}
                    </TooltipButton>
                    <TooltipButton
                      variant="ghost"
                      size="icon-sm"
                      aria-label={`Edit ${machine.name}`}
                      tooltip="Edit machine"
                      disabled={busy}
                      onClick={() => setDraft(inputFor(machine))}
                    >
                      <PencilIcon aria-hidden="true" />
                    </TooltipButton>
                    <TooltipButton
                      variant="ghost"
                      size="icon-sm"
                      className="text-destructive"
                      aria-label={`Remove ${machine.name}`}
                      tooltip="Remove machine"
                      disabled={busy}
                      onClick={() => void onRemoveMachine(machine.id)}
                    >
                      <Trash2Icon aria-hidden="true" />
                    </TooltipButton>
                  </>
                ) : null}
              </li>
            ))}
          </ul>

          {draft ? (
            <form className="grid grid-cols-2 gap-3 border bg-muted/35 p-3" onSubmit={(event) => void submitMachine(event)}>
              <Field className="col-span-2">
                <FieldLabel htmlFor="machine-name">Name</FieldLabel>
                <Input
                  id="machine-name"
                  autoFocus
                  required
                  maxLength={80}
                  value={draft.name}
                  onChange={(event) => setDraft((current) => current && ({ ...current, name: event.target.value }))}
                  placeholder="Build server"
                />
              </Field>
              <Field className="col-span-2">
                <FieldLabel htmlFor="machine-host">SSH host</FieldLabel>
                <Input
                  id="machine-host"
                  required
                  maxLength={255}
                  value={draft.host}
                  onChange={(event) => setDraft((current) => current && ({ ...current, host: event.target.value }))}
                  placeholder="Alias from ~/.ssh/config"
                  spellCheck={false}
                />
              </Field>
              <Field>
                <FieldLabel htmlFor="machine-user">User <span className="text-muted-foreground">optional</span></FieldLabel>
                <Input
                  id="machine-user"
                  value={draft.user ?? ""}
                  onChange={(event) => setDraft((current) => current && ({ ...current, user: event.target.value || null }))}
                  placeholder="From SSH config"
                  spellCheck={false}
                />
              </Field>
              <Field>
                <FieldLabel htmlFor="machine-port">Port <span className="text-muted-foreground">optional</span></FieldLabel>
                <Input
                  id="machine-port"
                  type="number"
                  min={1}
                  max={65535}
                  value={draft.port ?? ""}
                  onChange={(event) => setDraft((current) => current && ({
                    ...current,
                    port: event.target.value === "" ? null : Number(event.target.value),
                  }))}
                  placeholder="22"
                />
              </Field>
              <label className="col-span-2 flex items-center gap-2 text-sm">
                <Checkbox
                  checked={draft.enabled}
                  onCheckedChange={(checked) => setDraft((current) => current && ({ ...current, enabled: checked === true }))}
                />
                Enable this machine
              </label>
              <div className="col-span-2 flex justify-end gap-2">
                <Button type="button" variant="ghost" size="sm" disabled={busy} onClick={() => setDraft(null)}>Cancel</Button>
                <Button type="submit" size="sm" disabled={busy}>
                  {busy ? <Spinner data-icon="inline-start" /> : null}
                  {draft.id ? "Save machine" : "Add machine"}
                </Button>
              </div>
            </form>
          ) : null}
        </section>

        <Separator />

        <section aria-labelledby="settings-tools" className="flex flex-col gap-3">
          <div className="flex flex-col gap-1">
            <h3 id="settings-tools" className={sectionTitleClass}>Tools & Git</h3>
            <p className="text-sm text-muted-foreground">Choose from applications detected on this computer. Remote worktrees use compatible editors' SSH workspace support.</p>
          </div>
          {preferences && externalTools ? (
            <div className="grid grid-cols-2 gap-3">
              <Field>
                <FieldLabel htmlFor="settings-editor">Editor</FieldLabel>
                <Select items={editorItems} value={preferences.editorId} onValueChange={(value) => setPreferences((current) => current && ({ ...current, editorId: value }))} disabled={editorOptions.length === 0}>
                  <SelectTrigger id="settings-editor"><SelectValue placeholder="No supported editors found" /></SelectTrigger>
                  <SelectContent><SelectGroup>
                    {editorOptions.map((option) => (
                      <SelectItem key={option.id} value={option.id} label={option.label}>{option.label}</SelectItem>
                    ))}
                  </SelectGroup></SelectContent>
                </Select>
              </Field>
              <Field>
                <FieldLabel htmlFor="settings-terminal">Terminal</FieldLabel>
                <Select items={terminalItems} value={preferences.terminalId} onValueChange={(value) => setPreferences((current) => current && ({ ...current, terminalId: value }))} disabled={terminalOptions.length === 0}>
                  <SelectTrigger id="settings-terminal"><SelectValue placeholder="No supported terminals found" /></SelectTrigger>
                  <SelectContent><SelectGroup>
                    {terminalOptions.map((option) => (
                      <SelectItem key={option.id} value={option.id} label={option.label}>{option.label}</SelectItem>
                    ))}
                  </SelectGroup></SelectContent>
                </Select>
              </Field>
              <Field orientation="horizontal" className="col-span-2">
                <Checkbox id="settings-sign-commits" checked={preferences.defaultSignCommits} onCheckedChange={(value) => setPreferences((current) => current && ({ ...current, defaultSignCommits: value === true }))} />
                <FieldLabel htmlFor="settings-sign-commits" className="font-normal">Sign commits by default using Git configuration</FieldLabel>
              </Field>
              <div className="col-span-2 flex justify-end">
                <Button size="sm" disabled={preferencesBusy} onClick={() => void persistPreferences()}>{preferencesBusy ? <Spinner data-icon="inline-start" /> : null}Save preferences</Button>
              </div>
            </div>
          ) : <p className="text-sm text-muted-foreground">Loading application preferences…</p>}
          {preferencesError && <p className="text-sm text-destructive">{preferencesError}</p>}
        </section>

        <Separator />

        <section aria-labelledby="settings-updates" className="flex flex-col gap-3">
          <h3 id="settings-updates" className={sectionTitleClass}>Updates</h3>
          <UpdateSettings />
        </section>

        <Separator />

        <section aria-labelledby="settings-appearance" className="flex flex-col gap-3">
          <h3 id="settings-appearance" className={sectionTitleClass}>Appearance</h3>
          <Field>
            <FieldLabel id="settings-theme-label">Theme</FieldLabel>
            <ThemePicker labelledBy="settings-theme-label" />
          </Field>
        </section>
      </DialogContent>
    </Dialog>
  );
}
