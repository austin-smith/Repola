import { useEffect, useState } from "react";
import { DownloadIcon, RefreshCwIcon, XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { TooltipButton } from "@/components/tooltip-button";
import { Spinner } from "@/components/ui/spinner";
import { checkForUpdates, installAvailableUpdate, useUpdaterState } from "./updater";

function progressLabel(downloadedBytes: number, totalBytes: number | null): string {
  const downloaded = Math.max(0, downloadedBytes);
  if (!totalBytes || totalBytes <= 0) return `${Math.round(downloaded / 1024 / 1024)} MB downloaded`;
  return `${Math.min(100, Math.round((downloaded / totalBytes) * 100))}%`;
}

export function AppUpdater() {
  const updater = useUpdaterState();
  const [dismissedVersion, setDismissedVersion] = useState<string | null>(null);

  useEffect(() => {
    const timer = window.setTimeout(() => void checkForUpdates(false), 15_000);
    return () => window.clearTimeout(timer);
  }, []);

  const version = "version" in updater ? updater.version : null;
  const visible = (updater.status === "available" && dismissedVersion !== updater.version)
    || updater.status === "installing"
    || updater.status === "restarting"
    || (updater.status === "error" && updater.context === "install");
  if (!visible) return null;

  return (
    <aside aria-live="polite" className="fixed right-4 bottom-4 z-[140] w-[min(24rem,calc(100vw-2rem))] border bg-popover p-4 text-popover-foreground shadow-xl">
      <div className="flex items-start gap-3">
        <div className="mt-0.5 grid size-8 shrink-0 place-items-center bg-brand/10 text-brand">
          {updater.status === "installing" || updater.status === "restarting" ? <Spinner className="size-4" /> : <DownloadIcon className="size-4" aria-hidden="true" />}
        </div>
        <div className="min-w-0 flex-1">
          <strong className="text-sm">
            {updater.status === "available" ? `Repola ${updater.version} is ready`
              : updater.status === "installing" ? `Installing Repola ${updater.version}`
                : updater.status === "restarting" ? "Restarting Repola…"
                  : "Update could not be installed"}
          </strong>
          <p className="mt-1 text-xs text-muted-foreground">
            {updater.status === "available" ? (updater.notes || "A signed update is available to download and install.")
              : updater.status === "installing" ? progressLabel(updater.downloadedBytes, updater.totalBytes)
                : updater.status === "restarting" ? "The verified update is installed."
                  : updater.message}
          </p>
          {updater.status === "installing" && updater.totalBytes ? (
            <div className="mt-2 h-1.5 overflow-hidden bg-muted" aria-hidden="true">
              <div className="h-full bg-brand transition-[width]" style={{ width: `${Math.min(100, (updater.downloadedBytes / updater.totalBytes) * 100)}%` }} />
            </div>
          ) : null}
          {(updater.status === "available" || updater.status === "error") && (
            <div className="mt-3 flex gap-2">
              <Button size="sm" onClick={() => void installAvailableUpdate()}>
                {updater.status === "error" ? <RefreshCwIcon data-icon="inline-start" aria-hidden="true" /> : <DownloadIcon data-icon="inline-start" aria-hidden="true" />}
                {updater.status === "error" ? "Retry" : "Install and Restart"}
              </Button>
              {version ? <Button variant="ghost" size="sm" onClick={() => setDismissedVersion(version)}>Later</Button> : null}
            </div>
          )}
        </div>
        {version && updater.status === "available" ? (
          <TooltipButton variant="ghost" size="icon-xs" aria-label="Dismiss update" tooltip="Dismiss update" onClick={() => setDismissedVersion(version)}>
            <XIcon aria-hidden="true" />
          </TooltipButton>
        ) : null}
      </div>
    </aside>
  );
}

export function UpdateSettings() {
  const updater = useUpdaterState();
  const checking = updater.status === "checking";
  const installing = updater.status === "installing" || updater.status === "restarting";
  return (
    <div className="flex items-start justify-between gap-4 border bg-card p-3">
      <div className="min-w-0">
        <strong className="text-sm">Software updates</strong>
        <p className={`mt-1 text-xs ${updater.status === "error" ? "text-destructive" : "text-muted-foreground"}`}>
          {updater.status === "checking" ? "Checking the signed release channel…"
            : updater.status === "upToDate" ? "Repola is up to date."
              : updater.status === "available" ? `Repola ${updater.version} is available.`
                : updater.status === "installing" ? `Installing ${updater.version} · ${progressLabel(updater.downloadedBytes, updater.totalBytes)}`
                  : updater.status === "restarting" ? "Update installed; restarting…"
                    : updater.status === "error" ? updater.message
                      : "Checks use Repola's pinned signing key and release channel."}
        </p>
      </div>
      {updater.status === "available" || (updater.status === "error" && updater.context === "install") ? (
        <Button size="sm" disabled={installing} onClick={() => void installAvailableUpdate()}>
          <DownloadIcon data-icon="inline-start" aria-hidden="true" />Install and Restart
        </Button>
      ) : (
        <Button variant="outline" size="sm" disabled={checking || installing} onClick={() => void checkForUpdates(true)}>
          {checking ? <Spinner data-icon="inline-start" /> : <RefreshCwIcon data-icon="inline-start" aria-hidden="true" />}
          Check Now
        </Button>
      )}
    </div>
  );
}
