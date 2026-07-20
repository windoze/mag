import type {
  Command,
  ConfigDto,
  DelegationMessageWire,
  DelegationStatusWire,
  DelegationTrace,
  Event,
  HistoryEntry,
  InteractionKindWire,
  InteractionOrigin,
  InteractionResponseWire,
  MessageAttachment,
  RequestId,
  RunErrorKind,
  RunId,
  SessionConfig,
  SessionId,
  SessionInfo,
  SessionStatusWire,
  SourceInfo,
  ToolStatusWire,
  ToolTrace
} from "@mag/protocol";

import type { ITransport } from "./transport";

import { configDtoToToml, tomlToConfigDto } from "./config";

/** Connection state of the live event stream. */
export type ConnectionStatus = "idle" | "connecting" | "connected" | "disconnected";

/** Provenance of a view-model item. */
export type ItemSource = "history" | "event" | "local";

/** Assistant/user message rendered by thread views. */
export interface ConversationMessage {
  /** Stable view-model identity. */
  readonly id: string;
  /** Message author. */
  readonly role: "assistant" | "user";
  /** User-visible text. */
  text: string;
  /** Optional attachments supplied with user messages. */
  readonly attachments?: readonly MessageAttachment[];
  /** True while assistant deltas are still arriving. */
  streaming: boolean;
  /** Source that produced this message. */
  readonly source: ItemSource;
}

/** Tool-call card state keyed by `ToolTrace.call_id`. */
export interface ToolCallView {
  /** Stable view-model identity, equal to the tool call id. */
  readonly id: string;
  /** Latest trace snapshot for this call. */
  trace: ToolTrace;
  /** Source that first produced this card. */
  readonly source: ItemSource;
}

/** A delegated-agent message wrapped with its arrival order in the session stream. */
export interface StoredDelegationMessage {
  /** Monotonic arrival sequence shared with interactions for stable interleaving. */
  readonly seq: number;
  /** Wire payload emitted by the delegated agent. */
  readonly message: DelegationMessageWire;
}

/** Delegated-agent card state keyed by run/delegate identity. */
export interface DelegationView {
  /** Stable view-model identity. */
  readonly id: string;
  /** Latest delegation lifecycle snapshot. */
  trace: DelegationTrace;
  /** Monotonic marker of the last trace update, for "latest status" derivation. */
  updatedSeq: number;
  /** Messages emitted by the delegated agent, in arrival order. */
  readonly messages: StoredDelegationMessage[];
  /** Source that first produced this card. */
  readonly source: ItemSource;
}

/** Pending or submitted interaction request. */
export interface InteractionView {
  /** Stable request identity emitted by the service. */
  readonly requestId: RequestId;
  /** Interaction payload to render. */
  readonly kind: InteractionKindWire;
  /** Origin attribution inherited from the service event. */
  readonly origin: InteractionOrigin;
  /** Monotonic arrival sequence shared with delegation messages. */
  readonly seq: number;
  /** UI lifecycle for the request. */
  status: "pending" | "responded";
  /** Response submitted through the store, when known. */
  response?: InteractionResponseWire;
}

/** Lightweight pivot system notice. */
export interface PivotNotice {
  /** Stable view-model identity. */
  readonly id: string;
  /** Pivot lifecycle event. */
  readonly status: "queued" | "applied" | "dropped";
  /** Optional drop reason. */
  readonly reason?: string;
}

/** Terminal run error notice kept in the thread. */
export interface RunErrorNotice {
  /** Stable view-model identity. */
  readonly id: string;
  /** Human-readable failure message. */
  readonly message: string;
  /** Machine-readable run error kind. */
  readonly kind: RunErrorKind;
}

/** Thread item union consumed by UI containers. */
export type ThreadItem =
  | { readonly type: "message"; readonly message: ConversationMessage }
  | { readonly type: "tool_call"; readonly toolCall: ToolCallView }
  | { readonly type: "delegation"; readonly delegation: DelegationView }
  | { readonly type: "interaction"; readonly interaction: InteractionView }
  | { readonly type: "pivot"; readonly notice: PivotNotice }
  | { readonly type: "run_error"; readonly error: RunErrorNotice };

/** One sub-thread item inside a delegation group, in arrival order. */
export type DelegationGroupItem =
  | {
      readonly type: "message";
      /** Stable item identity derived from the delegation key and arrival sequence. */
      readonly id: string;
      /** Message text emitted by the delegated agent. */
      readonly text: string;
    }
  | { readonly type: "interaction"; readonly interaction: InteractionView };

