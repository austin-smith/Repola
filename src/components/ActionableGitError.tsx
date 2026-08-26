import { AlertTriangleIcon } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { classifyGitError } from "../domain/git-errors";

export function ActionableGitError({ message, className }: { message: string; className?: string }) {
  const guidance = classifyGitError(message);
  return (
    <Alert variant="destructive" className={className} role="alert">
      <AlertTriangleIcon aria-hidden="true" />
      <AlertTitle>{guidance.title}</AlertTitle>
      <AlertDescription>
        <ul className="list-disc space-y-1 pl-4">
          {guidance.guidance.map((item) => <li key={item}>{item}</li>)}
        </ul>
        <details open className="mt-3 border bg-background/70 p-2">
          <summary className="cursor-pointer text-xs font-medium">Complete Git output</summary>
          <pre className="mt-2 max-h-40 overflow-auto font-mono text-xs whitespace-pre-wrap break-words">{message}</pre>
        </details>
      </AlertDescription>
    </Alert>
  );
}
