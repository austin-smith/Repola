import { Component, type ErrorInfo, type ReactNode } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";

interface ErrorBoundaryProps {
  children: ReactNode;
  /** Short description of the region that failed, shown in the fallback. */
  label?: string;
  /** Rendered instead of the default fallback when provided. */
  fallback?: (error: Error, reset: () => void) => ReactNode;
  /** Resetting when this value changes lets a boundary recover after navigation. */
  resetKey?: unknown;
}

interface ErrorBoundaryState {
  error: Error | null;
}

function describe(error: unknown): Error {
  return error instanceof Error ? error : new Error(String(error));
}

export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: unknown): ErrorBoundaryState {
    return { error: describe(error) };
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    console.error("Unhandled render error", error, info.componentStack);
  }

  componentDidUpdate(previous: ErrorBoundaryProps) {
    if (this.state.error && previous.resetKey !== this.props.resetKey) {
      this.reset();
    }
  }

  reset = () => {
    this.setState({ error: null });
  };

  render() {
    const { error } = this.state;
    if (!error) {
      return this.props.children;
    }
    if (this.props.fallback) {
      return this.props.fallback(error, this.reset);
    }
    return (
      <div role="alert" className="p-4">
        <Alert variant="destructive">
          <AlertTitle>{this.props.label ? `${this.props.label} failed to render` : "Something went wrong"}</AlertTitle>
          <AlertDescription>
            <p className="break-words font-mono text-xs">{error.message}</p>
            <div className="mt-2 flex gap-2">
              <Button size="sm" variant="outline" onClick={this.reset}>
                Try again
              </Button>
              <Button size="sm" variant="ghost" onClick={() => window.location.reload()}>
                Reload Repola
              </Button>
            </div>
          </AlertDescription>
        </Alert>
      </div>
    );
  }
}
