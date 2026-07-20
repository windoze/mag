import { ArrowLeft, RefreshCw } from "lucide-react";

import { cn } from "../lib/utils";

import { Button } from "./Button";
import type { SourceView } from "./types";

/** Props for the sources table page. */
export interface SourcesViewProps {
  /** Sources to render, in display order. */
  readonly sources: readonly SourceView[];
  /** Called when Probe is clicked (re-detects local agent sources). */
  readonly onProbe: () => void;
  /** Disables the Probe button while a probe is in flight. */
  readonly busy?: boolean;
  /** Last list/probe failure, rendered as an alert row. */
  readonly error?: string;
  /** Called when the back button is clicked. */
  readonly onBack?: () => void;
  /** Additional class names. */
  readonly className?: string;
}

/** Renders the configured/probed source list with a probe action (docs/WEB.md §5.5). */
export function SourcesView({
  busy = false,
  className,
  error,
  onBack,
  onProbe,
  sources
}: SourcesViewProps): React.JSX.Element {
  return (
    <div className={cn("mx-auto flex w-full max-w-5xl flex-col gap-4 p-4", className)}>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-3">
          {onBack !== undefined ? (
            <Button aria-label="Back" size="sm" type="button" variant="ghost" onClick={onBack}>
              <ArrowLeft className="mr-1 h-4 w-4" />
              Back
            </Button>
          ) : null}
          <h2 className="text-lg font-semibold">Sources</h2>
        </div>
        <Button disabled={busy} type="button" variant="outline" onClick={onProbe}>
          <RefreshCw className={cn("mr-1.5 h-4 w-4", busy && "animate-spin")} />
          {busy ? "Probing..." : "Probe"}
        </Button>
      </div>

      {error !== undefined ? (
        <p
          className="rounded-lg border border-destructive/20 bg-destructive/10 px-3 py-2 text-sm text-destructive"
          role="alert"
        >
          {error}
        </p>
      ) : null}

      {sources.length === 0 ? (
        <div className="rounded-xl border border-dashed border-border bg-card p-6 text-center text-sm text-muted-foreground">
          No sources yet. Probe to detect local agents.
        </div>
      ) : (
        <div className="overflow-x-auto rounded-xl border border-border bg-card">
          <table className="w-full text-left text-sm">
            <thead>
              <tr className="border-b border-border text-xs uppercase tracking-wide text-muted-foreground">
                <th className="px-3 py-2 font-semibold">Name</th>
                <th className="px-3 py-2 font-semibold">Kind</th>
                <th className="px-3 py-2 font-semibold">Status</th>
                <th className="px-3 py-2 font-semibold">Version</th>
                <th className="px-3 py-2 font-semibold">Capabilities</th>
              </tr>
            </thead>
            <tbody>
              {sources.map((source) => (
                <tr className="border-b border-border last:border-b-0" key={source.id}>
                  <td className="px-3 py-2 font-medium">{source.name}</td>
                  <td className="px-3 py-2 text-muted-foreground">{source.kind}</td>
                  <td className="px-3 py-2">
                    <AvailabilityBadge available={source.available} />
                  </td>
                  <td className="px-3 py-2 text-muted-foreground">{source.version ?? "—"}</td>
                  <td className="px-3 py-2">
                    {source.capabilities.length === 0 ? (
                      <span className="text-muted-foreground">—</span>
                    ) : (
                      <span className="flex flex-wrap gap-1">
                        {source.capabilities.map((capability) => (
                          <span
                            className="rounded-full bg-muted px-2 py-0.5 text-xs text-muted-foreground"
                            key={capability}
                          >
                            {capability}
                          </span>
                        ))}
                      </span>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

function AvailabilityBadge({ available }: { readonly available: boolean }): React.JSX.Element {
  return (
    <span
      className={cn(
        "rounded-full px-2 py-0.5 text-[11px] font-semibold uppercase tracking-wide",
        available ? "bg-primary/10 text-primary" : "bg-destructive/10 text-destructive"
      )}
    >
      {available ? "available" : "unavailable"}
    </span>
  );
}
