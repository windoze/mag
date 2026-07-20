# 实施计划：mag-acp（mag 作为 ACP server，第一个落地 interface）

> **唯一设计输入**：[`docs/ACP.md`](../../ACP.md)（mag-acp 的实现级设计）。参照 [`docs/DESIGN.md`](../../DESIGN.md)
> §5（ACP 概览）、§3.0（`MagService` trait，mag-acp 面对的接口）。
>
> service 主干（`mag-service` 接口 + `mag-core` 实现 + `mag-tools`/`mag-sources` 核心）已全部 `[DONE]`、
> `MagService` 契约已冻结，其已完成计划归档在
> [`docs/archive/2026-07-19-mag-service/`](../2026-07-19-mag-service/)。**本计划只覆盖第一个
> interface：`mag-acp` crate。** 逐任务清单见 [`TODO.md`](TODO.md)。

## 目标

落地 `docs/ACP.md` 描述的 **mag-acp**：把已冻结的 `mag-service::MagService`（`docs/DESIGN.md` §3.0）通过
**ACP（Agent Client Protocol）agent 端**暴露，被 Zed 等 ACP client 反向驱动（stdio 上的 JSON-RPC）。mag-acp 是
一层**纯协议翻译器**——把 ACP 入站请求翻译成 `MagService` 方法调用，把 `MagService` 的事件流（`subscribe`）与
审批往返翻译成 ACP 出站通知/请求，**不含任何 agent 逻辑**（那在 mag-core）。具体交付：

- **acp crate v1.2.0 的 agent 端接线**（`docs/ACP.md` §1）：用 role-marker + builder + typed-handler 模型
  （`agent_client_protocol::Agent::builder()` → `on_receive_request` / `on_receive_notification` 注册 handler →
  `connect_to(Stdio)` 跑 run loop）暴露 `initialize` / `session/new` / `session/load` / `session/prompt` /
  `session/cancel`。
- **`session/prompt` 请求 ↔ 事件流的泵**（`docs/ACP.md` §3.4）：handler 内先 `subscribe(Some(sid))` 再
  `send_message`，把 `ServiceEvent` 流泵成 `session/update` 通知，直到本轮终态返回 `PromptResponse { stop_reason }`。
- **审批桥接**（`docs/ACP.md` §5）：`ServiceEvent::InteractionRequested` ↔ ACP `session/request_permission`，
  outcome 回灌 `MagService::respond_interaction` 唤醒被暂停的 driver。复用**同一** gate，审批逻辑（mag-core）
  一行不改。
- **类型映射纯函数**（`docs/ACP.md` §4）：`ServiceEvent ↔ SessionUpdate`、`ContentBlock ↔ UserInput`、
  interaction ↔ permission、stop reason——集中在一个无 IO 的 `map` 模块，便于离线单测。
- **能力如实宣告**（`docs/ACP.md` §3.1/§7）：`initialize` 只打开 mag 真正支持的 `AgentCapabilities` 位
  （第一版文本优先；`load_session` 取决于 mag-core 恢复能力实际就绪度）。
- **充分的离线测试**（`docs/ACP.md` §9）：映射纯函数单测 + handler 级用 scripted `Arc<dyn MagService>` +
  协议级 e2e 用 acp crate 自身 client 经内存管道驱动 mag-acp（fake LLM），真实 Zed 联调 `#[ignore]`。

## 非目标（本计划）

- **不改已冻结的 `MagService` 契约**（`docs/DESIGN.md` §3.0）：mag-acp 只消费现有 trait 方法与 `ServiceEvent`
  变体；若确需新增，须回到 service 主干按向后兼容方式加（只加方法/变体/字段），并在 `TODO.md` 显式插前置任务。
- **不碰 mag-core 实现细节 / agent-lib**：mag-acp **只依赖 `mag-service` + acp crate**，面对
  `Arc<dyn MagService>`。装配 `mag-core::Engine` 注入的是上层 bin，不在 mag-acp 库内。
