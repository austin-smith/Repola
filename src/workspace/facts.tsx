import type { ReactNode } from "react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { sectionHeadingClass } from "./labels";

export function DetailSection({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-2.5">
      <h3 className={sectionHeadingClass}>{title}</h3>
      {children}
    </section>
  );
}

export function StatusFact({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex flex-col gap-1 bg-card p-2">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="m-0 font-mono text-sm tabular-nums">{value}</dd>
    </div>
  );
}

export function ActivityFact({ label, value, tooltip }: { label: string; value: string; tooltip?: string }) {
  return (
    <div className="grid grid-cols-[96px_1fr] gap-2.5 border-b py-2">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      {tooltip ? (
        <Tooltip>
          <TooltipTrigger render={<dd className="m-0 text-right text-xs break-all" tabIndex={0} />}>
            {value}
          </TooltipTrigger>
          <TooltipContent>{tooltip}</TooltipContent>
        </Tooltip>
      ) : <dd className="m-0 text-right text-xs break-all">{value}</dd>}
    </div>
  );
}
