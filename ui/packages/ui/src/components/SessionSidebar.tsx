import { Settings, Sparkles, Trash2 } from "lucide-react";

import { cn } from "../lib/utils";

import { Button } from "./Button";
import type { SessionStatusView, SidebarSessionView } from "./types";

/** Props for the session navigation sidebar. */
export interface SessionSidebarProps {
  /** Sessions to render, already filtered for the current user. */
  readonly sessions: readonly SidebarSessionView[];
  /** Currently open session id. */
  readonly activeSessionId?: string;
  /** Called when New chat is clicked. */
  readonly onNewSession?: () => void;
  /** Called when a session row is selected. */
  readonly onSelectSession?: (sessionId: string) => void;
  /** Called when a session delete button is clicked. */
  readonly onDeleteSession?: (sessionId: string) => void;
  /** Called when Sources is clicked. */
  readonly onOpenSources?: () => void;
  /** Called when Config is clicked. */
  readonly onOpenConfig?: () => void;
  /** Additional class names. */
  readonly className?: string;
}

/** Renders fixed navigation entries and cwd-grouped session rows. */
export function SessionSidebar({
  activeSessionId,
  className,
  onDeleteSession,
  onNewSession,
  onOpenConfig,
  onOpenSources,
  onSelectSession,
  sessions
}: SessionSidebarProps): React.JSX.Element {
  const groupedSessions = groupSessions(sessions);

  return (
    <aside
      className={cn("flex h-full min-h-0 w-72 flex-col border-r border-border bg-card", className)}
    >
      <div className="space-y-2 border-b border-border p-3">
        <Button className="w-full justify-start" type="button" onClick={onNewSession}>
          <Sparkles className="mr-2 h-4 w-4" />
          New chat
        </Button>
        <div className="grid grid-cols-2 gap-2">
          <Button size="sm" type="button" variant="outline" onClick={onOpenSources}>
            Sources
          </Button>
          <Button size="sm" type="button" variant="outline" onClick={onOpenConfig}>
            <Settings className="mr-1.5 h-3.5 w-3.5" />
            Config
          </Button>
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto p-3">
        {groupedSessions.length === 0 ? (
          <div className="rounded-lg border border-dashed border-border p-4 text-sm text-muted-foreground">
            No sessions yet.
          </div>
        ) : (
          <div className="space-y-5">
            {groupedSessions.map((group) => (
              <section className="space-y-2" key={group.name}>
                <h2 className="truncate px-1 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                  {group.name}
                </h2>
                <div className="space-y-1">
                  {group.sessions.map((session) => (
                    <SessionRow
                      active={session.id === activeSessionId}
                      key={session.id}
                      onDelete={onDeleteSession}
                      onSelect={onSelectSession}
                      session={session}
                    />
                  ))}
                </div>
              </section>
            ))}
          </div>
        )}
      </div>
    </aside>
  );
}

function SessionRow({
  active,
  onDelete,
  onSelect,
  session
}: {
  readonly active: boolean;
  readonly onDelete?: (sessionId: string) => void;
  readonly onSelect?: (sessionId: string) => void;
  readonly session: SidebarSessionView;
}): React.JSX.Element {
  return (
    <div
      className={cn(
        "group flex items-start gap-2 rounded-lg border border-transparent p-2 transition-colors",
        active ? "border-primary/30 bg-primary/10" : "hover:bg-muted"
      )}
    >
      <button
        className="min-w-0 flex-1 text-left"
        type="button"
        onClick={() => onSelect?.(session.id)}
      >
        <div className="flex items-center gap-2">
          <span className="truncate text-sm font-medium">
            {session.title ?? "Untitled session"}
          </span>
          <SessionStatusBadge status={session.status} />
        </div>
        <div className="mt-1 flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
          <span>{formatRelativeTime(session.lastActiveAt)}</span>
          {session.subtitle !== undefined ? (
            <span className="truncate">{session.subtitle}</span>
          ) : null}
        </div>
      </button>
      <button
        aria-label={`Delete ${session.title ?? session.id}`}
        className="rounded-md p-1 text-muted-foreground opacity-0 transition hover:bg-destructive/10 hover:text-destructive group-hover:opacity-100 focus:opacity-100"
        type="button"
        onClick={() => onDelete?.(session.id)}
      >
        <Trash2 className="h-4 w-4" />
      </button>
    </div>
  );
}

function SessionStatusBadge({ status }: { readonly status: SessionStatusView }): React.JSX.Element {
  return (
    <span
      className={cn(
        "shrink-0 rounded-full px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide",
        status === "idle" && "bg-muted text-muted-foreground",
        status === "running" && "bg-primary/10 text-primary",
        status === "awaiting_interaction" && "bg-warning/15 text-warning"
      )}
    >
      {status === "awaiting_interaction" ? "awaiting" : status}
    </span>
  );
}

function groupSessions(sessions: readonly SidebarSessionView[]): ReadonlyArray<{
  readonly name: string;
  readonly sessions: readonly SidebarSessionView[];
}> {
  const groups = new Map<string, SidebarSessionView[]>();
  sessions.forEach((session) => {
    const group = session.cwd ?? "No project";
    groups.set(group, [...(groups.get(group) ?? []), session]);
  });

  return [...groups.entries()].map(([name, groupSessions]) => ({
    name,
    sessions: [...groupSessions].sort(
      (left, right) => normalizeTime(right.lastActiveAt) - normalizeTime(left.lastActiveAt)
    )
  }));
}

function formatRelativeTime(timestamp: number | undefined): string {
  if (timestamp === undefined) {
    return "no activity";
  }

  const normalizedTimestamp = normalizeTime(timestamp);
  const elapsedMs = Math.max(Date.now() - normalizedTimestamp, 0);
  const minutes = Math.floor(elapsedMs / 60_000);
  if (minutes < 1) {
    return "just now";
  }
  if (minutes < 60) {
    return `${minutes}m ago`;
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    return `${hours}h ago`;
  }
  return `${Math.floor(hours / 24)}d ago`;
}

function normalizeTime(timestamp: number | undefined): number {
  if (timestamp === undefined) {
    return 0;
  }

  return timestamp < 10_000_000_000 ? timestamp * 1000 : timestamp;
}
