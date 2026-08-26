import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ThemeProvider } from "@/components/theme-provider";
import { EnvironmentProvider } from "./environment";
import { Toaster } from "@/components/ui/toast";
import { TooltipProvider } from "@/components/ui/tooltip";
import { AppUpdater } from "./AppUpdater";
import { ErrorBoundary } from "@/components/error-boundary";
import "./index.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ThemeProvider>
      <ErrorBoundary label="Repola">
        <EnvironmentProvider>
          <TooltipProvider>
            <Toaster>
              <ErrorBoundary label="The workspace">
                <App />
              </ErrorBoundary>
              <AppUpdater />
            </Toaster>
          </TooltipProvider>
        </EnvironmentProvider>
      </ErrorBoundary>
    </ThemeProvider>
  </React.StrictMode>,
);
