import {
  HttpSseTransport,
  SessionStore,
  TransportError,
  type DelegationGroupView,
  type ITransport,
  type RunView,
  type SessionStoreSnapshot,
  type SessionView,
  type ThreadItem
} from "@mag/client";
import {
  Button,
  Composer,
  DelegateThreadPanel,
  SessionSidebar,
  ThreadView,
  type ComposerMode,
  type DelegateThreadItemView,
  type DelegationItemView,
  type InteractionResponseView,
  type SidebarSessionView,
  type ThreadItemView
} from "@mag/ui";
import "@mag/ui/styles.css";
import * as React from "react";

import {
  captureFragmentToken,
  readStoredToken,
  type ShellHistory,
  type ShellLocation,
  type ShellStorage
} from "./token";

import { ConfigPage } from "./ConfigPage";
import { SourcesPage } from "./SourcesPage";

const DEFAULT_SESSION_CONFIG = {
  provider: "openai",
  model: "gpt-5-codex",
  routing: "model_routed"
} satisfies Parameters<SessionStore["createSession"]>[0];

let browserStore: SessionStore | undefined;

/** Draft-map key for the composer before any session is selected. */
const NEW_SESSION_DRAFT_KEY = "(new-session)";

type Route =
  | { readonly page: "session"; readonly sessionId?: string }
  | { readonly page: "sources" }
  | { readonly page: "config" };

/** Optional test hooks for injecting scripted transports without changing production wiring. */
export interface AppProps {
  /** Prebuilt store; mainly used by shell-level tests. */
  readonly store?: SessionStore;
  /** Transport used to create a per-App store when `store` is not supplied. */
  readonly transport?: ITransport;
  /** Storage implementation for token capture. Defaults to `sessionStorage`. */
  readonly storage?: ShellStorage;
  /** Location used for `#t=` token capture. Defaults to `window.location`. */
  readonly location?: ShellLocation;
  /** History used to remove captured token fragments. Defaults to `window.history`. */
  readonly history?: ShellHistory;
}

