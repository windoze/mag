import * as React from "react";

import { cn } from "../lib/utils";

import { Button } from "./Button";

/** Composer mode derived from session run state. */
export type ComposerMode = "idle" | "running" | "awaiting_interaction";

/** Props for the chat composer. */
export interface ComposerProps {
  /** Current textarea value. */
  readonly value: string;
  /** Called when the textarea changes. */
  readonly onValueChange: (value: string) => void;
  /** Called with the submitted text. */
  readonly onSend: (text: string) => void;
  /** Called when the user clicks cancel during a run. */
  readonly onCancel?: () => void;
  /** Current composer mode. */
  readonly mode?: ComposerMode;
  /** Number of pending interactions in the open session. */
  readonly pendingInteractionCount?: number;
  /** Disable all controls. */
  readonly disabled?: boolean;
  /** Optional placeholder override. */
  readonly placeholder?: string;
  /** Additional class names. */
  readonly className?: string;
}

/** Renders the bottom composer with send/pivot/cancel state transitions. */
export function Composer({
  className,
  disabled = false,
  mode = "idle",
  onCancel,
  onSend,
  onValueChange,
  pendingInteractionCount = 0,
  placeholder,
  value
}: ComposerProps): React.JSX.Element {
  const trimmedValue = value.trim();
  const canSend = !disabled && trimmedValue.length > 0;
  const sendLabel = mode === "running" ? "Insert pivot..." : "Send";

  return (
    <form
      className={cn("rounded-xl border border-border bg-card p-3 shadow-sm", className)}
      onSubmit={(event) => {
        event.preventDefault();
        if (canSend) {
          onSend(value);
        }
      }}
    >
      <label className="sr-only" htmlFor="mag-composer-input">
        Message
      </label>
      <textarea
        className="min-h-24 w-full resize-y rounded-md border border-input bg-background px-3 py-2 text-sm outline-none focus:ring-2 focus:ring-ring disabled:cursor-not-allowed disabled:opacity-60"
        disabled={disabled}
        id="mag-composer-input"
        onChange={(event) => onValueChange(event.currentTarget.value)}
        onKeyDown={(event) => {
          if ((event.metaKey || event.ctrlKey) && event.key === "Enter" && canSend) {
            event.preventDefault();
            onSend(value);
          }
        }}
        placeholder={
          placeholder ??
          (mode === "running" ? "Write a pivot to insert into the running turn" : "Message mag")
        }
        value={value}
      />
      <div className="mt-3 flex flex-wrap items-center justify-between gap-3">
        <div className="text-xs text-muted-foreground">
          {mode === "running"
            ? "A run is active. Sending will try pivot first."
            : "Press Ctrl/⌘ + Enter to send."}
          {pendingInteractionCount > 0 ? (
            <span className="ml-2 rounded-full bg-warning/15 px-2 py-0.5 font-medium text-warning">
              {pendingInteractionCount} pending interaction
              {pendingInteractionCount === 1 ? "" : "s"}
            </span>
          ) : null}
        </div>
        <div className="flex items-center gap-2">
          {mode === "running" ? (
            <Button disabled={disabled} type="button" variant="outline" onClick={onCancel}>
              Cancel
            </Button>
          ) : null}
          <Button disabled={!canSend} type="submit">
            {sendLabel}
          </Button>
        </div>
      </div>
    </form>
  );
}
