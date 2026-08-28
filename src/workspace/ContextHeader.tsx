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

/** Caption-over-value trigger used by every select in the toolbar. */
export function ToolbarSelectTrigger({
  id,
  caption,
  icon,
  placeholder,
  children,
  className,
  ...props
}: Omit<React.ComponentProps<typeof SelectTrigger>, "children"> & {
  caption: string;
  icon: ReactNode;
  placeholder?: string;
  children?: ReactNode;
}) {
  return (
    <SelectTrigger
      id={id}
      size="sm"
      className={cn("h-10 min-w-0 flex-1 gap-2 border-transparent bg-transparent py-1 pr-1.5 pl-2 hover:bg-muted data-popup-open:bg-muted dark:bg-transparent dark:hover:bg-muted/50", className)}
      {...props}
    >
      <span className="shrink-0 text-muted-foreground [&_svg]:size-4" aria-hidden="true">{icon}</span>
      <span className="flex min-w-0 flex-1 flex-col text-left leading-tight">
        <span className="text-[10px] font-medium tracking-wide text-muted-foreground uppercase">{caption}</span>
        <SelectValue className="block truncate text-sm" placeholder={placeholder}>{children}</SelectValue>
      </span>
    </SelectTrigger>
  );
}

/**
 * The single toolbar row: machine, then the repository context controls
 * passed as children, then the workspace status and global actions.
 */
export function ContextHeader({
  machines,
  selectedMachine,
  selectedMachineId,
  connection,
  disabled,
  onChange,
  onRetry,
  status,
  actions,
  children,
}: {
  machines: MachineProfile[];
  selectedMachine: MachineProfile;
  selectedMachineId: string;
  connection: MachineConnection | null;
  disabled: boolean;
  onChange: (machineId: string) => Promise<void>;
  onRetry: () => void;
  status?: ReactNode;
  actions?: ReactNode;
  children?: ReactNode;
}) {
  const enabledMachines = machines.filter((machine) => machine.enabled);
  const items = Object.fromEntries(enabledMachines.map((machine) => [machine.id, machine.name]));
  // A single local machine is the common case; the selector only earns its
  // width once there is something to switch to.
  const showMachine = enabledMachines.length > 1 || selectedMachine.kind === "ssh";
  const machineTrigger = (
    <ToolbarSelectTrigger id="current-machine" caption="Machine" icon={selectedMachine.kind === "ssh" ? <ServerIcon /> : <LaptopIcon />} className="w-40 flex-none" aria-label="Current machine" />
  );
  return (
    <header className="flex h-14 shrink-0 items-center gap-1 border-b bg-card pr-2 pl-3">
      <div className="mr-1 grid size-6 shrink-0 content-center gap-[3px] border border-foreground bg-brand p-[5px]" aria-label="Repola" role="img">
        <span className="block h-px bg-brand-foreground" />
        <span className="block h-px bg-brand-foreground" />
        <span className="block h-px bg-brand-foreground" />
      </div>
      {showMachine ? (
        <Select items={items} value={selectedMachineId} disabled={disabled} onValueChange={(value) => { if (value) void onChange(value); }}>
          {disabled ? (
            <Tooltip>
              <TooltipTrigger render={<span className="inline-flex" tabIndex={0} />}>
                {machineTrigger}
              </TooltipTrigger>
              <TooltipContent>Finish the current operation before switching machines.</TooltipContent>
            </Tooltip>
          ) : machineTrigger}
          <SelectContent>
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
      ) : null}
      {selectedMachine.kind === "ssh" ? (
        <MachineConnectionIndicator connection={connection} onRetry={onRetry} />
      ) : null}
      {children}
      {status ? <div className="ml-2 flex shrink-0 items-center gap-2 text-xs text-muted-foreground">{status}</div> : null}
      {actions ? <div className="ml-1 flex shrink-0 items-center gap-0.5">{actions}</div> : null}
    </header>
  );
}

function MachineConnectionIndicator({
  connection,
  onRetry,
}: {
  connection: MachineConnection | null;
  onRetry: () => void;
}) {
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