/** Thin web shell that wires auth, routing, SessionStore, and shared UI components. */
export function App(props: AppProps = {}): React.JSX.Element {
  const [store] = React.useState(() => createStore(props));
  const snapshot = useSessionStoreSnapshot(store);
  const [route, setRoute] = React.useState<Route>({ page: "session" });
  const [composerDrafts, setComposerDrafts] = React.useState<Readonly<Record<string, string>>>({});
  const [pendingAction, setPendingAction] = React.useState<string>();
  const [error, setError] = React.useState<string>();
  const [notice, setNotice] = React.useState<string>();
  const [selectedDelegate, setSelectedDelegate] = React.useState<string>();
  const [railCollapsed, setRailCollapsed] = React.useState(false);
  const activeSession = selectActiveSession(snapshot, route);
  const activeSessionId = activeSession?.id;
  const activeRun = isActiveRun(activeSession?.run);
  const composerMode = composerModeFor(activeSession?.run);
  const pendingInteractionCount = activeSession?.pendingInteractions.length ?? 0;
  const delegationGroups = activeSession?.delegationGroups ?? [];
  // Composer drafts are kept per session so a cross-session jump cannot send
  // text typed for one session to another.
  const draftKey = activeSessionId ?? NEW_SESSION_DRAFT_KEY;
  const composerValue = composerDrafts[draftKey] ?? "";
  const setComposerValue = (value: string): void => {
    setComposerDrafts((drafts) => ({ ...drafts, [draftKey]: value }));
  };
  const clearComposerDrafts = (...keys: readonly string[]): void => {
    setComposerDrafts((drafts) => {
      const next = { ...drafts };
      keys.forEach((key) => delete next[key]);
      return next;
    });
  };

  React.useEffect(() => {
    captureFragmentToken(props.storage, props.location, props.history);
    let cancelled = false;
    store.start();
    store.refreshSessions().catch((refreshError: unknown) => {
      if (!cancelled) {
        setError(errorMessage(refreshError));
      }
    });

    return () => {
      cancelled = true;
      store.stop();
    };
  }, [props.history, props.location, props.storage, store]);

  React.useEffect(() => {
    if (snapshot.configRevision !== undefined) {
      setNotice(
        `Config updated (r${snapshot.configRevision}); apply takes effect at each session's next turn boundary.`
      );
    }
  }, [snapshot.configRevision]);

  React.useEffect(() => {
    setSelectedDelegate(undefined);
  }, [activeSessionId]);

  React.useEffect(() => {
    if (
      selectedDelegate !== undefined &&
      !delegationGroups.some((group) => group.delegate === selectedDelegate)
    ) {
      setSelectedDelegate(undefined);
    }
  }, [delegationGroups, selectedDelegate]);

  const openDelegateGroup = (delegate: string): void => {
    setSelectedDelegate(delegate);
    setRailCollapsed(false);
  };

  const openDelegation = (delegationId: string): void => {
    const delegation = activeSession?.delegations.find(
      (candidate) => candidate.id === delegationId
    );
    if (delegation !== undefined) {
      openDelegateGroup(delegation.trace.delegate);
    }
  };

  const runAction = async (name: string, action: () => Promise<void>): Promise<void> => {
    setPendingAction(name);
    setError(undefined);
    try {
      await action();
    } catch (actionError: unknown) {
      setError(errorMessage(actionError));
    } finally {
      setPendingAction(undefined);
    }
  };

  const createAndOpenSession = async (): Promise<string> => {
    const created = await store.createSession(DEFAULT_SESSION_CONFIG);
    setRoute({ page: "session", sessionId: created.id });
    await store.openSession(created.id);
    await store.refreshSessions();
    return created.id;
  };

  const openSession = async (sessionId: string): Promise<void> => {
    setRoute({ page: "session", sessionId });
    await store.openSession(sessionId, { resume: true });
  };

  const sendComposerText = async (rawText: string): Promise<void> => {
    const text = rawText.trim();
    if (text.length === 0) {
      return;
    }

    const sessionId = activeSessionId ?? (await createAndOpenSession());
    if (activeRun) {
      try {
        await store.pivotMessage(sessionId, text);
      } catch (pivotError: unknown) {
        if (!isNotPivotableConflict(pivotError)) {
          throw pivotError;
        }
        await store.sendMessage(sessionId, text);
      }
    } else {
      await store.sendMessage(sessionId, text);
    }
    clearComposerDrafts(draftKey, sessionId);
    await store.refreshSessions();
  };

  const respondInteraction = async (
    requestId: string,
    response: InteractionResponseView
  ): Promise<void> => {
    if (activeSessionId === undefined) {
      return;
    }

    await store.respondInteraction(
      activeSessionId,
      requestId,
      response as Parameters<SessionStore["respondInteraction"]>[2]
    );
  };

  return (
    <main className="flex min-h-screen flex-col bg-background text-foreground md:flex-row">
      <SessionSidebar
        activeSessionId={activeSessionId}
        className="h-72 w-full shrink-0 border-b border-r-0 md:h-screen md:w-72 md:border-b-0 md:border-r"
        sessions={snapshot.sessions.map(toSidebarSession)}
        onDeleteSession={(sessionId) => {
          if (!confirmDelete(sessionId, snapshot.sessions)) {
            return;
          }
          void runAction("delete-session", async () => {
            await store.deleteSession(sessionId);
            if (activeSessionId === sessionId) {
              setRoute({ page: "session" });
            }
          });
        }}
        onNewSession={() =>
          void runAction("new-session", async () => void (await createAndOpenSession()))
        }
        onOpenConfig={() => setRoute({ page: "config" })}
        onOpenSources={() => setRoute({ page: "sources" })}
        onSelectSession={(sessionId) =>
          void runAction("open-session", async () => openSession(sessionId))
        }
      />

      <section className="flex min-h-[42rem] min-w-0 flex-1 flex-col md:h-screen md:min-h-0">
        <ShellHeader
          activeSession={activeSession}
          connectionStatus={snapshot.connectionStatus}
          pendingAction={pendingAction}
          railCollapsed={railCollapsed}
          route={route}
          onToggleRail={() => setRailCollapsed((collapsed) => !collapsed)}
        />
        <StatusMessage error={error} notice={notice} onDismissNotice={() => setNotice(undefined)} />
        <div className="min-h-0 flex-1 overflow-y-auto bg-muted/30">
          {route.page === "sources" ? (
            <SourcesPage
              sources={snapshot.sources}
              store={store}
              onBack={() => setRoute({ page: "session", sessionId: activeSessionId })}
            />
          ) : route.page === "config" ? (
            <ConfigPage
              store={store}
              onBack={() => setRoute({ page: "session", sessionId: activeSessionId })}
            />
          ) : (
            <ThreadView
              className="mx-auto max-w-5xl"
              emptyState={
                activeSessionId === undefined
                  ? "Create or select a session, then send a message."
                  : "This session has no committed history yet."
              }
              items={(activeSession?.thread ?? []).map(toThreadItemView)}
              onOpenDelegation={openDelegation}
              onRespondInteraction={(requestId, response) => {
                void runAction("respond-interaction", async () =>
                  respondInteraction(requestId, response)
                );
              }}
            />
          )}
        </div>
        {route.page === "session" ? (
          <div className="border-t border-border bg-background p-3">
            <Composer
              className="mx-auto max-w-5xl"
              disabled={pendingAction === "send-message" || pendingAction === "new-session"}
              mode={composerMode}
              pendingInteractionCount={pendingInteractionCount}
              value={composerValue}
              onCancel={() => {
                if (activeSessionId !== undefined) {
                  void runAction("cancel-run", async () => store.cancelRun(activeSessionId));
                }
              }}
              onSend={(text) => void runAction("send-message", async () => sendComposerText(text))}
              onValueChange={setComposerValue}
            />
          </div>
        ) : null}
      </section>

      <RightRail
        activeSession={activeSession}
        collapsed={railCollapsed}
        groups={delegationGroups}
        selectedDelegate={selectedDelegate}
        sessions={snapshot.sessions}
        onCloseDelegate={() => setSelectedDelegate(undefined)}
        onRespondInteraction={(requestId, response) => {
          void runAction("respond-interaction", async () =>
            respondInteraction(requestId, response)
          );
        }}
        onSelectDelegate={openDelegateGroup}
        onSelectSession={(sessionId) =>
          void runAction("open-session", async () => openSession(sessionId))
        }
      />
    </main>
  );
}

