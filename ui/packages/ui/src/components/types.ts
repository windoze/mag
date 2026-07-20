/** JSON value shape accepted by mag protocol-like UI props. */
export type JsonValue =
  null | boolean | number | string | readonly JsonValue[] | { readonly [key: string]: JsonValue };

/** Optional attribution shown for delegated work. */
export interface OriginAttribution {
  /** Delegate name; absent means the root session produced the item. */
  readonly delegate?: string;
  /** Delegation depth where `0` is the root session. */
  readonly depth: number;
}

/** Minimal attachment metadata rendered under user messages. */
export interface MessageAttachmentView {
  /** User-facing attachment name. */
  readonly name?: string;
  /** MIME type when known. */
  readonly mime_type?: string;
  /** Path or URI visible to the engine. */
  readonly uri?: string;
  /** Inline text preview for small attachments. */
  readonly text?: string;
}

/** Message item consumed by `ThreadView`. */
export interface ThreadMessageView {
  /** Stable item identity. */
  readonly id: string;
  /** Message author. */
  readonly role: "assistant" | "user";
  /** Markdown-capable message text. */
  readonly text: string;
  /** True while assistant deltas are still streaming. */
  readonly streaming?: boolean;
  /** Optional user attachments. */
  readonly attachments?: readonly MessageAttachmentView[];
  /** Optional delegated origin badge. */
  readonly origin?: OriginAttribution;
}

/** Lifecycle states displayed by tool cards. */
export type ToolStatusView = "started" | "finished" | "denied" | "cancelled" | "failed";

/** Tool call trace consumed by `ToolCallCard`. */
export interface ToolTraceView {
  /** Framework-level tool call identity. */
  readonly call_id: string;
  /** Tool name selected by the model. */
  readonly name: string;
  /** Optional input payload. */
  readonly input?: JsonValue;
  /** Optional output payload. */
  readonly output?: JsonValue;
  /** Current lifecycle status. */
  readonly status: ToolStatusView;
  /** Optional status or failure message. */
  readonly message?: string;
}

/** Tool-card view model consumed by `ThreadView`. */
export interface ToolCallItemView {
  /** Stable item identity. */
  readonly id: string;
  /** Latest tool trace. */
  readonly trace: ToolTraceView;
  /** Optional delegated origin badge. */
  readonly origin?: OriginAttribution;
}

/** Delegation lifecycle states displayed in thread cards. */
export type DelegationStatusView = "started" | "finished" | "failed";

/** Inline delegation item consumed by `ThreadView`. */
export interface DelegationItemView {
  /** Stable item identity. */
  readonly id: string;
  /** Delegate name or source key. */
  readonly delegate: string;
  /** Current delegate lifecycle state. */
  readonly status: DelegationStatusView;
  /** Optional task summary. */
  readonly task?: string;
  /** Optional terminal output. */
  readonly output?: string;
  /** Optional status or failure message. */
  readonly message?: string;
  /** Optional token usage summary. */
  readonly usage?: {
    readonly input_tokens?: number;
    readonly output_tokens?: number;
    readonly total_tokens?: number;
  };
}

/** Approval policy summary shown in approval interaction cards. */
export type ApprovalRequirementView =
  | { readonly type: "auto_approve" }
  | { readonly type: "require_approval"; readonly reason?: string | null };

/** Privileged-action category for permission cards. */
export type PermissionCategoryView =
  "shell" | "file_read" | "file_write" | "network" | "spawn_agent" | "mcp" | "other";

/** Privileged-action risk for permission cards. */
export type PermissionRiskView = "low" | "medium" | "high" | "critical";

/** Interaction request variants rendered by `InteractionCard`. */
export type InteractionKindView =
  | {
      readonly kind: "approval";
      readonly call_id: string;
      readonly requirement: ApprovalRequirementView;
      readonly tool_name?: string;
      readonly input?: JsonValue;
    }
  | { readonly kind: "question"; readonly prompt: string }
  | { readonly kind: "choice"; readonly prompt: string; readonly options: readonly string[] }
  | {
      readonly kind: "permission";
      readonly action_id: string;
      readonly actor: string;
      readonly category: PermissionCategoryView;
      readonly risk: PermissionRiskView;
      readonly summary: string;
      readonly subject: JsonValue;
      readonly reason?: string | null;
    };

