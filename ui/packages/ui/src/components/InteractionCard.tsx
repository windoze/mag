import * as React from "react";

import { cn } from "../lib/utils";

import { Button } from "./Button";
import { OriginBadge } from "./OriginBadge";
import type {
  ApprovalDecisionView,
  InteractionKindView,
  InteractionResponseView,
  JsonValue,
  OriginAttribution,
  PermissionDecisionView
} from "./types";

/** Props for a pending or resolved user-interaction card. */
export interface InteractionCardProps {
  /** Stable request identity. */
  readonly requestId: string;
  /** Interaction payload to render. */
  readonly kind: InteractionKindView;
  /** Optional delegated origin badge. */
  readonly origin?: OriginAttribution;
  /** Card lifecycle. Defaults to pending. */
  readonly status?: "pending" | "responded";
  /** Submitted response for read-only rendering. */
  readonly response?: InteractionResponseView;
  /** Called with the protocol-shaped response when the user submits a decision. */
  readonly onSubmit?: (response: InteractionResponseView, requestId: string) => void;
  /** Additional class names. */
  readonly className?: string;
}

/** Renders approval, question, choice, and permission requests as blocking inline cards. */
export function InteractionCard({
  className,
  kind,
  onSubmit,
  origin,
  requestId,
  response,
  status = "pending"
}: InteractionCardProps): React.JSX.Element {
  const resolved = status === "responded";

  return (
    <section
      className={cn(
        "rounded-xl border border-primary/25 bg-primary/5 p-4 shadow-sm",
        resolved && "border-border bg-card",
        className
      )}
    >
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <span className="rounded-full bg-primary px-2 py-0.5 text-[11px] font-semibold uppercase tracking-wide text-primary-foreground">
          Interaction
        </span>
        <span className="text-xs text-muted-foreground">{requestId}</span>
        <OriginBadge origin={origin} />
        {resolved ? (
          <span className="rounded-full bg-success/10 px-2 py-0.5 text-[11px] font-semibold text-success">
            submitted
          </span>
        ) : null}
      </div>
      {resolved ? (
        <ResolvedInteraction kind={kind} response={response} />
      ) : (
        <PendingInteraction
          kind={kind}
          onSubmit={(nextResponse) => onSubmit?.(nextResponse, requestId)}
        />
      )}
    </section>
  );
}

function PendingInteraction({
  kind,
  onSubmit
}: {
  readonly kind: InteractionKindView;
  readonly onSubmit: (response: InteractionResponseView) => void;
}): React.JSX.Element {
  switch (kind.kind) {
    case "approval":
      return <ApprovalInteraction kind={kind} onSubmit={onSubmit} />;
    case "question":
      return <QuestionInteraction prompt={kind.prompt} onSubmit={onSubmit} />;
    case "choice":
      return <ChoiceInteraction options={kind.options} prompt={kind.prompt} onSubmit={onSubmit} />;
    case "permission":
      return <PermissionInteraction kind={kind} onSubmit={onSubmit} />;
  }
}

function ApprovalInteraction({
  kind,
  onSubmit
}: {
  readonly kind: Extract<InteractionKindView, { readonly kind: "approval" }>;
  readonly onSubmit: (response: InteractionResponseView) => void;
}): React.JSX.Element {
  const submitDecision = (decision: ApprovalDecisionView): void => {
    onSubmit({
      kind: "approval",
      step_id: kind.call_id,
      call_id: kind.call_id,
      decision,
      message: decision === "cancel" ? "interaction cancelled" : undefined
    });
  };

  return (
    <div className="space-y-3">
      <div className="space-y-1">
        <h3 className="font-semibold">Approve tool call</h3>
        <p className="text-sm text-muted-foreground">
          {kind.tool_name ?? "Tool"} is waiting for approval.
        </p>
        {kind.requirement.type === "require_approval" && kind.requirement.reason !== undefined ? (
          <p className="rounded-md bg-card p-2 text-sm">Reason: {kind.requirement.reason}</p>
        ) : null}
        {kind.input !== undefined ? <JsonSummary label="Input" value={kind.input} /> : null}
      </div>
      <div className="flex flex-wrap gap-2">
        <Button size="sm" type="button" onClick={() => submitDecision("approve")}>
          Approve
        </Button>
        <Button size="sm" type="button" variant="outline" onClick={() => submitDecision("deny")}>
          Deny
        </Button>
        <Button size="sm" type="button" variant="ghost" onClick={() => submitDecision("cancel")}>
          Cancel
        </Button>
      </div>
    </div>
  );
}

