import { Bot, GitBranch, User } from "lucide-react";
import type * as React from "react";

import { cn } from "../lib/utils";

import { InteractionCard } from "./InteractionCard";
import { Markdown } from "./Markdown";
import { OriginBadge } from "./OriginBadge";
import { ToolCallCard } from "./ToolCallCard";
import type {
  DelegationItemView,
  InteractionResponseView,
  RunErrorKindView,
  ThreadItemView,
  ThreadMessageView
} from "./types";

/** Props for the central conversation thread. */
export interface ThreadViewProps {
  /** Ordered items from the store projection. */
  readonly items: readonly ThreadItemView[];
  /** Called when an interaction card submits a response. */
  readonly onRespondInteraction?: (requestId: string, response: InteractionResponseView) => void;
  /** Called when a delegation card is opened. */
  readonly onOpenDelegation?: (delegationId: string) => void;
  /** Empty-state copy. */
  readonly emptyState?: string;
  /** Additional class names. */
  readonly className?: string;
}

/** Renders the ordered message/tool/interaction thread without knowing about transports. */
export function ThreadView({
  className,
  emptyState = "Start a conversation to see the thread here.",
  items,
  onOpenDelegation,
  onRespondInteraction
}: ThreadViewProps): React.JSX.Element {
  if (items.length === 0) {
    return (
      <div className={cn("flex min-h-96 items-center justify-center p-8", className)}>
        <div className="max-w-sm rounded-xl border border-dashed border-border bg-card p-6 text-center text-sm text-muted-foreground">
          {emptyState}
        </div>
      </div>
    );
  }

  return (
    <div className={cn("space-y-4 p-4", className)}>
      {items.map((item) => {
        switch (item.type) {
          case "message":
            return <MessageBubble key={item.message.id} message={item.message} />;
          case "tool_call":
            return (
              <ToolCallCard
                key={item.toolCall.id}
                origin={item.toolCall.origin}
                trace={item.toolCall.trace}
              />
            );
          case "interaction":
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
          case "delegation":
            return (
              <DelegationInlineCard
                delegation={item.delegation}
                key={item.delegation.id}
                onOpen={onOpenDelegation}
              />
            );
          case "pivot":
            return <SystemNotice key={item.notice.id} tone="info" text={pivotText(item.notice)} />;
          case "run_error":
            return (
              <SystemNotice
                key={item.error.id}
                tone={runErrorTone(item.error.kind)}
                text={runErrorText(item.error.kind, item.error.message)}
              />
            );
        }
      })}
    </div>
  );
}

function MessageBubble({ message }: { readonly message: ThreadMessageView }): React.JSX.Element {
  const user = message.role === "user";

  return (
    <article className={cn("flex gap-3", user && "justify-end")}>
      {!user ? <Avatar icon={<Bot className="h-4 w-4" />} /> : null}
      <div className={cn("max-w-[min(48rem,85%)] space-y-2", user && "items-end")}>
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            {user ? "You" : "Mag"}
          </span>
          <OriginBadge origin={message.origin} />
          {message.streaming === true ? (
            <span className="rounded-full bg-primary/10 px-2 py-0.5 text-[11px] font-semibold text-primary">
              streaming
            </span>
          ) : null}
        </div>
        <div
          className={cn(
            "rounded-2xl border px-4 py-3 shadow-sm",
            user ? "border-primary/20 bg-primary text-primary-foreground" : "border-border bg-card"
          )}
        >
          <Markdown className={user ? "text-primary-foreground" : undefined} text={message.text} />
          {message.streaming === true ? (
            <span className="ml-1 inline-block h-4 w-1 animate-pulse bg-current" />
          ) : null}
        </div>
        {message.attachments !== undefined && message.attachments.length > 0 ? (
          <div className="flex flex-wrap gap-2">
            {message.attachments.map((attachment, index) => (
              <span
                className="rounded-full border border-border bg-muted px-2 py-1 text-xs text-muted-foreground"
                key={`${index}-${attachment.name ?? attachment.uri ?? "attachment"}`}
              >
                {attachment.name ?? attachment.uri ?? attachment.mime_type ?? "attachment"}
              </span>
            ))}
          </div>
        ) : null}
      </div>
      {user ? <Avatar icon={<User className="h-4 w-4" />} /> : null}
    </article>
  );
}

function Avatar({ icon }: { readonly icon: React.ReactNode }): React.JSX.Element {
  return (
    <div className="mt-6 flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-muted text-muted-foreground">
      {icon}
    </div>
  );
}

function DelegationInlineCard({
  delegation,
  onOpen
}: {
  readonly delegation: DelegationItemView;
  readonly onOpen?: (delegationId: string) => void;
}): React.JSX.Element {
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
  const cardClass =
    "flex w-full items-start gap-3 rounded-xl border border-border bg-card p-4 text-left shadow-sm";

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

function SystemNotice({
  text,
  tone
}: {
  readonly text: string;
  readonly tone: "danger" | "info" | "muted";
}): React.JSX.Element {
  return (
    <div
      className={cn(
        "mx-auto max-w-2xl rounded-full border px-4 py-2 text-center text-xs font-medium",
        tone === "info" && "border-primary/20 bg-primary/10 text-primary",
        tone === "muted" && "border-border bg-muted text-muted-foreground",
        tone === "danger" && "border-destructive/20 bg-destructive/10 text-destructive"
      )}
    >
      {text}
    </div>
  );
}

function pivotText(notice: Extract<ThreadItemView, { readonly type: "pivot" }>["notice"]): string {
  if (notice.status === "queued") {
    return "Pivot queued for the running turn.";
  }
  if (notice.status === "applied") {
    return "Pivot applied to the running turn.";
  }
  return `Pivot dropped${notice.reason === undefined ? "." : `: ${notice.reason}`}`;
}

function runErrorTone(kind: RunErrorKindView): "danger" | "muted" {
  return kind === "other" ? "danger" : "muted";
}

function runErrorText(kind: RunErrorKindView, message: string): string {
  const label = kind.replaceAll("_", " ");
  return `${label}: ${message}`;
}

function delegationStatusClass(status: DelegationItemView["status"]): string {
  return cn(
    "rounded-full px-2 py-0.5 text-[11px] font-semibold",
    status === "started" && "bg-primary/10 text-primary",
    status === "finished" && "bg-success/10 text-success",
    status === "failed" && "bg-destructive/10 text-destructive"
  );
}

function formatUsage(usage: NonNullable<DelegationItemView["usage"]>): string {
  if (usage.total_tokens !== undefined) {
    return `${usage.total_tokens} tokens`;
  }
  const input = usage.input_tokens ?? 0;
  const output = usage.output_tokens ?? 0;
  return `${input + output} tokens`;
}