- **不做其它 interface**（web WebSocket、Tauri invoke/emit、React/app 前端）——那是后续 interface。
- **不做 ACP client 方向**（把外部 ACP agent 当来源消费，`docs/DESIGN.md` §6）：那是 mag-core / mag-sources
  的事（agent-lib `external-acp`），与 mag-acp（agent 方向）无关（`docs/ACP.md` §8）。
- **不实现 proxy / conductor 角色**（`docs/ACP.md` §0）；不启 `unstable_protocol_v2` feature（`docs/ACP.md` §7）。
- **不经 Command/Event**：Command/Event 是 tauri/web 的平行 wire 编码（`docs/DESIGN.md` §4）；ACP 直接映射到
  `MagService`（`docs/ACP.md` §0）。
- **多模态输入第一版不做**：`prompt_capabilities` 的 `image`/`audio`/`embedded_context` 保持 `false`，输入取
  `ContentBlock::Text`（`docs/ACP.md` §3.1/§4）。

## 现有代码锚点（实现时无需再翻库）

### acp crate（`agent-client-protocol` v1.2.0；schema `agent-client-protocol-schema` v1.4.0，经 `agent_client_protocol::schema::v1::*`）

来源见 `docs/ACP.md` §1。agent 端形状：

- **构造**：零大小角色标记 `agent_client_protocol::Agent` → `.builder()` 得 `Builder<Agent, ..>`；
  `.on_receive_request(closure, on_receive_request!())` / `.on_receive_notification(closure,
  on_receive_notification!())` 注册 handler（第二个宏参数必填，绕过 return-type-notation）；
  `.connect_to(Stdio::new()).await` 本身就是 server-only run loop（永久处理入站消息）。
- **request handler 闭包签名**（`Host = Agent`，`Counterpart = Client`）：
  `async move |req: XxxRequest, responder: Responder<XxxResponse>, cx: ConnectionTo<Client>| -> Result<_, acp::Error>`；
  用 `responder.respond(resp)` / `responder.respond_with_error(err)` 回复。
- **出站句柄 `cx: ConnectionTo<Client>`（`Clone`）**：
  - `cx.send_notification(SessionNotification::new(session_id, SessionUpdate::..))` —— 发 `session/update`（无返回）。
  - `cx.send_request(RequestPermissionRequest::new(..)).block_task().await?` —— 发 `session/request_permission`
    并 await `RequestPermissionResponse`。
- **方法 ↔ 类型**（schema v1）：`initialize`(`InitializeRequest`/`InitializeResponse`)、
  `authenticate`(`AuthenticateRequest`/`AuthenticateResponse`)、`session/new`(`NewSessionRequest`/
  `NewSessionResponse`)、`session/load`(`LoadSessionRequest`/`LoadSessionResponse`，仅当宣告 `load_session`)、
  `session/prompt`(`PromptRequest`/`PromptResponse`)、`session/cancel`(`CancelNotification`，notification →
  `on_receive_notification`)。
- **关键 schema 类型**：`InitializeResponse::new(version).agent_capabilities(caps)`；
  `AgentCapabilities { load_session, prompt_capabilities, mcp_capabilities, session_capabilities, auth }`；
  `NewSessionRequest { cwd, additional_directories, mcp_servers }` / `NewSessionResponse::new(acp_session_id)`；
  `PromptRequest { session_id, prompt: Vec<ContentBlock> }` / `PromptResponse::new(stop_reason)`；
  `StopReason::{EndTurn, Cancelled, Refusal, MaxTokens, MaxTurnRequests}`；
  `SessionUpdate::{AgentMessageChunk(ContentChunk), AgentThoughtChunk, ToolCall(ToolCall),
  ToolCallUpdate(ToolCallUpdate), Plan}`；`ContentBlock::{Text, Image, Audio, ResourceLink, Resource}`；
  `RequestPermissionRequest::new(session_id, tool_call: ToolCallUpdate, options: Vec<PermissionOption>)` /
  `RequestPermissionResponse { outcome }`；`PermissionOption { id: PermissionOptionId, name, kind:
  PermissionOptionKind }`、`PermissionOptionKind::{AllowOnce, AllowAlways, RejectOnce, RejectAlways}`；
  `RequestPermissionOutcome::{Selected { option_id }, Cancelled }`；`SessionId`(`Arc<str>`)；
  `LoadSessionRequest { session_id }`；`CancelNotification { session_id }`。