/**
 * Drill-down view for one delegate: every delegation lifecycle card for that
 * delegate name plus the activity attributed to it (decision D5). Wire tool
 * events carry no origin, so the sub-thread aggregates delegation messages and
 * origin-attributed interactions; tool-card badges render whenever an origin
 * is available.
 */
export interface DelegationGroupView {
  /** Stable group identity, equal to the delegate name. */
  readonly id: string;
  /** Delegate name shared by the group. */
  readonly delegate: string;
  /** Delegation lifecycle cards for this delegate, in arrival order. */
  readonly delegations: readonly DelegationView[];
  /** Deepest origin depth observed on attributed interactions, when any exist. */
  readonly depth?: number;
  /** Arrival-ordered sub-thread items attributed to this delegate. */
  readonly items: readonly DelegationGroupItem[];
}

/** Current run state for a session. */
export type RunView =
  | { readonly state: "idle" }
  | { readonly state: "running"; readonly runId?: RunId }
  | { readonly state: "awaiting_interaction"; readonly runId?: RunId }
  | { readonly state: "error"; readonly message: string; readonly kind: RunErrorKind };

/** Complete transport-neutral view model for one session. */
export interface SessionView {
  /** Stable session identity. */
  readonly id: SessionId;
  /** Last known session configuration. */
  readonly config?: SessionConfig;
  /** Last metadata row returned by `list_sessions`, when loaded. */
  readonly info?: SessionInfo;
  /** Derived status used by sidebars. */
  readonly status: SessionStatusWire;
  /** User and assistant messages. */
  readonly messages: readonly ConversationMessage[];
  /** Tool cards keyed by tool call id. */
  readonly toolCalls: readonly ToolCallView[];
  /** Delegation cards keyed by delegate/run identity. */
  readonly delegations: readonly DelegationView[];
  /** Delegation drill-down groups derived for the right rail. */
  readonly delegationGroups: readonly DelegationGroupView[];
  /** Pending interaction queue in arrival order. */
  readonly pendingInteractions: readonly InteractionView[];
  /** Full ordered thread projection. */
  readonly thread: readonly ThreadItem[];
  /** Current run state. */
  readonly run: RunView;
  /** Pivot lifecycle notices. */
  readonly pivotNotices: readonly PivotNotice[];
}

/** Whole-store immutable snapshot delivered to subscribers. */
export interface SessionStoreSnapshot {
  /** Live event-stream connection status. */
  readonly connectionStatus: ConnectionStatus;
  /** Sessions in the latest list order, followed by locally discovered sessions. */
  readonly sessions: readonly SessionView[];
  /** Last probed source list. */
  readonly sources: readonly SourceInfo[];
  /** Latest config revision announced by `config_changed`. */
  readonly configRevision?: number;
  /** Last stream or alignment error observed by the store. */
  readonly lastError?: unknown;
}

/** Listener invoked when the store snapshot changes. */
export type SessionStoreListener = (snapshot: SessionStoreSnapshot) => void;

/** Options for SessionStore behavior. */
export interface SessionStoreOptions {
  /** Delay before reconnect alignment starts after a stream failure. */
  readonly reconnectDelayMs?: number;
  /** Start the event subscription from the constructor. Defaults to false. */
  readonly autoStart?: boolean;
}

/** Response shape returned by `POST /api/sessions`. */
export interface CreateSessionResult {
  /** Created session identity. */
  readonly id: SessionId;
  /** Stored session configuration. */
  readonly config: SessionConfig;
}

/** Response shape returned by `POST /api/sessions/{id}/messages`. */
export interface SendMessageResult {
  /** Started run identity. */
  readonly run_id: RunId;
}

interface MutableSession {
  id: SessionId;
  config?: SessionConfig;
  info?: SessionInfo;
  status: SessionStatusWire;
  messages: ConversationMessage[];
  toolCalls: ToolCallView[];
  delegations: DelegationView[];
  interactions: InteractionView[];
  pendingInteractions: InteractionView[];
  thread: ThreadItem[];
  run: RunView;
  pivotNotices: PivotNotice[];
  toolCallsById: Map<string, ToolCallView>;
  delegationsById: Map<string, DelegationView>;
  interactionsById: Map<RequestId, InteractionView>;
  activeAssistant?: ConversationMessage;
}

/** Bookkeeping for an in-flight history refresh; see `SessionStore.historyRefreshes`. */
interface HistoryRefresh {
  /** Number of overlapping refreshes for the same session. */
  depth: number;
  /** Session events received while the refresh was in flight. */
  buffer: Event[];
}

