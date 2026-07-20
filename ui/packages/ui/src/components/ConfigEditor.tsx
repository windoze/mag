import { ArrowLeft } from "lucide-react";
import * as React from "react";

import { cn } from "../lib/utils";

import { Button } from "./Button";

/** ConfigEditor editing mode; `graph` is a reserved placeholder. */
export type ConfigEditorMode = "text" | "graph";

/** Props for the text-form runtime config editor. */
export interface ConfigEditorProps {
  /** Current TOML document text. */
  readonly text: string;
  /** Called on every editor change. */
  readonly onChange: (text: string) => void;
  /** Called when Save is clicked (PUT the parsed config). */
  readonly onSave: () => void;
  /** Called when Reload is clicked (server re-reads its config sources). */
  readonly onReload: () => void;
  /** Called when Apply is clicked (staged config is applied to sessions). */
  readonly onApply: () => void;
  /** Initially selected mode. Defaults to `text`; `graph` is a placeholder. */
  readonly mode?: ConfigEditorMode;
  /** Disables all action buttons while a request is in flight. */
  readonly busy?: boolean;
  /** Optional error line rendered above the actions. */
  readonly error?: string;
  /** Called when the back button is clicked. */
  readonly onBack?: () => void;
  /** Additional class names. */
  readonly className?: string;
}

/**
 * Text-form editor for the runtime config (docs/WEB.md §5.5). The document is
 * edited as raw TOML; secret references (`{ env = "..." }` /
 * `{ keyring = "..." }` inline tables) stay as references and are never
 * materialized. The mode switch reserves a slot for a future graph mode.
 */
export function ConfigEditor({
  busy = false,
  className,
  error,
  mode = "text",
  onApply,
  onBack,
  onChange,
  onReload,
  onSave,
  text
}: ConfigEditorProps): React.JSX.Element {
  const [selectedMode, setSelectedMode] = React.useState<ConfigEditorMode>(mode);

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
          <h2 className="text-lg font-semibold">Config</h2>
        </div>
        <div
          aria-label="Editor mode"
          className="flex overflow-hidden rounded-lg border border-border"
          role="tablist"
        >
          <ModeTab
            active={selectedMode === "text"}
            label="Text"
            onSelect={() => setSelectedMode("text")}
          />
          <ModeTab
            active={selectedMode === "graph"}
            label="Graph"
            onSelect={() => setSelectedMode("graph")}
          />
        </div>
      </div>

      {selectedMode === "graph" ? (
        <div className="flex min-h-64 items-center justify-center rounded-xl border border-dashed border-border bg-card p-6 text-center text-sm text-muted-foreground">
          Graph mode lands in a future version. Use Text mode to edit the config.
        </div>
      ) : (
        <>
          <label className="sr-only" htmlFor="mag-config-editor">
            Config TOML
          </label>
          <textarea
            autoCapitalize="off"
            autoCorrect="off"
            className="h-96 w-full resize-y rounded-xl border border-input bg-background px-3 py-2 font-mono text-sm leading-relaxed outline-none focus:ring-2 focus:ring-ring disabled:cursor-not-allowed disabled:opacity-60"
            disabled={busy}
            id="mag-config-editor"
            spellCheck={false}
            value={text}
            onChange={(event) => onChange(event.currentTarget.value)}
          />
          <p className="text-xs text-muted-foreground">
            Secret references such as <code>{'{ env = "VAR" }'}</code> or{" "}
            <code>{'{ keyring = "NAME" }'}</code> are kept as references and never materialized.
          </p>
        </>
      )}

      {error !== undefined ? (
        <p
          className="rounded-lg border border-destructive/20 bg-destructive/10 px-3 py-2 text-sm text-destructive"
          role="alert"
        >
          {error}
        </p>
      ) : null}

      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-xs text-muted-foreground">
          Apply takes effect at each session&apos;s next turn boundary.
        </p>
        <div className="flex items-center gap-2">
          <Button disabled={busy} type="button" variant="outline" onClick={onReload}>
            Reload
          </Button>
          <Button disabled={busy} type="button" variant="outline" onClick={onApply}>
            Apply
          </Button>
          <Button disabled={busy || selectedMode !== "text"} type="button" onClick={onSave}>
            Save
          </Button>
        </div>
      </div>
    </div>
  );
}

function ModeTab({
  active,
  label,
  onSelect
}: {
  readonly active: boolean;
  readonly label: string;
  readonly onSelect: () => void;
}): React.JSX.Element {
  return (
    <button
      aria-selected={active}
      className={cn(
        "px-3 py-1.5 text-sm transition-colors",
        active ? "bg-primary text-primary-foreground" : "bg-background hover:bg-foreground/5"
      )}
      role="tab"
      type="button"
      onClick={onSelect}
    >
      {label}
    </button>
  );
}
