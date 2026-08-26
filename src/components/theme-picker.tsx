import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { useTheme, type Theme } from "@/components/theme-provider";
import { cn } from "@/lib/utils";

const options: { value: Theme; label: string }[] = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
];

function isTheme(value: unknown): value is Theme {
  return value === "system" || value === "light" || value === "dark";
}

/**
 * A miniature of the app window drawn with the real theme tokens. Wrapping it
 * in `.light` or `.dark` re-resolves every token underneath, so the preview is
 * always the palette it claims to be, whatever the app is currently using.
 */
function WindowMiniature({ palette, className }: { palette: "light" | "dark"; className?: string }) {
  return (
    <div className={cn(palette, "absolute inset-0 flex flex-col bg-background text-foreground", className)} aria-hidden="true">
      <div className="flex h-[18%] items-center gap-1 border-b bg-card px-[6%]">
        <span className="size-[7px] border border-foreground bg-brand" />
        <span className="h-[3px] w-[22%] bg-foreground/70" />
      </div>
      <div className="flex min-h-0 flex-1">
        <div className="flex w-[30%] flex-col gap-[6%] border-r bg-sidebar p-[6%]">
          <span className="h-[3px] w-[80%] bg-muted-foreground/60" />
          <span className="h-[3px] w-[60%] bg-muted-foreground/40" />
          <span className="h-[3px] w-[70%] bg-muted-foreground/40" />
        </div>
        <div className="flex flex-1 flex-col gap-[5%] p-[6%]">
          {[0, 1, 2].map((row) => (
            <div key={row} className="flex h-[16%] items-center gap-[5%] border bg-card px-[5%]">
              <span className={cn("h-[3px] w-[35%]", row === 0 ? "bg-brand" : "bg-foreground/60")} />
              <span className="ml-auto h-[3px] w-[18%] bg-muted-foreground/50" />
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function ThemePreview({ theme }: { theme: Theme }) {
  return (
    <div className="relative aspect-[16/10] w-full overflow-hidden border bg-background">
      {theme === "system" ? (
        <>
          <WindowMiniature palette="light" />
          <WindowMiniature palette="dark" className="[clip-path:polygon(100%_0,100%_100%,0_100%)]" />
        </>
      ) : (
        <WindowMiniature palette={theme} />
      )}
    </div>
  );
}

export function ThemePicker({ labelledBy }: { labelledBy: string }) {
  const { theme, setTheme } = useTheme();
  return (
    <RadioGroup
      aria-labelledby={labelledBy}
      value={theme}
      onValueChange={(value) => {
        if (isTheme(value)) setTheme(value);
      }}
      className="grid-cols-3 gap-3"
    >
      {options.map((option) => (
        <label
          key={option.value}
          className="group/theme-tile flex cursor-pointer flex-col gap-2 outline-none"
        >
          <div
            className={cn(
              "transition-[box-shadow] group-has-data-checked/theme-tile:ring-2 group-has-data-checked/theme-tile:ring-brand group-has-data-checked/theme-tile:ring-offset-2 group-has-data-checked/theme-tile:ring-offset-background",
              "group-hover/theme-tile:ring-2 group-hover/theme-tile:ring-border",
            )}
          >
            <ThemePreview theme={option.value} />
          </div>
          <span className="flex items-center gap-2 text-sm">
            <RadioGroupItem value={option.value} />
            {option.label}
          </span>
        </label>
      ))}
    </RadioGroup>
  );
}