function createStore(props: AppProps): SessionStore {
  if (props.store !== undefined) {
    return props.store;
  }
  if (props.transport !== undefined) {
    return new SessionStore(props.transport);
  }
  if (browserStore === undefined) {
    browserStore = new SessionStore(new HttpSseTransport({ token: () => readStoredToken() }));
  }
  return browserStore;
}

function useSessionStoreSnapshot(store: SessionStore): SessionStoreSnapshot {
  const [snapshot, setSnapshot] = React.useState(() => store.getSnapshot());

  React.useEffect(() => {
    setSnapshot(store.getSnapshot());
    return store.subscribe((nextSnapshot) => setSnapshot(nextSnapshot));
  }, [store]);

  return snapshot;
}

function selectActiveSession(
  snapshot: SessionStoreSnapshot,
  route: Route
): SessionView | undefined {
  if (route.page !== "session" || route.sessionId === undefined) {
    return undefined;
  }
  return snapshot.sessions.find((session) => session.id === route.sessionId);
}

function toSidebarSession(session: SessionView): SidebarSessionView {
  const config = session.info?.config ?? session.config;
  return {
    id: session.id,
    title: session.info?.title ?? firstUserMessage(session)?.text ?? session.id,
    cwd: config?.cwd,
    lastActiveAt: session.info?.last_active_at,
    status: session.status,
    subtitle: config === undefined ? undefined : `${config.provider} / ${config.model}`
  };
}

function firstUserMessage(session: SessionView): { readonly text: string } | undefined {
  return session.messages.find((message) => message.role === "user");
}

function toThreadItemView(item: ThreadItem): ThreadItemView {
  switch (item.type) {
    case "message":
      return { type: "message", message: item.message };
    case "tool_call":
      return { type: "tool_call", toolCall: item.toolCall };
    case "delegation":
      return {
        type: "delegation",
        delegation: {
          id: item.delegation.id,
          delegate: item.delegation.trace.delegate,
          status: item.delegation.trace.status,
          task: item.delegation.trace.task,
          output: item.delegation.trace.output,
          message: item.delegation.trace.message,
          usage: item.delegation.trace.usage
        }
      };
    case "interaction":
      return {
        type: "interaction",
        interaction: {
          requestId: item.interaction.requestId,
          kind: item.interaction.kind,
          origin: item.interaction.origin,
          status: item.interaction.status,
          response: item.interaction.response
        }
      };
    case "pivot":
      return { type: "pivot", notice: item.notice };
    case "run_error":
      return { type: "run_error", error: item.error };
  }
}

function isActiveRun(run: RunView | undefined): boolean {
  return run?.state === "running" || run?.state === "awaiting_interaction";
}

function composerModeFor(run: RunView | undefined): ComposerMode {
  return isActiveRun(run) ? "running" : "idle";
}

/**
 * True only for the pivot fallback condition of docs/WEB.md §5.4: HTTP 409
 * with kind `not_pivotable`. Every other failure must surface to the user
 * instead of silently falling back to `send_message`.
 */
