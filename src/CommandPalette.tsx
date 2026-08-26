import { useEffect, useMemo, useState } from "react";
import { SearchIcon } from "lucide-react";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import { cn } from "@/lib/utils";

export interface CommandPaletteItem {
  id: string;
  label: string;
  detail: string;
  shortcut?: string;
  disabled?: boolean;
  run: () => void;
}

export function CommandPalette({ open, commands, onOpenChange }: { open: boolean; commands: CommandPaletteItem[]; onOpenChange: (open: boolean) => void }) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const visible = useMemo(() => {
    const normalized = query.trim().toLowerCase();
    return commands.filter((command) => !normalized || `${command.label} ${command.detail}`.toLowerCase().includes(normalized));
  }, [commands, query]);

  useEffect(() => {
    if (open) {
      setQuery("");
      setActive(0);
    }
  }, [open]);
  useEffect(() => setActive((value) => Math.min(value, Math.max(visible.length - 1, 0))), [visible.length]);

  const run = (command: CommandPaletteItem | undefined) => {
    if (!command || command.disabled) return;
    onOpenChange(false);
    command.run();
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="top-[18%] translate-y-0 gap-2 p-2 sm:max-w-xl" showCloseButton={false}>
        <DialogHeader className="sr-only">
          <DialogTitle>Command Palette</DialogTitle>
          <DialogDescription>Search and run a Repola command.</DialogDescription>
        </DialogHeader>
        <InputGroup className="h-11 border-0 shadow-none">
          <InputGroupAddon><SearchIcon aria-hidden="true" /></InputGroupAddon>
          <InputGroupInput autoFocus value={query} placeholder="Type a command…" aria-label="Command search" onChange={(event) => { setQuery(event.currentTarget.value); setActive(0); }} onKeyDown={(event) => {
            if (event.key === "ArrowDown") { event.preventDefault(); setActive((value) => visible.length ? (value + 1) % visible.length : 0); }
            else if (event.key === "ArrowUp") { event.preventDefault(); setActive((value) => visible.length ? (value - 1 + visible.length) % visible.length : 0); }
            else if (event.key === "Enter") { event.preventDefault(); run(visible[active]); }
          }} />
        </InputGroup>
        <div className="max-h-80 overflow-y-auto border-t pt-1" role="listbox" aria-label="Commands">
          {visible.map((command, index) => (
            <button key={command.id} type="button" role="option" aria-selected={index === active} disabled={command.disabled} className={cn("flex w-full items-center gap-3 rounded-sm px-2.5 py-2 text-left outline-none", index === active && "bg-accent text-accent-foreground", command.disabled && "opacity-45")} onMouseMove={() => setActive(index)} onClick={() => run(command)}>
              <span className="min-w-0 flex-1"><strong className="block text-sm font-medium">{command.label}</strong><span className="block truncate text-xs text-muted-foreground">{command.detail}</span></span>
              {command.shortcut && <kbd className="font-mono text-[11px] text-muted-foreground">{command.shortcut}</kbd>}
            </button>
          ))}
          {visible.length === 0 && <p className="p-5 text-center text-sm text-muted-foreground">No matching commands.</p>}
        </div>
      </DialogContent>
    </Dialog>
  );
}