/** Transport-neutral session state authority used by web and future desktop shells. */
export class SessionStore {
  private readonly transport: ITransport;
  private readonly reconnectDelayMs: number;
  private readonly sessions = new Map<SessionId, MutableSession>();
  private readonly openSessionIds = new Set<SessionId>();
  private readonly listeners = new Set<SessionStoreListener>();
  private sessionOrder: SessionId[] = [];
  private sources: SourceInfo[] = [];
  private connectionStatus: ConnectionStatus = "idle";
  private configRevision: number | undefined;
  private lastError: unknown;
  private unsubscribeEvents: (() => void) | undefined;
  private reconnectHandle: ReturnType<typeof setTimeout> | undefined;
  private started = false;
  private sequence = 0;
  private itemSeq = 0;
  /**
   * In-flight history refreshes keyed by session. While a refresh is awaiting
   * the server snapshot, live events for that session are buffered and
   * replayed after the replace so the replace cannot wipe events that were
   * emitted after the snapshot was taken.
   */
  private readonly historyRefreshes = new Map<SessionId, HistoryRefresh>();

  /** Creates a store over the provided transport. */
  constructor(transport: ITransport, options: SessionStoreOptions = {}) {
    this.transport = transport;
    this.reconnectDelayMs = options.reconnectDelayMs ?? 500;

    if (options.autoStart === true) {
      this.start();
    }
  }

  /** Starts consuming the live event stream. */
  start(): void {
    if (this.started) {
      return;
    }

    this.started = true;
    this.connect();
  }

  /** Stops the live event stream and cancels pending reconnect attempts. */
  stop(): void {
    this.started = false;
    this.unsubscribeEvents?.();
    this.unsubscribeEvents = undefined;
    if (this.reconnectHandle !== undefined) {
      clearTimeout(this.reconnectHandle);
      this.reconnectHandle = undefined;
    }
    this.connectionStatus = "idle";
    this.notify();
  }

  /** Subscribes to store changes and returns an unsubscribe callback. */
  subscribe(listener: SessionStoreListener): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  /** Returns a snapshot of the current state. */
  getSnapshot(): SessionStoreSnapshot {
    return {
      connectionStatus: this.connectionStatus,
      sessions: this.selectSessions(),
      sources: [...this.sources],
      configRevision: this.configRevision,
      lastError: this.lastError
    };
  }

  /** Returns all sessions in sidebar order. */
  selectSessions(): readonly SessionView[] {
    return this.sessionOrder
      .map((id) => this.mutableSessionToView(this.sessions.get(id)))
      .filter(isDefined);
  }

  /** Returns one session view, if known. */
  selectSession(id: SessionId): SessionView | undefined {
    return this.mutableSessionToView(this.sessions.get(id));
  }

  /** Returns the ordered thread projection for one session. */
  selectThread(id: SessionId): readonly ThreadItem[] {
    const thread = this.sessions.get(id)?.thread;
    return thread === undefined ? [] : [...thread];
  }

  /** Returns pending interactions for one session in arrival order. */
  selectPendingInteractions(id: SessionId): readonly InteractionView[] {
    const pending = this.sessions.get(id)?.pendingInteractions;
    return pending === undefined ? [] : [...pending];
  }

  /**
   * Groups one session's delegation activity per delegate name for the
   * drill-down rail: lifecycle cards plus arrival-ordered messages and
   * origin-attributed interactions. Delegates that only ever surfaced through
   * an interaction origin (no `delegation_*` trace on the wire) still get a
   * group with an empty `delegations` list.
   */
  selectDelegationGroups(id: SessionId): readonly DelegationGroupView[] {
    const session = this.sessions.get(id);
    return session === undefined ? [] : this.buildDelegationGroups(session);
  }

  private buildDelegationGroups(session: MutableSession): DelegationGroupView[] {
    interface GroupDraft {
      readonly delegate: string;
      readonly delegations: DelegationView[];
      depth?: number;
      readonly entries: { readonly seq: number; readonly item: DelegationGroupItem }[];
    }

    const drafts = new Map<string, GroupDraft>();
    const draftFor = (delegate: string): GroupDraft => {
      const existing = drafts.get(delegate);
      if (existing !== undefined) {
        return existing;
      }
      const draft: GroupDraft = { delegate, delegations: [], entries: [] };
      drafts.set(delegate, draft);
      return draft;
    };

    session.delegations.forEach((delegation) => {
      const draft = draftFor(delegation.trace.delegate);
      draft.delegations.push(delegation);
      delegation.messages.forEach((stored) =>
        draft.entries.push({
          seq: stored.seq,
          item: {
            type: "message",
            id: `${delegation.id}:msg:${stored.seq}`,
            text: stored.message.text
          }
        })
      );
    });
    session.interactions.forEach((interaction) => {
      const delegate = interaction.origin.delegate;
      if (delegate === undefined) {
        return;
      }
      const draft = draftFor(delegate);
      draft.entries.push({ seq: interaction.seq, item: { type: "interaction", interaction } });
      draft.depth = Math.max(draft.depth ?? 0, interaction.origin.depth);
    });

    return [...drafts.values()].map((draft) => ({
      id: draft.delegate,
      delegate: draft.delegate,
      delegations: [...draft.delegations],
      depth: draft.depth,
      items: draft.entries.sort((left, right) => left.seq - right.seq).map((entry) => entry.item)
    }));
  }

