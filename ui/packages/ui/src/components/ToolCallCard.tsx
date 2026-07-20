import { ChevronRight } from "lucide-react";

import { cn } from "../lib/utils";

import { OriginBadge } from "./OriginBadge";
import type { JsonValue, OriginAttribution, ToolStatusView, ToolTraceView } from "./types";

/** Props for the collapsible tool-call card. */
export interface ToolCallCardProps {
  /** Latest tool trace to display. */
  readonly trace: ToolTraceView;
  /** Optional delegated origin badge. */
  readonly origin?: OriginAttribution;
  /** Whether the details section starts expanded. */
  readonly defaultExpanded?: boolean;
  /** Additional class names. */
  readonly className?: string;
}

/** Renders a tool call with lifecycle badge, summaries, and expandable JSON details. */
export function ToolCallCard({
  className,
  defaultExpanded = false,
  origin,
  trace
}: ToolCallCardProps): React.JSX.Element {
  const inputSummary = summarizeJson(trace.input);
  const outputSummary = summarizeJson(trace.output);

  return (
    <details
      className={cn(
        "group rounded-xl border border-border bg-card text-card-foreground shadow-sm",
        className
      )}
      open={defaultExpanded}
    >
      <summary className="flex cursor-pointer list-none items-start gap-3 p-4 marker:hidden">
        <ChevronRight className="mt-1 h-4 w-4 shrink-0 text-muted-foreground transition-transform group-open:rotate-90" />
        <div className="min-w-0 flex-1 space-y-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium">{trace.name}</span>
            <StatusBadge status={trace.status} />
            <OriginBadge origin={origin} />
          </div>
          <p className="truncate text-xs text-muted-foreground">call {trace.call_id}</p>
          {inputSummary !== undefined ? (
            <p className="line-clamp-2 text-sm text-muted-foreground">Input: {inputSummary}</p>
          ) : null}
          {trace.message !== undefined ? (
            <p className="text-sm text-muted-foreground">{trace.message}</p>
          ) : null}
        </div>
      </summary>
      <div className="space-y-3 border-t border-border px-4 pb-4 pt-3">
        <JsonPanel label="Input" value={trace.input} />
        <JsonPanel label="Output" value={trace.output} emptyLabel="No output yet" />
        {outputSummary !== undefined ? (
          <p className="text-xs text-muted-foreground">Output summary: {outputSummary}</p>
        ) : null}
      </div>
    </details>
  );
}

function StatusBadge({ status }: { readonly status: ToolStatusView }): React.JSX.Element {
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1.5 rounded-full px-2 py-0.5 text-[11px] font-semibold capitalize",
        status === "started" && "bg-primary/10 text-primary",
        status === "finished" && "bg-success/10 text-success",
        status === "denied" && "bg-warning/15 text-warning",
        status === "cancelled" && "bg-muted text-muted-foreground",
        status === "failed" && "bg-destructive/10 text-destructive"
      )}
    >
      {status === "started" ? (
        <span
          aria-hidden
          className="inline-block h-2.5 w-2.5 animate-spin rounded-full border border-current border-t-transparent"
        />
      ) : null}
      {status.replace("_", " ")}
    </span>
  );
}

function JsonPanel({
  emptyLabel = "Not provided",
  label,
  value
}: {
  readonly emptyLabel?: string;
  readonly label: string;
  readonly value?: JsonValue;
}): React.JSX.Element {
  return (
    <section className="space-y-1">
      <h4 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        {label}
      </h4>
      <pre className="max-h-72 overflow-auto rounded-md bg-muted p-3 text-xs leading-5">
        <code>{value === undefined ? emptyLabel : stringifyJson(value)}</code>
      </pre>
    </section>
  );
}

function summarizeJson(value: JsonValue | undefined): string | undefined {
  if (value === undefined) {
    return undefined;
  }

  const text = stringifyJson(value).replace(/\s+/g, " ").trim();
  return text.length > 140 ? `${text.slice(0, 137)}...` : text;
}

function stringifyJson(value: JsonValue): string {
  return typeof value === "string" ? value : JSON.stringify(value, null, 2);
}