> 上述类型名/构造子/方法名以 `docs/ACP.md` §1/§3/§4/§5 为准；schema 具体字段/变体拼写在 M1-1 实现时对
> `agent_client_protocol::schema::v1` 就地核对（crate 是真实依赖）。若发现 acp crate 缺口（缺 API、类型不匹配、
> 无内存传输构造子），按「关键设计约束」的缺口处理约定在 `TODO.md` 插最小前置任务，不 papering over。

### `mag-service`（已冻结契约，来源 `crates/mag-service/src/{service.rs,lib.rs}`）

- **trait `MagService`**（object-safe，`Arc<dyn MagService>`）：
  `create_session(SessionConfig) -> SessionId`、`list_sessions() -> Vec<SessionInfo>`、
  `resume_session(SessionId)`、`delete_session(SessionId)`、`send_message(SessionId, UserInput) -> RunId`、
  `cancel(SessionId)`、`respond_interaction(SessionId, RequestId, InteractionResponseWire)`、
  `subscribe(Option<SessionId>) -> BoxStream<'static, ServiceEvent>`、`list_sources() -> Vec<SourceInfo>`、
  `probe_local_agents() -> Vec<SourceInfo>`。所有 async 方法返回 `Result<_, ServiceError>`。
- **`ServiceEvent`**（`#[non_exhaustive]`，`#[serde(tag="type", rename_all="snake_case")]`）变体：
  `SessionCreated{id,config}`、`RunStarted{id,run_id}`、`RunFinished{id,output:RunOutput}`、
  `RunError{id,message,kind:RunErrorKind}`、`TextDelta{id,text}`、`ToolStarted{id,trace:ToolTrace}`、
  `ToolFinished{id,trace:ToolTrace}`、`InteractionRequested{id,request_id:RequestId,kind:InteractionKindWire}`、
  `DelegationStarted/Finished/Failed{id,trace:DelegationTrace}`、`DelegationMessage{id,message:DelegationMessageWire}`、
  `LocalAgentsProbed{available:Vec<SourceInfo>}`。`ServiceEvent::session_id() -> Option<SessionId>`。
- **wire 类型**（`lib.rs`）：`SessionId`/`RunId`/`RequestId`（`#[serde(transparent)]` 包 `Uuid`；
  `new(Uuid)`/`parse_str(&str)`/`as_uuid()`/`Display`/`FromStr`）；`UserInput{text:String,
  attachments:Vec<MessageAttachment>}`（`UserInput::text(impl Into<String>)`）；
  `SessionConfig{provider:String, model:String, tool_profile:Option<String>, cwd:Option<PathBuf>, routing:RoutingMode, budget:Option<SessionBudget>}`；
  `SessionBudget{max_steps,max_tokens,max_cost_micros,max_wall_time_secs}`（全 `Option<u64>`，逐 run 预算）；
  `RunErrorKind::{Other,Cancelled,LoopLimitExceeded,BudgetExhausted}`（`#[serde(default)]` 分类，`Other` 为缺省）；
  `RunOutput{text,usage:Option<UsageInfo>}`；`ToolTrace{run_id,call_id:ToolCallIdWire,name,input,output,
  status:ToolStatusWire,message}`、`ToolStatusWire::{Started,Finished,Denied,Cancelled,Failed}`；
  `InteractionKindWire{Approval{call_id:ToolCallIdWire,requirement:ApprovalRequirementWire},
  Question{prompt}, Choice{prompt,options}, Permission{action_id,actor,category:PermissionCategoryWire,
  risk:PermissionRiskWire,summary,subject,reason}}`（`#[serde(tag="kind")]`）；
  `InteractionResponseWire{Approval{step_id:StepIdWire,call_id:ToolCallIdWire,decision:ApprovalDecisionWire,
  message}, Answer{text}, Choice{index}, Permission{action_id,decision:PermissionDecisionWire}}`；
  `ApprovalDecisionWire::{Approve,Deny,Timeout,Cancel}`；`PermissionDecisionWire::{Approve,Deny{reason},Cancel}`；
  `SourceInfo{id,name,kind:SourceKindWire,available,version,path,capabilities}`；`ServiceError`。