  /** Sends `list_sessions` and syncs sidebar metadata. */
  async refreshSessions(): Promise<readonly SessionInfo[]> {
    const sessions = (await this.transport.send({ type: "list_sessions" })) as SessionInfo[];
    this.syncSessionInfos(sessions);
    this.notify();
    return sessions;
  }

  /** Fetches the runtime config and serializes it as TOML text for the ConfigEditor. */
  async getConfigText(): Promise<string> {
    const dto = (await this.transport.send({ type: "get_config" })) as ConfigDto;
    return configDtoToToml(dto);
  }

  /** Parses ConfigEditor TOML text and sends it as the new runtime config. */
  async saveConfigText(text: string): Promise<void> {
    const config = tomlToConfigDto(text);
    await this.transport.send({ type: "update_config", config });
  }

  /** Asks the server to reload the runtime config from its sources. */
  async reloadConfig(): Promise<void> {
    await this.transport.send({ type: "reload_config" });
  }

  /** Asks the server to apply the staged config to running sessions. */
  async applyConfig(): Promise<void> {
    await this.transport.send({ type: "apply_config" });
  }

  /** Sends `list_sources` and replaces the store's source list. */
  async refreshSources(): Promise<readonly SourceInfo[]> {
    const sources = (await this.transport.send({ type: "list_sources" })) as SourceInfo[];
    this.sources = [...sources];
    this.notify();
    return sources;
  }

  /**
   * Sends `probe_local_agents` and merges the fresh result into the store's
   * source list. Probing only covers local agents — provider rows reported by
   * `list_sources` are kept. The server also broadcasts `local_agents_probed`;
   * merging here is idempotent with that event.
   */
  async probeSources(): Promise<readonly SourceInfo[]> {
    const probed = (await this.transport.send({ type: "probe_local_agents" })) as SourceInfo[];
    this.mergeProbedSources(probed);
    this.notify();
    return probed;
  }

  /** Replaces local-agent rows with the probed list, keeping all other source kinds. */
  private mergeProbedSources(probed: readonly SourceInfo[]): void {
    const others = this.sources.filter((source) => source.kind !== "local_agent");
    this.sources = [...others, ...probed];
  }

  /** Marks a session as open and replaces its thread with authoritative history. */
  async openSession(
    id: SessionId,
    options: { readonly resume?: boolean } = {}
  ): Promise<readonly HistoryEntry[]> {
    this.openSessionIds.add(id);
    if (options.resume === true) {
      await this.transport.send({ type: "resume_session", id });
    }
    return this.refreshHistory(id);
  }

  /** Fetches and installs authoritative history for one session. */
  async refreshHistory(id: SessionId): Promise<readonly HistoryEntry[]> {
    const refresh = this.beginHistoryRefresh(id);
    try {
      const history = (await this.transport.send({
        type: "get_session_history",
        id
      })) as HistoryEntry[];
      this.replaceHistory(id, history);
      this.notify();
      return history;
    } finally {
      this.endHistoryRefresh(id, refresh);
    }
  }

  /** Marks a history refresh as in flight so live events are buffered instead of applied. */
  private beginHistoryRefresh(id: SessionId): HistoryRefresh {
    const existing = this.historyRefreshes.get(id);
    if (existing !== undefined) {
      existing.depth += 1;
      return existing;
    }

    const refresh: HistoryRefresh = { depth: 1, buffer: [] };
    this.historyRefreshes.set(id, refresh);
    return refresh;
  }

  /**
   * Closes a history refresh; the last close replays buffered events in arrival
   * order. Replay is dedup-safe: tool/delegation upserts are status-ranked,
   * interactions dedup by request id, final assistant text dedups against
   * history, and pivot notices dedup against the previous notice.
   */
  private endHistoryRefresh(id: SessionId, refresh: HistoryRefresh): void {
    refresh.depth -= 1;
    if (refresh.depth > 0) {
      return;
    }

    this.historyRefreshes.delete(id);
    const buffered = refresh.buffer.splice(0);
    buffered.forEach((event) => this.applyEvent(event));
  }

