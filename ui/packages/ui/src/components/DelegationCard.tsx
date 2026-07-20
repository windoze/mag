import { GitBranch } from "lucide-react";

import { cn } from "../lib/utils";

import type { DelegationItemView } from "./types";

/** Props for the inline delegation lifecycle card. */
export interface DelegationCardProps {
  /** Delegation lifecycle snapshot to render. */
  readonly delegation: DelegationItemView;
  /** Called when the card is opened for drill-down; omit to render a static card. */
  readonly onOpen?: (delegationId: string) => void;
  /** Additional class names. */
  readonly className?: string;
}

/** Renders one delegate's name, status, task/output, and token usage inline in a thread. */
export function DelegationCard({
  className,
  delegation,
  onOpen
}: DelegationCardProps): React.JSX.Element {
  const body = (
    <>
      <GitBranch className="mt-0.5 h-4 w-4 text-muted-foreground" />
      <div className="min-w-0 flex-1 space-y-1">
        <div className="flex flex-wrap items-center gap-2">
          <span className="font-medium">{delegation.delegate}</span>
          <span className={delegationStatusClass(delegation.status)}>{delegation.status}</span>
        </div>
        {delegation.task !== undefined ? (
          <p className="text-sm text-muted-foreground">Task: {delegation.task}</p>
        ) : null}
        {delegation.output !== undefined ? <p className="text-sm">{delegation.output}</p> : null}
        {delegation.message !== undefined ? (
          <p className="text-sm text-muted-foreground">{delegation.message}</p>
        ) : null}
        {delegation.usage !== undefined ? (
          <p className="text-xs text-muted-foreground">Usage: {formatUsage(delegation.usage)}</p>
        ) : null}
      </div>
    </>
  );
  const cardClass = cn(
    "flex w-full items-start gap-3 rounded-xl border border-border bg-card p-4 text-left shadow-sm",
    className
  );

  // Renders as a button only when a drill-down handler is wired; without one
  // the card is static so it does not advertise a dead affordance.
  if (onOpen === undefined) {
    return <div className={cardClass}>{body}</div>;
  }
  return (
    <button
      className={cn(cardClass, "transition hover:border-primary/40")}
      type="button"
      onClick={() => onOpen(delegation.id)}
    >
      {body}
    </button>
  );
}

/** Pill class names for one delegation lifecycle status. */
export function delegationStatusClass(status: DelegationItemView["status"]): string {
  return cn(
    "rounded-full px-2 py-0.5 text-[11px] font-semibold",
    status === "started" && "bg-primary/10 text-primary",
    status === "finished" && "bg-success/10 text-success",
    status === "failed" && "bg-destructive/10 text-destructive"
  );
}

/** Compact token summary used by delegation cards and panels. */
export function formatUsage(usage: NonNullable<DelegationItemView["usage"]>): string {
  if (usage.total_tokens !== undefined) {
    return `${usage.total_tokens} tokens`;
  }
  const input = usage.input_tokens ?? 0;
  const output = usage.output_tokens ?? 0;
  return `${input + output} tokens`;
}
