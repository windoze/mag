import { GitBranch, X } from "lucide-react";

import { cn } from "../lib/utils";

import { delegationStatusClass, formatUsage } from "./DelegationCard";
import { InteractionCard } from "./InteractionCard";
import { Markdown } from "./Markdown";
import { OriginBadge } from "./OriginBadge";
import type {
  DelegationItemView,
  DelegationStatusView,
  InteractionItemView,
  InteractionResponseView
} from "./types";

/** Plain delegated-agent message rendered inside the sub-thread panel. */
export interface DelegateThreadMessageView {
  /** Stable item identity. */
  readonly id: string;
  /** Message text emitted by the delegated agent. */
  readonly text: string;
}

/** One item in a delegate's sub-thread, in arrival order. */
export type DelegateThreadItemView =
  | { readonly type: "message"; readonly message: DelegateThreadMessageView }
  | { readonly type: "interaction"; readonly interaction: InteractionItemView };

/** Props for the right-rail delegate sub-thread panel. */
export interface DelegateThreadPanelProps {
  /** Delegate name shown in the panel header. */
  readonly delegate: string;
  /** Delegation depth derived from attributed interactions, when known. */
  readonly depth?: number;
  /** Latest lifecycle status of the delegate, when a trace exists. */
  readonly status?: DelegationStatusView;
  /** Latest token usage reported by the delegate, when known. */
  readonly usage?: DelegationItemView["usage"];
  /** Arrival-ordered messages and interactions attributed to the delegate. */
  readonly items: readonly DelegateThreadItemView[];
  /** Called when the panel close button is pressed. */
  readonly onClose?: () => void;
  /** Called when an interaction card inside the panel submits a response. */
  readonly onRespondInteraction?: (requestId: string, response: InteractionResponseView) => void;
  /** Additional class names. */
  readonly className?: string;
}

/**
 * Renders one delegate's sub-thread: the activity the wire attributes to the
 * delegate (decision D5) — delegation messages and origin-tagged interaction
 * cards carrying their `[from <delegate>@depth<n>]` badges.
 */
export function DelegateThreadPanel({
  className,
  delegate,
  depth,
  items,
  onClose,
  onRespondInteraction,
  status,
  usage
}: DelegateThreadPanelProps): React.JSX.Element {
  const origin = depth === undefined ? undefined : { delegate, depth };

  return (
    <section
      className={cn("flex flex-col rounded-xl border border-border bg-background", className)}
      aria-label={`Delegate ${delegate} sub-thread`}
    >
      <header className="flex items-start gap-2 border-b border-border p-3">
        <GitBranch className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" />
        <div className="min-w-0 flex-1 space-y-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="truncate text-sm font-semibold">{delegate}</span>
            {status !== undefined ? (
              <span className={delegationStatusClass(status)}>{status}</span>
            ) : null}
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <OriginBadge origin={origin} />
            {usage !== undefined ? (
              <span className="text-[11px] text-muted-foreground">{formatUsage(usage)}</span>
            ) : null}
          </div>
        </div>
        {onClose !== undefined ? (
          <button
            aria-label={`Close ${delegate} sub-thread`}
            className="rounded-md p-1 text-muted-foreground transition hover:bg-muted hover:text-foreground"
            type="button"
            onClick={onClose}
          >
            <X className="h-4 w-4" />
          </button>
        ) : null}
      </header>
      <div className="space-y-3 p-3">
        {items.length === 0 ? (
          <p className="rounded-lg border border-dashed border-border p-3 text-center text-xs text-muted-foreground">
            No delegated activity yet.
          </p>
        ) : (
          items.map((item) => {
            if (item.type === "message") {
              return (
                <div className="space-y-1" key={item.message.id}>
                  <OriginBadge origin={origin} />
                  <div className="rounded-lg border border-border bg-card px-3 py-2 text-sm">
                    <Markdown text={item.message.text} />
                  </div>
                </div>
              );
            }
            return (
              <InteractionCard
                key={item.interaction.requestId}
                kind={item.interaction.kind}
                origin={item.interaction.origin}
                requestId={item.interaction.requestId}
                response={item.interaction.response}
                status={item.interaction.status}
                onSubmit={(response, requestId) => onRespondInteraction?.(requestId, response)}
              />
            );
          })
        )}
      </div>
    </section>
  );
}