  /**
   * Replaces one session's committed history while preserving still-pending
   * interactions and streamed delegation messages. History entries carry
   * delegation lifecycle traces but no `delegation_message` payloads (the wire
   * history format has no such entry), so messages received via the live event
   * stream are re-attached to the rebuilt cards; without this, every resume or
   * reconnect alignment would empty the delegate sub-threads.
   */
  replaceHistory(id: SessionId, history: readonly HistoryEntry[]): void {
    const session = this.ensureSession(id);
    const pendingInteractions = session.interactions.filter(
      (interaction) => interaction.status === "pending"
    );
    const streamedMessages = new Map<string, DelegationView>();

    session.delegations.forEach((delegation) => {
      if (delegation.messages.length > 0) {
        streamedMessages.set(delegation.id, delegation);
      }
    });

    session.messages = [];
    session.toolCalls = [];
    session.delegations = [];
    session.interactions = [];
    session.pendingInteractions = [];
    session.thread = [];
    session.pivotNotices = [];
    session.toolCallsById = new Map();
    session.delegationsById = new Map();
    session.interactionsById = new Map();
    session.activeAssistant = undefined;

    history.forEach((entry, index) => this.applyHistoryEntry(session, entry, index));
    streamedMessages.forEach((previous, key) => {
      const rebuilt = session.delegationsById.get(key);
      if (rebuilt !== undefined) {
        rebuilt.messages.push(...previous.messages);
        return;
      }
      // The trace never reached history (e.g. the delegation was still
      // in-flight when the snapshot was taken): keep a synthetic card so the
      // streamed messages stay reachable.
      const delegation = {
        id: key,
        trace: previous.trace,
        updatedSeq: previous.updatedSeq,
        messages: [...previous.messages],
        source: previous.source
      } satisfies DelegationView;
      session.delegations.push(delegation);
      session.delegationsById.set(key, delegation);
      session.thread.push({ type: "delegation", delegation });
    });
    pendingInteractions.forEach((interaction) => this.installInteraction(session, interaction));
  }

  /** Sends a create-session command and syncs the returned session row. */
  async createSession(config: SessionConfig): Promise<CreateSessionResult> {
    const result = (await this.transport.send({
      type: "create_session",
      config
    })) as CreateSessionResult;
    const session = this.ensureSession(result.id, result.config);
    session.config = result.config;
    this.notify();
    return result;
  }

  /** Deletes a session and removes all local view state after the server accepts it. */
  async deleteSession(id: SessionId): Promise<void> {
    await this.transport.send({ type: "delete_session", id });
    this.openSessionIds.delete(id);
    this.sessions.delete(id);
    this.sessionOrder = this.sessionOrder.filter((sessionId) => sessionId !== id);
    this.notify();
  }

  /** Sends a user message and appends the local user bubble after the server accepts it. */
  async sendMessage(
    id: SessionId,
    text: string,
    attachments?: readonly MessageAttachment[]
  ): Promise<SendMessageResult> {
    const command: Command = {
      type: "send_message",
      session_id: id,
      text,
      attachments: attachments === undefined ? undefined : [...attachments]
    };
    const result = (await this.transport.send(command)) as SendMessageResult;
    const session = this.ensureSession(id);
    this.addMessage(session, {
      id: this.nextId("local-user"),
      role: "user",
      text,
      attachments,
      streaming: false,
      source: "local"
    });
    this.notify();
    return result;
  }

  /**
   * Sends a pivot message, preserving typed `not_pivotable` failures for
   * callers. On success the pivot text is appended as a local user bubble so
   * the typed line stays visible like in the CLI (decision D6); the server
   * commits applied pivots into history as user messages, so a later history
   * replace converges onto the authoritative entry.
   */
  async pivotMessage(id: SessionId, text: string): Promise<void> {
    await this.transport.send({ type: "pivot_message", session_id: id, text });
    const session = this.ensureSession(id);
    this.addMessage(session, {
      id: this.nextId("local-pivot"),
      role: "user",
      text,
      streaming: false,
      source: "local"
    });
    this.notify();
  }

  /** Sends a run cancellation command. */
  async cancelRun(id: SessionId): Promise<void> {
    await this.transport.send({ type: "cancel_run", session_id: id });
  }