/** Approval decision payload shape compatible with the wire protocol. */
export type ApprovalDecisionView = "approve" | "deny" | "timeout" | "cancel";

/** Permission decision payload shape compatible with the wire protocol. */
export type PermissionDecisionView =
  | { readonly type: "approve" }
  | { readonly type: "deny"; readonly reason?: string | null }
  | { readonly type: "cancel" };

/** Interaction response payload emitted by `InteractionCard`. */
export type InteractionResponseView =
  | {
      readonly kind: "approval";
      readonly step_id: string;
      readonly call_id: string;
      readonly decision: ApprovalDecisionView;
      readonly message?: string | null;
    }
  | { readonly kind: "answer"; readonly text: string }
  | { readonly kind: "choice"; readonly index: number }
  | {
      readonly kind: "permission";
      readonly action_id: string;
      readonly decision: PermissionDecisionView;
    };

/** Interaction item consumed by `ThreadView`. */
export interface InteractionItemView {
  /** Stable request identity. */
  readonly requestId: string;
  /** Interaction payload to render. */
  readonly kind: InteractionKindView;
  /** Origin badge supplied by the service. */
  readonly origin?: OriginAttribution;
  /** UI lifecycle. */
  readonly status?: "pending" | "responded";
  /** Submitted response when known. */
  readonly response?: InteractionResponseView;
}

/** Pivot lifecycle notice rendered as a system row. */
export interface PivotNoticeView {
  /** Stable item identity. */
  readonly id: string;
  /** Pivot lifecycle state. */
  readonly status: "queued" | "applied" | "dropped";
  /** Optional drop reason. */
  readonly reason?: string;
}

/** Run error kind used by system notices. */
export type RunErrorKindView = "other" | "cancelled" | "loop_limit_exceeded" | "budget_exhausted";

/** Terminal run error rendered as a system row. */
export interface RunErrorNoticeView {
  /** Stable item identity. */
  readonly id: string;
  /** Human-readable message. */
  readonly message: string;
  /** Machine-readable failure kind. */
  readonly kind: RunErrorKindView;
}

/** Ordered thread item union consumed by `ThreadView`. */
export type ThreadItemView =
  | { readonly type: "message"; readonly message: ThreadMessageView }
  | { readonly type: "tool_call"; readonly toolCall: ToolCallItemView }
  | { readonly type: "delegation"; readonly delegation: DelegationItemView }
  | { readonly type: "interaction"; readonly interaction: InteractionItemView }
  | { readonly type: "pivot"; readonly notice: PivotNoticeView }
  | { readonly type: "run_error"; readonly error: RunErrorNoticeView };

/** Sidebar session status shown as a badge. */
export type SessionStatusView = "idle" | "running" | "awaiting_interaction";

/** Session row consumed by `SessionSidebar`. */
export interface SidebarSessionView {
  /** Stable session identity. */
  readonly id: string;
  /** User-facing title. */
  readonly title?: string;
  /** Project/cwd grouping key. */
  readonly cwd?: string;
  /** Last activity timestamp in milliseconds or seconds. */
  readonly lastActiveAt?: number;
  /** Current session status. */
  readonly status: SessionStatusView;
  /** Optional provider/model subtitle. */
  readonly subtitle?: string;
}

/** Source row consumed by `SourcesView`. */
export interface SourceView {
  /** Stable source key used by configuration. */
  readonly id: string;
  /** Human-readable source name. */
  readonly name: string;
  /** Source family label (e.g. `llm_provider`, `local_agent`). */
  readonly kind: string;
  /** Whether the source is currently usable. */
  readonly available: boolean;
  /** Optional version string reported by the source. */
  readonly version?: string;
  /** Capability labels reported by the source. */
  readonly capabilities: readonly string[];
}
