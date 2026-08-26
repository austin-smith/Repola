import { Suspense, type ReactNode } from "react";
import { ErrorBoundary } from "@/components/error-boundary";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Spinner } from "@/components/ui/spinner";

/**
 * Hosts a code-split dialog: shows a visible loading state while its chunk downloads and
 * turns a failed chunk load or render error into a closable dialog instead of a blank app.
 */
export function LazyDialog({ onClose, children }: { onClose: () => void; children: ReactNode }) {
  return (
    <ErrorBoundary
      fallback={(error) => (
        <Dialog open onOpenChange={(open) => { if (!open) onClose(); }}>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>Could not open this dialog</DialogTitle>
              <DialogDescription className="break-words font-mono text-xs">{error.message}</DialogDescription>
            </DialogHeader>
            <DialogFooter>
              <Button variant="outline" onClick={onClose}>Close</Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      )}
    >
      <Suspense
        fallback={(
          <div role="status" aria-label="Loading" className="fixed inset-0 z-50 grid place-items-center bg-background/60">
            <Spinner className="size-6" />
          </div>
        )}
      >
        {children}
      </Suspense>
    </ErrorBoundary>
  );
}