  /** Sends an interaction response and marks the local card as submitted. */
  async respondInteraction(
    id: SessionId,
    requestId: RequestId,
    response: InteractionResponseWire
  ): Promise<void> {
    await this.transport.send({
      type: "respond_interaction",
      session_id: id,
      request_id: requestId,
      response
    });
    const session = this.ensureSession(id);
    const interaction = session.interactionsById.get(requestId);
    if (interaction !== undefined) {
      interaction.status = "responded";
      interaction.response = response;
      session.pendingInteractions = session.pendingInteractions.filter(
        (pending) => pending.requestId !== requestId
      );
      if (
        session.pendingInteractions.length === 0 &&
        session.run.state === "awaiting_interaction"
      ) {
        session.run =
          session.run.runId === undefined
            ? { state: "idle" }
            : { state: "running", runId: session.run.runId };
        session.status = session.run.state === "running" ? "running" : "idle";
      }
      this.notify();
    }
  }

  /** Runs reconnect alignment: refresh session list and histories for open sessions. */
  async align(): Promise<void> {
    await this.refreshSessions();
    await Promise.all([...this.openSessionIds].map((id) => this.refreshHistory(id)));
  }

  /** Applies one live event to the store. Exposed for scripted tests and desktop transports. */
  applyEvent(event: Event): void {
    const sessionId = eventSessionId(event);
    if (sessionId !== undefined) {
      const refresh = this.historyRefreshes.get(sessionId);
      if (refresh !== undefined) {
        refresh.buffer.push(event);
        return;
      }
    }

    switch (event.type) {
      case "session_created":
        this.applySessionCreated(event.id, event.config);
        break;
      case "run_started":
        this.applyRunStarted(event.id, event.run_id);
        break;
      case "run_finished":
        this.applyRunFinished(event.id, event.output.text);
        break;
      case "run_error":
        this.applyRunError(event.id, event.message, event.kind);
        break;
      case "text_delta":
        this.applyTextDelta(event.id, event.text);
        break;
      case "tool_started":
      case "tool_finished":
        this.upsertToolTrace(event.id, event.trace, "event");
        break;
      case "interaction_requested":
        this.applyInteractionRequested(event.id, event.request_id, event.kind, event.origin);
        break;
      case "delegation_started":
      case "delegation_finished":
      case "delegation_failed":
        this.upsertDelegationTrace(event.id, event.trace, "event");
        break;
      case "delegation_message":
        this.applyDelegationMessage(event.id, event.message);
        break;
      case "local_agents_probed":
        this.mergeProbedSources(event.available);
        break;
      case "pivot_queued":
        this.addPivotNotice(event.id, "queued");
        break;
      case "pivot_applied":
        this.addPivotNotice(event.id, "applied");
        break;
      case "pivot_dropped":
        this.addPivotNotice(event.id, "dropped", event.reason);
        break;
      case "config_changed":
        this.configRevision = event.revision;
        break;
    }

    this.notify();
  }

  private connect(): void {
    this.unsubscribeEvents?.();
    this.connectionStatus = "connecting";
    this.notify();
    this.unsubscribeEvents = this.transport.subscribe((event) => this.applyEvent(event), {
      onError: (error) => this.handleStreamError(error)
    });
    this.connectionStatus = "connected";
    this.notify();
  }

  private handleStreamError(error: unknown): void {
    if (!this.started) {
      return;
    }

    this.lastError = error;
    this.connectionStatus = "disconnected";
    this.notify();
    this.scheduleReconnect();
  }

  private scheduleReconnect(): void {
    if (this.reconnectHandle !== undefined) {
      return;
    }

    this.reconnectHandle = setTimeout(() => {
      this.reconnectHandle = undefined;
      void this.reconnect();
    }, this.reconnectDelayMs);
  }

  private async reconnect(): Promise<void> {
    if (!this.started) {
      return;
    }

    try {
      await this.align();
      this.connect();
    } catch (error: unknown) {
      this.lastError = error;
      this.connectionStatus = "disconnected";
      this.notify();
      this.scheduleReconnect();
    }
  }

  private syncSessionInfos(infos: readonly SessionInfo[]): void {
    const listedIds = infos.map((info) => info.id);
    this.sessionOrder = [
      ...listedIds,
      ...this.sessionOrder.filter((id) => !listedIds.includes(id))
    ];
    infos.forEach((info) => {
      const session = this.ensureSession(info.id, info.config);
      session.info = info;
      session.config = info.config;
      session.status = info.status;
      if (info.status === "running" && session.run.state === "idle") {
        session.run = { state: "running" };
      } else if (
        info.status === "awaiting_interaction" &&
        session.run.state !== "awaiting_interaction"
      ) {
        session.run =
          session.run.state === "running"
            ? { state: "awaiting_interaction", runId: session.run.runId }
            : { state: "awaiting_interaction" };
      } else if (
        info.status === "idle" &&
        (session.run.state === "running" || session.run.state === "awaiting_interaction")
      ) {
        // The server is authoritative: a locally active run whose session the
        // server reports as idle has ended (e.g. the interaction was answered
        // from another tab, or a terminal event was missed during a
        // disconnect). Reset so the composer does not stay stuck in running
        // mode; reconnect alignment cannot fix this because history entries
        // carry no run state.
        session.run = { state: "idle" };
      }
    });
  }