## 里程碑

逐层增加概念、每层可独立**离线**验证。顺序照 `docs/ACP.md` 的结构。

| 里程碑 | 主题 | 主要产出 | 默认测试形态 |
|---|---|---|---|
| M1 | crate 骨架 + `initialize` + `session/new` + stdio 跑通 | 新建 `mag-acp` crate（依赖 mag-service + acp crate）、`map` 纯函数（SessionId 映射 + 能力宣告）、bin 装配（builder + `connect_to`）、`initialize` / `session/new` handler、内存管道 e2e 骨架 | 纯函数单测 + handler 级 + 内存管道 e2e（`initialize`/`session/new` 往返） |
| M2 | `session/prompt` 泵 + 类型映射 | `map`：`ServiceEvent → SessionUpdate` / `ContentBlock → UserInput` / stop reason；`session/prompt` handler 的泵（`subscribe`→`send_message`→loop→`PromptResponse`） | 纯函数单测 + handler 级（scripted `MagService`，断言 update 序列 + stop reason） |
| M3 | 审批桥接 | `map`：`InteractionKindWire → RequestPermissionRequest`（tool_call + options）、outcome → `InteractionResponseWire`；`bridge_permission` 接入泵 | 纯函数单测 + handler 级（`InteractionRequested` 触发 `send_request`、outcome 回灌 `respond_interaction`） |
| M4 | cancel + `session/load` + 能力宣告收口 | `session/cancel` notification → `cancel`（泵以 `Cancelled` 收尾、挂起权限请求 cancel 收尾）；`session/load` → `resume_session`；`load_session`/`prompt` 能力宣告按实际就绪度收口 | handler 级（cancel 三路径、load 往返、能力位断言） |
| M5 | 协议级 e2e + 收官验收 | 全回合内存管道 e2e（`initialize → session/new → session/prompt`（流式+权限）`→ session/cancel`，fake LLM）；真实 Zed 联调 `#[ignore]`；收官 review | 集成（离线全链路，经 acp client ↔ mag-acp）+ review |

每个里程碑末尾有独立 review 任务 `M<n>-R`（对照 `docs/ACP.md` 对应节，确认无遗漏映射 / 未调度失败测试）。

## 关键设计约束（落地时必须守）

- **mag-acp 依赖边界**：`Cargo.toml` 只依赖 `mag-service` + `agent-client-protocol`（+ 其 schema、tokio、
  futures、serde/serde_json、async 运行时）；**不得**依赖 `mag-core` / `agent-lib`。装配 `Engine` 注入
  `Arc<dyn MagService>` 的是上层 bin（依赖 mag-core + mag-acp），mag-acp 库对 service 只见 `dyn MagService`。
- **不改已冻结契约**：只消费现有 `MagService` 方法与 `ServiceEvent` 变体。缺映射/缺字段先在 `TODO.md` 插前置
  任务回主干补，**禁止**在 mag-acp 侧臆造 ACP 语义或 papering over。
- **审批是异步暂停点**：`bridge_permission` 必须真正 `await` client 的 `RequestPermissionResponse`，再
  `respond_interaction` 唤醒 driver；决不能"发了就当批准"。测试断言 driver 在 outcome 回灌前不前进
  （`docs/ACP.md` §5）。
- **先 `subscribe` 再 `send_message`**：泵在触发运行前订阅，防竞态丢事件；`subscribe(Some(sid))` 按会话过滤，
  多个并发 prompt 各自泵各自的流（`docs/ACP.md` §3.4）。
- **能力如实宣告**：`initialize` 只打开实现确已支持的位；未实现的一律不宣告（`docs/ACP.md` §3.1/§7）。
  `load_session` 仅当 mag-core 恢复能力就绪才置 `true`。
- **shell 等特权工具审批不可省**：即便 client 可信，特权动作也经 `session/request_permission` 让用户确认，
  这是最后防线（`docs/ACP.md` §6）。