function QuestionInteraction({
  onSubmit,
  prompt
}: {
  readonly onSubmit: (response: InteractionResponseView) => void;
  readonly prompt: string;
}): React.JSX.Element {
  const [answer, setAnswer] = React.useState("");

  return (
    <form
      className="space-y-3"
      onSubmit={(event) => {
        event.preventDefault();
        if (answer.trim().length > 0) {
          onSubmit({ kind: "answer", text: answer });
        }
      }}
    >
      <label className="block space-y-2">
        <span className="font-semibold">{prompt}</span>
        <textarea
          className="min-h-24 w-full rounded-md border border-input bg-background px-3 py-2 text-sm outline-none focus:ring-2 focus:ring-ring"
          onChange={(event) => setAnswer(event.currentTarget.value)}
          placeholder="Type an answer..."
          value={answer}
        />
      </label>
      <Button disabled={answer.trim().length === 0} size="sm" type="submit">
        Submit answer
      </Button>
    </form>
  );
}

function ChoiceInteraction({
  onSubmit,
  options,
  prompt
}: {
  readonly onSubmit: (response: InteractionResponseView) => void;
  readonly options: readonly string[];
  readonly prompt: string;
}): React.JSX.Element {
  return (
    <div className="space-y-3">
      <h3 className="font-semibold">{prompt}</h3>
      <div className="grid gap-2">
        {options.map((option, index) => (
          <Button
            className="justify-start"
            key={`${index}-${option}`}
            type="button"
            variant="outline"
            onClick={() => onSubmit({ kind: "choice", index })}
          >
            <span className="mr-2 rounded bg-muted px-1.5 py-0.5 text-xs text-muted-foreground">
              {index + 1}
            </span>
            {option}
          </Button>
        ))}
      </div>
    </div>
  );
}

function PermissionInteraction({
  kind,
  onSubmit
}: {
  readonly kind: Extract<InteractionKindView, { readonly kind: "permission" }>;
  readonly onSubmit: (response: InteractionResponseView) => void;
}): React.JSX.Element {
  const [reason, setReason] = React.useState("");
  const submitDecision = (decision: PermissionDecisionView): void => {
    onSubmit({ kind: "permission", action_id: kind.action_id, decision });
  };

  return (
    <div className="space-y-3">
      <div className="space-y-2">
        <div className="flex flex-wrap items-center gap-2">
          <h3 className="font-semibold">{kind.summary}</h3>
          <span className={riskBadgeClass(kind.risk)}>{kind.risk}</span>
          <span className="rounded-full bg-muted px-2 py-0.5 text-[11px] font-semibold text-muted-foreground">
            {kind.category}
          </span>
        </div>
        <p className="text-sm text-muted-foreground">Requested by {kind.actor}</p>
        {kind.reason !== undefined && kind.reason !== null ? (
          <p className="rounded-md bg-card p-2 text-sm">Reason: {kind.reason}</p>
        ) : null}
        <JsonSummary label="Subject" value={kind.subject} />
      </div>
      <label className="block space-y-1 text-sm">
        <span className="text-muted-foreground">Optional denial reason</span>
        <input
          className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm outline-none focus:ring-2 focus:ring-ring"
          onChange={(event) => setReason(event.currentTarget.value)}
          placeholder="Visible to the requesting agent"
          value={reason}
        />
      </label>
      <div className="flex flex-wrap gap-2">
        <Button size="sm" type="button" onClick={() => submitDecision({ type: "approve" })}>
          Allow
        </Button>
        <Button
          size="sm"
          type="button"
          variant="outline"
          onClick={() =>
            submitDecision({ type: "deny", reason: reason.trim().length > 0 ? reason : undefined })
          }
        >
          Deny
        </Button>
        <Button
          size="sm"
          type="button"
          variant="ghost"
          onClick={() => submitDecision({ type: "cancel" })}
        >
          Cancel
        </Button>
      </div>
    </div>
  );
}

function ResolvedInteraction({
  kind,
  response
}: {
  readonly kind: InteractionKindView;
  readonly response?: InteractionResponseView;
}): React.JSX.Element {
  return (
    <div className="space-y-2">
      <h3 className="font-semibold">Resolved {kind.kind}</h3>
      <pre className="overflow-auto rounded-md bg-muted p-3 text-xs leading-5">
        <code>
          {response === undefined ? "Response submitted" : JSON.stringify(response, null, 2)}
        </code>
      </pre>
    </div>
  );
}

function JsonSummary({
  label,
  value
}: {
  readonly label: string;
  readonly value: JsonValue;
}): React.JSX.Element {
  return (
    <div className="space-y-1">
      <span className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        {label}
      </span>
      <pre className="max-h-40 overflow-auto rounded-md bg-card p-2 text-xs leading-5">
        <code>{typeof value === "string" ? value : JSON.stringify(value, null, 2)}</code>
      </pre>
    </div>
  );
}

function riskBadgeClass(
  risk: Extract<InteractionKindView, { readonly kind: "permission" }>["risk"]
): string {
  return cn(
    "rounded-full px-2 py-0.5 text-[11px] font-semibold uppercase tracking-wide",
    risk === "low" && "bg-success/10 text-success",
    risk === "medium" && "bg-warning/15 text-warning",
    (risk === "high" || risk === "critical") && "bg-destructive/10 text-destructive"
  );
}
