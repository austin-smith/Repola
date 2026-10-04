import { LaptopIcon, ServerIcon, SettingsIcon, WifiIcon, WifiOffIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { TooltipButton } from "@/components/tooltip-button";
import { cn } from "@/lib/utils";
import type { AgentInfo, MachineProfile, WorkspaceView } from "../ipc/types";
import { WorkspaceNavigation } from "./WorkspaceNavigation";

export type MachineConnectionStatus = "checking" | "online" | "offline";

export interface MachineConnection {
  status: MachineConnectionStatus;
  latencyMs: number | null;
  checkedAtMs: number | null;
  lastConnectedAtMs: number | null;
  agent: AgentInfo | null;
  error: string | null;
}

export function AppSidebar({
  machines,
  selectedMachine,
  selectedMachineId,
  connection,
  disabled,
  view,
  onMachineChange,
  onRetry,
  onViewChange,
  onOpenSettings,
}: {
  machines: MachineProfile[];
  selectedMachine: MachineProfile;
  selectedMachineId: string;
  connection: MachineConnection | null;
  disabled: boolean;
  view: WorkspaceView;
  onMachineChange: (machineId: string) => Promise<void>;
  onRetry: () => void;
  onViewChange: (view: WorkspaceView) => void;
  onOpenSettings: () => void;
}) {
  const enabledMachines = machines.filter((machine) => machine.enabled);
  const items = Object.fromEntries(enabledMachines.map((machine) => [machine.id, machine.name]));

  return (
    <aside className="flex w-56 shrink-0 flex-col border-r bg-sidebar text-sidebar-foreground" aria-label="Repola application sidebar">
      <div className="flex h-16 shrink-0 items-center gap-3 border-b px-4">
        <div className="grid size-8 shrink-0 content-center gap-1 border border-foreground bg-brand p-2" aria-hidden="true">
          <span className="block h-px bg-brand-foreground" />
          <span className="block h-px bg-brand-foreground" />
          <span className="block h-px bg-brand-foreground" />
        </div>
        <h1 className="min-w-0 truncate text-base font-semibold tracking-tight" translate="no">Repola</h1>
      </div>

      <WorkspaceNavigation view={view} onViewChange={onViewChange} />

      <div className="mt-auto flex flex-col gap-1 border-t p-3">
        {enabledMachines.length > 1 ? (
          <div className="min-w-0">
            <Select items={items} value={selectedMachineId} disabled={disabled} onValueChange={(value) => { if (value) void onMachineChange(value); }}>
              <SelectTrigger id="current-machine" className="w-full" aria-label="Current machine">
                {selectedMachine.kind === "ssh" ? <ServerIcon aria-hidden="true" /> : <LaptopIcon aria-hidden="true" />}
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="start" alignItemWithTrigger={false}>
                <SelectGroup>
                  {enabledMachines.map((machine) => (
                    <SelectItem key={machine.id} value={machine.id}>
                      {machine.kind === "ssh" ? <ServerIcon aria-hidden="true" /> : <LaptopIcon aria-hidden="true" />}
                      {machine.name}
                    </SelectItem>
                  ))}
                </SelectGroup>
              </SelectContent>
            </Select>
          </div>
        ) : null}
        <div className="min-w-0">
          <MachineConnectionIndicator machine={selectedMachine} connection={connection} onRetry={onRetry} />
        </div>
        <div className="flex min-w-0 items-center gap-1">
          <Button variant="ghost" size="sm" className="min-w-0 flex-1 justify-start font-normal" onClick={onOpenSettings}>
            <SettingsIcon data-icon="inline-start" aria-hidden="true" />
            Settings
          </Button>
        </div>
      </div>
    </aside>
  );
}

function MachineConnectionIndicator({
  machine,
  connection,
  onRetry,
}: {
  machine: MachineProfile;
  connection: MachineConnection | null;
  onRetry: () => void;
}) {
  if (machine.kind === "local") return null;

  const status = connection?.status ?? "checking";
  const checked = connection?.checkedAtMs
    ? new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit", second: "2-digit" }).format(connection.checkedAtMs)
    : "not yet";
  const tooltip = status === "online"
    ? "Connected in " + (connection?.latencyMs ?? 0) + " ms · checked " + checked
      + "\n" + (connection?.agent?.operatingSystem ?? "Remote") + " "
      + (connection?.agent?.architecture ?? "") + " · " + (connection?.agent?.gitVersion ?? "Git unavailable")
    : status === "offline"
      ? (connection?.error ?? "The machine did not respond.") + "\nLast checked " + checked
      : "Negotiating a secure Repola agent connection…";

  return (
    <TooltipButton
      variant="ghost"
      size="xs"
      className={cn(
        "mt-1 h-7 w-full justify-start gap-2 px-1 font-normal",
        status === "online" && "text-success hover:text-success",
        status === "offline" && "text-destructive hover:text-destructive",
        status === "checking" && "text-muted-foreground",
      )}
      disabled={status === "checking"}
      onClick={onRetry}
      tooltip={<span className="whitespace-pre-line">{tooltip}</span>}
      aria-label={status === "online"
        ? "Remote machine online, " + (connection?.latencyMs ?? 0) + " milliseconds"
        : status === "offline"
          ? "Remote machine offline. Test connection again"
          : "Testing remote machine connection"}
    >
      {status === "checking"
        ? <Spinner />
        : status === "online"
          ? <WifiIcon aria-hidden="true" />
          : <WifiOffIcon aria-hidden="true" />}
      {status === "checking" ? "Connecting…" : status === "online" ? (connection?.latencyMs ?? 0) + " ms latency" : "Offline · Retry"}
    </TooltipButton>
  );
}