  private applyHistoryEntry(session: MutableSession, entry: HistoryEntry, index: number): void {
    switch (entry.type) {
      case "user_message":
        this.addMessage(session, {
          id: `history:${session.id}:${index}:user`,
          role: "user",
          text: entry.text,
          attachments: entry.attachments,
          streaming: false,
          source: "history"
        });
        break;
      case "assistant_message":
        this.addMessage(session, {
          id: `history:${session.id}:${index}:assistant`,
          role: "assistant",
          text: entry.text,
          streaming: false,
          source: "history"
        });
        break;
      case "tool_call":
        this.upsertToolTrace(session.id, entry.trace, "history");
        break;
      case "delegation":
        this.upsertDelegationTrace(session.id, entry.trace, "history");
        break;
    }
  }

  private applySessionCreated(id: SessionId, config: SessionConfig): void {
    const session = this.ensureSession(id, config);
    session.config = config;
  }

  private applyRunStarted(id: SessionId, runId: RunId): void {
    const session = this.ensureSession(id);
    session.run = { state: "running", runId };
    session.status = "running";
    session.activeAssistant = undefined;
  }

  private applyRunFinished(id: SessionId, outputText: string): void {
    const session = this.ensureSession(id);
    if (session.activeAssistant !== undefined) {
      session.activeAssistant.streaming = false;
      session.activeAssistant = undefined;
    } else if (outputText.length > 0 && !this.lastAssistantTextEquals(session, outputText)) {
      this.addMessage(session, {
        id: this.nextId("assistant-final"),
        role: "assistant",
        text: outputText,
        streaming: false,
        source: "event"
      });
    }

    session.run = { state: "idle" };
    session.status = "idle";
  }

  private applyRunError(id: SessionId, message: string, kind: RunErrorKind): void {
    const session = this.ensureSession(id);
    if (session.activeAssistant !== undefined) {
      session.activeAssistant.streaming = false;
      session.activeAssistant = undefined;
    }
    const error = { id: this.nextId("run-error"), message, kind } satisfies RunErrorNotice;
    session.thread.push({ type: "run_error", error });
    session.run = { state: "error", message, kind };
    session.status = "idle";
  }

  private applyTextDelta(id: SessionId, text: string): void {
    const session = this.ensureSession(id);
    if (session.activeAssistant === undefined) {
      const message = {
        id: this.nextId("assistant-stream"),
        role: "assistant",
        text: "",
        streaming: true,
        source: "event"
      } satisfies ConversationMessage;
      this.addMessage(session, message);
      session.activeAssistant = message;
    }
    session.activeAssistant.text += text;
  }

  private upsertToolTrace(id: SessionId, trace: ToolTrace, source: ItemSource): void {
    const session = this.ensureSession(id);
    const existing = session.toolCallsById.get(trace.call_id);
    if (existing !== undefined) {
      if (toolStatusRank(trace.status) >= toolStatusRank(existing.trace.status)) {
        existing.trace = trace;
      }
      return;
    }

    const toolCall = { id: trace.call_id, trace, source } satisfies ToolCallView;
    session.toolCalls.push(toolCall);
    session.toolCallsById.set(toolCall.id, toolCall);
    session.thread.push({ type: "tool_call", toolCall });
  }

  private applyInteractionRequested(
    id: SessionId,
    requestId: RequestId,
    kind: InteractionKindWire,
    origin: InteractionOrigin
  ): void {
    const session = this.ensureSession(id);
    if (session.interactionsById.has(requestId)) {
      return;
    }

    const interaction = {
      requestId,
      kind,
      origin,
      seq: this.nextSeq(),
      status: "pending"
    } satisfies InteractionView;
    this.installInteraction(session, interaction);
    session.run =
      session.run.state === "running" || session.run.state === "awaiting_interaction"
        ? { state: "awaiting_interaction", runId: session.run.runId }
        : { state: "awaiting_interaction" };
    session.status = "awaiting_interaction";
  }

  private installInteraction(session: MutableSession, interaction: InteractionView): void {
    session.interactions.push(interaction);
    session.interactionsById.set(interaction.requestId, interaction);
    if (interaction.status === "pending") {
      session.pendingInteractions.push(interaction);
    }
    session.thread.push({ type: "interaction", interaction });
  }