- **映射对 `#[non_exhaustive]` 保守**：只产出 mag 确有语义的 ACP 变体；未知/未支持的 mag 事件降级为一条文本
  `AgentMessageChunk` 或忽略（记日志），不臆造语义（`docs/ACP.md` §4）。
- **acp crate 缺口按前置任务处理**：遇缺 API / 类型不匹配 / 无内存传输，在 `TODO.md` 正确依赖位置插最小前置
  任务并让被阻塞任务显式依赖它，然后提交并停，不绕过。

## 测试策略（离线，`docs/ACP.md` §9）

默认序列（cheap → expensive）：

1. `cargo fmt --all -- --check`
2. 聚焦测试：任务给出精确过滤名（如 `cargo test -p mag-acp map::`）
3. `cargo clippy --all-targets -- -D warnings`
4. `cargo test --workspace`（完整套件）
5. `cargo doc --no-deps --workspace`（公开 API 带 rustdoc）

补充约束：

- **全离线**：映射纯函数直接单测；handler 级注入 fake/scripted `Arc<dyn MagService>`（脚本化 `subscribe`
  事件流 + 记录方法调用）；协议级 e2e 用 acp crate 自身 client（或 agent-lib 的 ACP client）经**内存管道**
  （如 `tokio::io::duplex`）驱动 mag-acp，service 侧注入 fake LLM。不依赖网络 / 真实凭据 / 真实 Zed / 子进程。
- **审批暂停语义必须显式测试**：脚本化断言 client 回 outcome 前 driver 不前进；approve / deny / cancel 三路径
  都覆盖。
- **每个测试 1 分钟内完成**，卡住即 bug，立即修。真实 Zed 联调一律 `#[ignore]`，缺环境干净跳过（绿）。
- **契约保守**：只对 mag 确有语义的变体断言映射；stop reason / outcome / content block 的 round-trip 与边界
  （未知/未支持变体的保守降级）都测。

## 风险与待确认

- **R-1 acp crate v1.2.0 精确 API**：类型/构造子/字段拼写以 `docs/ACP.md` §1 为准，M1-1 就地对
  `agent_client_protocol::schema::v1` 核对。若 builder / handler 宏 / `connect_to` 形状与文档有出入，按缺口
  处理约定在 `TODO.md` 修正锚点并插前置任务，不改主干。
- **R-2 内存传输**：协议级 e2e 需一个非 stdio 的内存管道把 acp client 与 mag-acp server 对接。若 acp crate 的
  `connect_to` 只接受具体传输而无现成内存构造子，M1-2 需先搭一个基于 `tokio::io::duplex` 的传输适配（属实现
  细节，非绕过）。这是 M5 全回合 e2e 的前置，M1-2 就要跑通最小 `initialize` 往返。
- **R-3 `load_session` 就绪度**：需审批会话恢复依赖 agent-lib M7-F1（已 `[DONE]`）+ mag-core C4 恢复能力。
  第一版 `load_session` 宣告策略以 mag-core 实际就绪度为准（未就绪则宣告 `false`，client 不会调用），见
  `docs/ACP.md` §3.3/§7、`docs/DESIGN.md` §3.6。
- **R-4 delegation 映射语义**：`DelegationStarted/...` 映射为 `ToolCall`/`ToolCallUpdate` 还是 `Plan` 由
  `docs/ACP.md` §4 保守约定；第一版可先降级为文本或 `ToolCall`，M2-1 明确并单测，不臆造 `Plan` 语义。
- **R-5 stop reason 精度**（**已大部消解**，2026-07-20 agent-lib 升级适配）：`RunError` 已带
  `kind: RunErrorKind`（向后兼容 `#[serde(default)]` 字段，主干侧由 `FacadeError` 变体结构化映射）。
  `Cancelled→StopReason::Cancelled`（满足 ACP 规范对 `session/cancel` 的强制要求）、
  `LoopLimitExceeded→MaxTurnRequests`、`BudgetExhausted→Refusal`（ACP 无预算专用变体）已在
  `map::run_terminal_to_stop_reason` 落地。**残留**：`MaxTokens` 仍不产出（service 侧无独立的
  token 上限信号；token 预算耗尽归 `BudgetExhausted→Refusal`）。