function isNotPivotableConflict(error: unknown): boolean {
  if (error instanceof TransportError) {
    return error.status === 409 && error.kind === "not_pivotable";
  }
  return (
    typeof error === "object" && error !== null && "kind" in error && error.kind === "not_pivotable"
  );
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function confirmDelete(sessionId: string, sessions: readonly SessionView[]): boolean {
  const session = sessions.find((candidate) => candidate.id === sessionId);
  const title = session?.info?.title ?? sessionId;
  if (typeof window === "undefined" || typeof window.confirm !== "function") {
    return true;
  }
  return window.confirm(`Delete session "${title}"? This cannot be undone.`);
}

function ShellHeader({
  activeSession,
  connectionStatus,
  onToggleRail,
  pendingAction,
  railCollapsed,
  route
}: {
  readonly activeSession?: SessionView;
  readonly connectionStatus: SessionStoreSnapshot["connectionStatus"];
  readonly onToggleRail: () => void;
  readonly pendingAction?: string;
  readonly railCollapsed: boolean;
  readonly route: Route;
}): React.JSX.Element {
  const title =
    route.page === "sources"
      ? "Sources"
      : route.page === "config"
        ? "Config"
        : (activeSession?.info?.title ?? "Conversation");

  return (
    <header className="flex flex-wrap items-center justify-between gap-3 border-b border-border bg-card px-4 py-3">
      <div className="min-w-0">
        <p className="text-xs font-semibold uppercase tracking-wide text-primary">mag web</p>
        <h1 className="truncate text-lg font-semibold">{title}</h1>
      </div>
      <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
        {pendingAction !== undefined ? <span>{pendingAction.replaceAll("-", " ")}...</span> : null}
        <span className="rounded-full border border-border bg-background px-2 py-1">
          {connectionStatus}
        </span>
        <Button
          aria-label={railCollapsed ? "Expand right rail" : "Collapse right rail"}
          size="sm"
          type="button"
          variant="ghost"
          onClick={onToggleRail}
        >
          {railCollapsed ? "⟨ rail" : "rail ⟩"}
        </Button>
      </div>
    </header>
  );
}

function StatusMessage({
  error,
  notice,
  onDismissNotice
}: {
  readonly error?: string;
  readonly notice?: string;
  readonly onDismissNotice: () => void;
}): React.JSX.Element | null {
  if (error === undefined && notice === undefined) {
    return null;
  }

  return (
    <div className="space-y-2 border-b border-border bg-background px-4 py-3">
      {error !== undefined ? (
        <div className="rounded-lg border border-destructive/20 bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {error}
        </div>
      ) : null}
      {notice !== undefined ? (
        <div className="flex items-center justify-between gap-3 rounded-lg border border-primary/20 bg-primary/10 px-3 py-2 text-sm text-primary">
          <span>{notice}</span>
          <Button size="sm" type="button" variant="ghost" onClick={onDismissNotice}>
            Dismiss
          </Button>
        </div>
      ) : null}
    </div>
  );
}

function RightRail({
  activeSession,
  collapsed,
  groups,
  onCloseDelegate,
  onRespondInteraction,
  onSelectDelegate,
  onSelectSession,
  selectedDelegate,
  sessions
}: {
  readonly activeSession?: SessionView;
  readonly collapsed: boolean;
  readonly groups: readonly DelegationGroupView[];
  readonly onCloseDelegate: () => void;
  readonly onRespondInteraction: (requestId: string, response: InteractionResponseView) => void;
  readonly onSelectDelegate: (delegate: string) => void;
  readonly onSelectSession: (sessionId: string) => void;
  readonly selectedDelegate?: string;
  readonly sessions: readonly SessionView[];
}): React.JSX.Element {
  const runningSessions = sessions.filter((session) => isActiveRun(session.run));
  const selectedGroup = groups.find((group) => group.delegate === selectedDelegate);

  return (
    <aside
      aria-label="Session progress rail"
      className={
        collapsed
          ? "hidden w-72 shrink-0 flex-col gap-4 overflow-y-auto border-l border-border bg-card p-4"
          : "hidden w-72 shrink-0 flex-col gap-4 overflow-y-auto border-l border-border bg-card p-4 xl:flex"
      }
    >
      <section>
        <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          Current run
        </h2>
        <p className="mt-2 rounded-lg bg-muted p-3 text-sm">
          {activeSession === undefined ? "No session selected." : runSummary(activeSession.run)}
        </p>
      </section>
      <section>
        <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          Delegates
        </h2>
        <div className="mt-2 space-y-2">
          {groups.length === 0 ? (
            <p className="rounded-lg border border-dashed border-border p-3 text-sm text-muted-foreground">
              No delegates in this session.
            </p>
          ) : (
            groups.map((group) => {
              const latest = latestDelegationTrace(group);
              const pendingCount = group.items.filter(
                (item) => item.type === "interaction" && item.interaction.status === "pending"
              ).length;
              return (
                <button
                  className={`w-full rounded-lg border p-3 text-left text-sm transition hover:border-primary/40 ${
                    group.delegate === selectedDelegate
                      ? "border-primary/60 bg-primary/5"
                      : "border-border"
                  }`}
                  key={group.id}
                  type="button"
                  onClick={() => onSelectDelegate(group.delegate)}
                >
                  <span className="flex items-center justify-between gap-2">
                    <span className="truncate font-medium">{group.delegate}</span>
                    {latest !== undefined ? (
                      <span className="text-[11px] text-muted-foreground">{latest.status}</span>
                    ) : null}
                  </span>
                  <span className="mt-1 block text-xs text-muted-foreground">
                    {groupSummary(group, pendingCount)}
                  </span>
                </button>
              );
            })
          )}
        </div>
      </section>
      {selectedGroup !== undefined ? (
        <DelegateThreadPanel
          delegate={selectedGroup.delegate}
          depth={selectedGroup.depth}
          items={selectedGroup.items.map(toDelegateThreadItem)}
          status={latestDelegationTrace(selectedGroup)?.status}
          usage={latestUsage(selectedGroup)}
          onClose={onCloseDelegate}
          onRespondInteraction={onRespondInteraction}
        />
      ) : null}
      <section>
        <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          Running sessions
        </h2>
        <div className="mt-2 space-y-2">
          {runningSessions.length === 0 ? (
            <p className="rounded-lg border border-dashed border-border p-3 text-sm text-muted-foreground">
              Nothing running.
            </p>
          ) : (
            runningSessions.map((session) => (
              <button
                className={`w-full rounded-lg border p-3 text-left text-sm transition hover:border-primary/40 ${
                  session.id === activeSession?.id
                    ? "border-primary/60 bg-primary/5"
                    : "border-border"
                }`}
                key={session.id}
                type="button"
                onClick={() => onSelectSession(session.id)}
              >
                <span className="block truncate font-medium">
                  {session.info?.title ?? session.id}
                </span>
                <span className="text-xs text-muted-foreground">{runSummary(session.run)}</span>
              </button>
            ))
          )}
        </div>
      </section>
    </aside>
  );
}

function toDelegateThreadItem(item: DelegationGroupView["items"][number]): DelegateThreadItemView {
  if (item.type === "message") {
    return { type: "message", message: { id: item.id, text: item.text } };
  }
  return {
    type: "interaction",
    interaction: {
      requestId: item.interaction.requestId,
      kind: item.interaction.kind,
      origin: item.interaction.origin,
      status: item.interaction.status,
      response: item.interaction.response
    }
  };
}

/**
 * Most recently updated lifecycle trace in the group. Traces are upserted in
 * place, so creation order (`.at(-1)`) would miss an older run that finished
 * later; `updatedSeq` tracks the last update instead.
 */
function latestDelegationTrace(
  group: DelegationGroupView
): DelegationGroupView["delegations"][number]["trace"] | undefined {
  return group.delegations.reduce<DelegationGroupView["delegations"][number] | undefined>(
    (latest, delegation) =>
      latest === undefined || delegation.updatedSeq >= latest.updatedSeq ? delegation : latest,
    undefined
  )?.trace;
}

function latestUsage(group: DelegationGroupView): DelegationItemView["usage"] {
  const withUsage = group.delegations.filter((delegation) => delegation.trace.usage !== undefined);
  return withUsage.reduce<DelegationGroupView["delegations"][number] | undefined>(
    (latest, delegation) =>
      latest === undefined || delegation.updatedSeq >= latest.updatedSeq ? delegation : latest,
    undefined
  )?.trace.usage;
}

function groupSummary(group: DelegationGroupView, pendingCount: number): string {
  const parts = [`${group.items.length} item${group.items.length === 1 ? "" : "s"}`];
  if (group.depth !== undefined) {
    parts.push(`depth ${group.depth}`);
  }
  if (pendingCount > 0) {
    parts.push(`${pendingCount} pending`);
  }
  return parts.join(" · ");
}

function runSummary(run: RunView): string {
  switch (run.state) {
    case "idle":
      return "Idle";
    case "running":
      return run.runId === undefined ? "Running" : `Running ${run.runId}`;
    case "awaiting_interaction":
      return run.runId === undefined ? "Awaiting interaction" : `Awaiting interaction ${run.runId}`;
    case "error":
      return `${run.kind}: ${run.message}`;
  }
}