  private upsertDelegationTrace(id: SessionId, trace: DelegationTrace, source: ItemSource): void {
    const session = this.ensureSession(id);
    const key = delegationKey(trace.run_id, trace.delegate);
    const existing = session.delegationsById.get(key);
    if (existing !== undefined) {
      if (delegationStatusRank(trace.status) >= delegationStatusRank(existing.trace.status)) {
        existing.trace = trace;
        existing.updatedSeq = this.nextSeq();
      }
      return;
    }

    const delegation = {
      id: key,
      trace,
      updatedSeq: this.nextSeq(),
      messages: [],
      source
    } satisfies DelegationView;
    session.delegations.push(delegation);
    session.delegationsById.set(key, delegation);
    session.thread.push({ type: "delegation", delegation });
  }

  private applyDelegationMessage(id: SessionId, message: DelegationMessageWire): void {
    const session = this.ensureSession(id);
    const key = delegationKey(message.run_id, message.delegate);
    const stored = { seq: this.nextSeq(), message } satisfies StoredDelegationMessage;
    const existing = session.delegationsById.get(key);
    if (existing !== undefined) {
      existing.messages.push(stored);
      return;
    }

    const delegation = {
      id: key,
      trace: { run_id: message.run_id, delegate: message.delegate, status: "started" },
      updatedSeq: stored.seq,
      messages: [stored],
      source: "event"
    } satisfies DelegationView;
    session.delegations.push(delegation);
    session.delegationsById.set(key, delegation);
    session.thread.push({ type: "delegation", delegation });
  }

  private addPivotNotice(id: SessionId, status: PivotNotice["status"], reason?: string): void {
    const session = this.ensureSession(id);
    const lastNotice = session.pivotNotices.at(-1);
    if (lastNotice?.status === status && lastNotice.reason === reason) {
      return;
    }

    const notice = { id: this.nextId(`pivot-${status}`), status, reason } satisfies PivotNotice;
    session.pivotNotices.push(notice);
    session.thread.push({ type: "pivot", notice });
  }

  private addMessage(session: MutableSession, message: ConversationMessage): void {
    session.messages.push(message);
    session.thread.push({ type: "message", message });
  }

  private ensureSession(id: SessionId, config?: SessionConfig): MutableSession {
    const existing = this.sessions.get(id);
    if (existing !== undefined) {
      if (config !== undefined) {
        existing.config = config;
      }
      return existing;
    }

    const session: MutableSession = {
      id,
      config,
      status: "idle",
      messages: [],
      toolCalls: [],
      delegations: [],
      interactions: [],
      pendingInteractions: [],
      thread: [],
      run: { state: "idle" },
      pivotNotices: [],
      toolCallsById: new Map(),
      delegationsById: new Map(),
      interactionsById: new Map()
    };
    this.sessions.set(id, session);
    if (!this.sessionOrder.includes(id)) {
      this.sessionOrder.push(id);
    }
    return session;
  }

  private mutableSessionToView(session: MutableSession | undefined): SessionView | undefined {
    if (session === undefined) {
      return undefined;
    }

    return {
      id: session.id,
      config: session.config,
      info: session.info,
      status: session.status,
      messages: [...session.messages],
      toolCalls: [...session.toolCalls],
      delegations: [...session.delegations],
      delegationGroups: this.buildDelegationGroups(session),
      pendingInteractions: [...session.pendingInteractions],
      thread: [...session.thread],
      run: session.run,
      pivotNotices: [...session.pivotNotices]
    };
  }

  private lastAssistantTextEquals(session: MutableSession, text: string): boolean {
    const lastAssistant = [...session.messages]
      .reverse()
      .find((message) => message.role === "assistant");
    return lastAssistant?.text === text;
  }

  private nextId(prefix: string): string {
    this.sequence += 1;
    return `${prefix}:${this.sequence}`;
  }

  private nextSeq(): number {
    this.itemSeq += 1;
    return this.itemSeq;
  }

  private notify(): void {
    const snapshot = this.getSnapshot();
    this.listeners.forEach((listener) => listener(snapshot));
  }
}

function delegationKey(runId: RunId | undefined, delegate: string): string {
  return `${runId ?? "global"}:${delegate}`;
}

/** Returns the session an event belongs to, or undefined for store-global events. */
function eventSessionId(event: Event): SessionId | undefined {
  return "id" in event ? event.id : undefined;
}

function toolStatusRank(status: ToolStatusWire): number {
  return status === "started" ? 0 : 1;
}

function delegationStatusRank(status: DelegationStatusWire): number {
  return status === "started" ? 0 : 1;
}

function isDefined<T>(value: T | undefined): value is T {
  return value !== undefined;
}
