import type { ReactNode } from "react";
import { LaptopIcon, ServerIcon, WifiIcon, WifiOffIcon } from "lucide-react";
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { TooltipButton } from "@/components/tooltip-button";
import { cn } from "@/lib/utils";
import type { AgentInfo, MachineProfile } from "../ipc/types";

export type MachineConnectionStatus = "checking" | "online" | "offline";

export interface MachineConnection {
  status: MachineConnectionStatus;
  latencyMs: number | null;
  checkedAtMs: number | null;
  lastConnectedAtMs: number | null;
  agent: AgentInfo | null;
  error: string | null;
}

export function ContextHeader({
  machines,
  selectedMachine,
  selectedMachineId,
  connection,
  disabled,
  onChange,
  onRetry,
  children,
}: {
  machines: MachineProfile[];
  selectedMachine: MachineProfile;
  selectedMachineId: string;
  connection: MachineConnection | null;
  disabled: boolean;
  onChange: (machineId: string) => Promise<void>;
  onRetry: () => void;
  children: ReactNode;
}) {
  const items = Object.fromEntries(
    machines.filter((machine) => machine.enabled).map((machine) => [machine.id, machine.name]),
  );
  const machineTrigger = (
    <SelectTrigger className={disabled ? "w-full" : "ml-3 w-52"} aria-label="Current machine">
      {selectedMachine.kind === "ssh"
        ? <ServerIcon className="size-3.5 text-muted-foreground" aria-hidden="true" />
        : <LaptopIcon className="size-3.5 text-muted-foreground" aria-hidden="true" />}
      <SelectValue />
    </SelectTrigger>
  );
  return (
    <header className="flex h-13 shrink-0 items-center gap-3 border-b bg-card px-5">
      <div className="grid size-6 shrink-0 content-center gap-[3px] border border-foreground bg-brand p-[5px]" aria-hidden="true">
        <span className="block h-px bg-brand-foreground" />
        <span className="block h-px bg-brand-foreground" />
        <span className="block h-px bg-brand-foreground" />
      </div>
      <h1 className="text-sm leading-none font-medium">Repola</h1>
      <Select items={items} value={selectedMachineId} disabled={disabled} onValueChange={(value) => { if (value) void onChange(value); }}>
        {disabled ? (
          <Tooltip>
            <TooltipTrigger render={<span className="ml-3 inline-flex w-52" tabIndex={0} />}>
              {machineTrigger}
            </TooltipTrigger>
            <TooltipContent>Finish the current operation before switching machines.</TooltipContent>
          </Tooltip>
        ) : machineTrigger}
        <SelectContent>
          <SelectGroup>
            {machines.filter((machine) => machine.enabled).map((machine) => (
              <SelectItem key={machine.id} value={machine.id}>
                {machine.kind === "ssh" ? <ServerIcon aria-hidden="true" /> : <LaptopIcon aria-hidden="true" />}
                {machine.name}
              </SelectItem>
            ))}
          </SelectGroup>
        </SelectContent>
      </Select>
      <MachineConnectionIndicator
        machine={selectedMachine}
        connection={connection}
        onRetry={onRetry}
      />
      {children}
    </header>
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
  if (machine.kind === "local") {
    return (
      <Tooltip>
        <TooltipTrigger render={<span className="flex items-center gap-1.5 text-xs text-muted-foreground" tabIndex={0} />}>
          <span className="size-1.5 rounded-full bg-success" aria-hidden="true" />
          Local
        </TooltipTrigger>
        <TooltipContent>Git operations run directly on this computer.</TooltipContent>
      </Tooltip>
    );
  }

  const status = connection?.status ?? "checking";
  const checked = connection?.checkedAtMs
    ? new Date(connection.checkedAtMs).toLocaleTimeString([], { hour: "numeric", minute: "2-digit", second: "2-digit" })
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
        "gap-1.5 px-2 font-normal",
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
        ? <Spinner className="size-3" />
        : status === "online"
          ? <WifiIcon className="size-3" aria-hidden="true" />
          : <WifiOffIcon className="size-3" aria-hidden="true" />}
      {status === "checking" ? "Connecting…" : status === "online" ? (connection?.latencyMs ?? 0) + " ms" : "Offline"}
    </TooltipButton>
  );
}
