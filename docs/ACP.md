# mag-acp 设计：mag 作为 ACP server

本文档展开 [`DESIGN.md`](DESIGN.md) §5，给出 **mag-acp**（mag 的第一个落地 interface）的实现级设计：把
`mag-service::MagService` 通过 **ACP（Agent Client Protocol）agent 端**暴露，被 Zed 等 ACP client 反向驱动。

- service 契约见 `DESIGN.md` §3.0（`MagService` trait）；本文只讲 ACP 适配层。
- 依赖 `agent-client-protocol` v1.2.0（下称 **acp crate**）+ 其 schema crate `agent-client-protocol-schema`
  v1.4.0（经 `agent_client_protocol::schema::v1::*` 重导出）。协议线版本以这两个 crate 为准。
- mag-acp **只依赖 `mag-service`**（面对 `Arc<dyn MagService>`）+ acp crate，**不依赖 mag-core / agent-lib**。

---

## 0. 定位与边界

- **角色**：ACP 定义 client / agent / proxy / conductor 四种角色。mag-acp 实现 **agent（server）** 角色——
  被 client 反向驱动。不实现 proxy / conductor。
- **传输**：stdio 上的 JSON-RPC。Zed 以子进程方式 spawn `mag --acp`，通过其 stdin/stdout 通信。
- **职责**：mag-acp 是一层**协议翻译器**——把 ACP 入站请求翻译成 `MagService` 方法调用，把 `MagService`
  的事件流（`subscribe`）与审批往返翻译成 ACP 出站通知/请求。**不含任何 agent 逻辑**（那在 mag-core）。
- **不经 Command/Event**：Command/Event 是 tauri/web 的 wire 编码（`DESIGN.md` §4）；ACP 是平行编码，直接
  映射到 `MagService`。

---

## 1. acp crate 的 agent 端形状（v1.2.0）

acp crate v1.2.0 **不提供** `trait Agent { async fn initialize(..); .. }` 供 impl（这与旧的 0.x API 不同）。
它用 **role-marker + builder + typed-handler** 模型：

- 取零大小角色标记 `agent_client_protocol::Agent`，调 `.builder()` 得到 `Builder<Agent, ..>`。
- 对每个 ACP 方法注册一个 **async 闭包 handler**：`on_receive_request(closure, on_receive_request!())` /
  `on_receive_notification(closure, on_receive_notification!())`（第二个参数是绕过 return-type-notation 的
  宏，必填）。
- 用 `.connect_to(Stdio::new()).await` 在 stdio 上跑起来——`connect_to` 本身就是 run loop（server-only，
  永久处理入站消息），无需手写 spawn/loop。

方法 ↔ 类型（schema v1）：

| ACP 方法 | 请求类型 | 响应类型 | handler |
|---|---|---|---|
| `initialize` | `InitializeRequest` | `InitializeResponse` | `on_receive_request` |
| `authenticate` | `AuthenticateRequest` | `AuthenticateResponse` | `on_receive_request` |
| `session/new` | `NewSessionRequest` | `NewSessionResponse` | `on_receive_request` |
| `session/load` | `LoadSessionRequest` | `LoadSessionResponse` | `on_receive_request`（仅当宣告 `load_session`）|
| `session/prompt` | `PromptRequest` | `PromptResponse` | `on_receive_request` |
| `session/cancel` | `CancelNotification` | —（notification）| `on_receive_notification` |

每个 request handler 闭包签名（`Host = Agent`，`Counterpart = Client`）：

```rust
async move |req: XxxRequest, responder: Responder<XxxResponse>, cx: ConnectionTo<Client>|
    -> Result<_, acp::Error>
```

- `responder.respond(resp)` / `respond_with_error(err)` 回复。
- **`cx: ConnectionTo<Client>`** 是 agent → client 的主动通信句柄（`Clone`）：
  - `cx.send_notification(SessionNotification::new(session_id, SessionUpdate::..))` —— 发 `session/update`（流式，无返回）。
  - `cx.send_request(RequestPermissionRequest::new(..)).block_task().await?` —— 发 `session/request_permission`
    并 await `RequestPermissionResponse`。

---

## 2. 整体结构

```
mag-acp
├── main / run(acp)         : 构造 Arc<dyn MagService>（bin 装配 mag-core::Engine 注入）、
│                             组 Agent.builder() 注册 handlers、connect_to(Stdio)
├── handlers                : initialize / session_new / session_load / session_prompt / cancel
├── pump                    : session/prompt 内把 MagService::subscribe 流泵成 session/update
├── permission bridge       : ServiceEvent::InteractionRequested ↔ session/request_permission
└── map                     : ServiceEvent ↔ SessionUpdate、SessionConfig ↔ NewSessionRequest、
                              stop reason、ContentBlock ↔ mag UserInput 的纯函数映射
```

mag-acp 持有一个 `Arc<dyn MagService>`（由 bin 在启动时用 mag-core `Engine` 装配后注入）。所有 handler 通过
它驱动 service，通过 `cx` 回吐 ACP。

---

## 3. 请求映射（ACP → `MagService`）

### 3.1 `initialize`

- 读 `InitializeRequest.protocol_version` / `client_capabilities`，回
  `InitializeResponse::new(version).agent_capabilities(caps)`。
- **能力如实宣告**（`DESIGN.md` §5.5）：`AgentCapabilities` 只打开 mag 真正支持的位：
  - `load_session`：仅当 mag 持久化确实支持恢复会话时置 `true`（见 §3.3 与 §7 的 restore 缺口约束）。
  - `prompt_capabilities`：`image`/`audio`/`embedded_context` 按 mag 实际支持的输入形态置位（第一版文本优先，
    多模态未支持则保持 `false`）。
  - `mcp_capabilities` / `session_capabilities` / `auth`：按实现覆盖如实填。
- `authenticate`：第一版 mag 本地单用户、凭据由 mag 自己的 `CredentialStore` 管（`DESIGN.md` §3.5），
  不通过 ACP 认证；`auth_methods` 留空，`authenticate` handler 返回成功空响应或不注册（视 client 要求）。

### 3.2 `session/new`

- `NewSessionRequest`：`cwd`（绝对路径）、`additional_directories`、`mcp_servers`。
- 组一个 `SessionConfig`（mag-service 类型）：cwd → 会话工作目录（映射到 agent worktree / 只读根，见
  §6 安全）；provider/model 用 mag 的默认或配置来源（ACP 不传 LLM 选择）；`mcp_servers` 第一版可忽略或作为
  后续来源接入点（对应 `DESIGN.md` §6，非本文范围）。
- 调 `service.create_session(cfg).await`，把返回的 mag `SessionId` 映射为 ACP `SessionId`（§4 ID 映射），
  回 `NewSessionResponse::new(acp_session_id)`。

### 3.3 `session/load`

- 仅当 §3.1 宣告了 `load_session`。`LoadSessionRequest` 带 ACP `SessionId`。
- 反查 mag `SessionId`，调 `service.resume_session(id).await`，回 `LoadSessionResponse`。
- **约束**：mag 的持久化恢复目前对"需审批会话"受限（依赖 agent-lib restore 注入口，见 `DESIGN.md` §3.6、
  §7）。因此 `load_session` 是否宣告为 `true`，取决于 mag-core 恢复能力的实际就绪度——未就绪则宣告 `false`，
  client 不会调用它。

### 3.4 `session/prompt`（核心：请求 vs 事件流的桥接）

这是 mag-acp 最关键的一段。**ACP 的 `session/prompt` 是一个请求**：handler 必须在其内部一直运行到本轮
turn 结束、返回 `PromptResponse { stop_reason }`。**而 mag 的一轮运行是异步事件流**（`send_message` 触发、
`subscribe` 产出流式事件、审批经 `respond_interaction` 往返）。mag-acp 在 handler 内做"泵"（pump）把两者对接：

```
on_receive_request(async move |req: PromptRequest, responder, cx| {
    let sid = map_session_id(req.session_id);
    let input = content_blocks_to_user_input(req.prompt);   // §4

    // 1. 订阅该会话的事件流（在 send_message 前订阅，避免丢事件）
    let mut events = service.subscribe(Some(sid));
    // 2. 触发一轮运行
    service.send_message(sid, input).await?;
    // 3. 泵：把 ServiceEvent 流翻成 session/update，直到本轮终态
    let stop = loop {
        match events.next().await {
            Some(ServiceEvent::TextDelta { text, .. }) =>
                cx.send_notification(SessionNotification::new(
                    req.session_id.clone(),
                    SessionUpdate::AgentMessageChunk(text.into())))?,
            Some(ServiceEvent::ToolStarted { trace, .. }) =>
                cx.send_notification(.. SessionUpdate::ToolCall(map_tool_call(trace)) ..)?,
            Some(ServiceEvent::ToolFinished { trace, .. }) =>
                cx.send_notification(.. SessionUpdate::ToolCallUpdate(..) ..)?,
            Some(ServiceEvent::InteractionRequested { request_id, kind, .. }) =>
                bridge_permission(&service, &cx, sid, request_id, kind).await?,   // §5
            Some(ServiceEvent::DelegationStarted { .. } | ..) =>
                cx.send_notification(.. map_delegation(..) ..)?,   // plan / tool_call 语义
            Some(ServiceEvent::RunFinished { .. }) => break StopReason::EndTurn,
            Some(ServiceEvent::RunError { .. })    => break StopReason::Refusal, // 或按错误分类
            None => break StopReason::EndTurn,   // 流结束
        }
    };
    responder.respond(PromptResponse::new(stop))
}, on_receive_request!())
```

要点：
- **先 `subscribe` 再 `send_message`**，防止竞态丢事件。`subscribe(Some(sid))` 按会话过滤。
- 泵**只针对本轮**：靠 `RunFinished`（本轮终态）跳出，返回 stop reason。多个并发 prompt（不同会话）各自泵
  各自的流，互不干扰（`MagService` 天然多会话）。
- **stop reason 映射**：正常结束→`EndTurn`；被 cancel→`Cancelled`（§3.5）；模型拒绝/错误→`Refusal`；
  达 loop/token 上限→`MaxTokens`/`MaxTurnRequests`（若 `ServiceEvent` 能区分，否则归 `EndTurn`）。

### 3.5 `session/cancel`（notification）

- `session/cancel` 是**通知**（无响应）。handler 用 `on_receive_notification`。
- 收到后调 `service.cancel(sid).await`，令该会话正在跑的一轮干净终止。
- 效果沿 §3.4 的泵传导：`cancel` 使 service 侧结束本轮，泵收到终态后 `session/prompt` handler 返回
  `PromptResponse { stop_reason: Cancelled }`（acp 规范要求 cancel 后必须返回 `Cancelled`）。
- 若此时有挂起的 `session/request_permission`（§5），mag-acp 需让其以 cancel outcome 收尾（见 §5）。

---

## 4. 类型映射（纯函数，可离线单测）

集中在一个 `map` 模块，全是无 IO 的纯函数，便于离线单元测试。

- **SessionId**：ACP `SessionId`（`Arc<str>`）↔ mag `SessionId`（uuid）。mag-acp 维护一张双向表
  （或直接用 mag SessionId 的字符串形式作 ACP SessionId，省一张表）。
- **UserInput**：`Vec<ContentBlock>`（`Text`/`Image`/`Audio`/`ResourceLink`/`Resource`）→ mag `UserInput`。
  第一版取 `Text` 拼接；`Image`/`Audio`/`Resource` 仅当 §3.1 宣告对应 `prompt_capabilities` 才接受，否则
  按能力协商它们不会到来。
- **ServiceEvent → SessionUpdate**：

  | `ServiceEvent` | `SessionUpdate` |
  |---|---|
  | `TextDelta` | `AgentMessageChunk(ContentChunk)` |
  | （若有 thinking 事件）| `AgentThoughtChunk` |
  | `ToolStarted` | `ToolCall(ToolCall)` |
  | `ToolFinished` | `ToolCallUpdate(ToolCallUpdate)`（终态 + 结果）|
  | `DelegationStarted/Progress/Finished/Failed` | `ToolCall`/`ToolCallUpdate` 或 `Plan`（把委派表示为工具/计划）|
  | `InteractionRequested` | 不走 update——走 `session/request_permission`（§5）|
  | `RunFinished`/`RunError` | 不发 update——决定 `session/prompt` 的 stop reason |

- **stop reason**：见 §3.4。

映射对 ACP schema 的 `#[non_exhaustive]` 枚举保持保守：只产出 mag 确有语义的变体，未知/未支持的 mag 事件
可降级为一条 `AgentMessageChunk` 文本或忽略（记日志），不臆造 ACP 语义。

---

## 5. 审批桥接（`InteractionRequested` ↔ `session/request_permission`）

mag 的审批 gate（`IpcApproval`，`DESIGN.md` §3.3）是 interface 无关的：它 emit `InteractionRequested`
（带 `request_id` + `InteractionKindWire`），并**异步暂停**等 `respond_interaction`。mag-acp 把这条往返映射到
ACP 的 `session/request_permission`：

```
async fn bridge_permission(service, cx, sid, request_id, kind) -> Result<(), Error> {
    // 1. 把 InteractionKindWire 映射成 ACP 权限请求
    let (tool_call, options) = map_interaction_to_permission(&kind);
    let req = RequestPermissionRequest::new(acp_sid, tool_call, options);
    // 2. 向 client 发请求并 await 结果（client 弹 UI、用户决定）
    let resp: RequestPermissionResponse = cx.send_request(req).block_task().await?;
    // 3. 把 ACP outcome 翻回 mag InteractionResponseWire，唤醒 service 侧暂停的 driver
    let mag_resp = map_outcome_to_response(&kind, resp.outcome);
    service.respond_interaction(sid, request_id, mag_resp).await?;
    Ok(())
}
```

映射细节：
- **options**：mag 的审批语义（approve / deny）映射为 `PermissionOption`：至少给 `AllowOnce`（approve）与
  `RejectOnce`（deny）；若 mag 支持"始终允许某工具"，加 `AllowAlways`/`RejectAlways`。每个 option 带稳定
  `PermissionOptionId`，`kind` 用 `PermissionOptionKind::{AllowOnce, AllowAlways, RejectOnce, RejectAlways}`。
- **tool_call**：`RequestPermissionRequest.tool_call: ToolCallUpdate` 用 mag 富化的审批信息填充——`InteractionKindWire`
  的 `Approval` 携 `tool_name`/`call_id`/`reason`/`input` 摘要（agent-lib M7-3，见 `DESIGN.md` §9），让 client
  能渲染有意义的权限框。
- **outcome → response**：
  - `RequestPermissionOutcome::Selected { option_id }` → 按 option_id 判定 approve/deny，组
    `InteractionResponseWire::Approval`（或 `Permission`）。
  - `RequestPermissionOutcome::Cancelled` → 映射为 deny/cancel（保守），唤醒 driver 以取消收尾。
- **`InteractionKind::Permission`**（本地 agent / 特权动作，带 category/risk/subject/summary）与工具审批
  `Approval` 走**同一** `session/request_permission` 通道，只是 tool_call/options 的填充来源不同。
- **cancel 交互**：若在 `bridge_permission` 的 `await` 期间收到 `session/cancel`（§3.5），需让挂起的权限请求
  以 cancel 收尾——或依赖 client 侧回 `Cancelled` outcome，或本地放弃并 `respond_interaction` 一个 deny，
  确保 service 侧 driver 不悬挂。

**一致性**：同一套审批往返在 Tauri 是弹窗、web 是 WS 消息、ACP 是 `session/request_permission`——因为它们都
只是 `MagService` 的 `InteractionRequested` / `respond_interaction` 的不同 wire 编码。审批逻辑（mag-core）
一行不改。

---

## 6. 安全边界

- **信任边界**：ACP 经 stdio 由父进程（spawn `mag --acp` 的 client，如 Zed）控制。mag-acp 信任其父进程。
- **工作目录**：`session/new` 的 `cwd` 决定会话工作根。mag 的工具（fs/shell）以此约束路径（`DESIGN.md`
  §7，`ToolContext.worktree`）。**shell 等特权工具仍走审批 gate**（§5）——即便 client 是可信编辑器，特权
  动作也经 `session/request_permission` 让用户确认，这是最后防线，任何 interface 都不能省。
- **凭据**：LLM 凭据由 mag 的 `CredentialStore` 管，不经 ACP 传输、不写日志。

---

## 7. 能力协商与已知约束

- **如实宣告**：`initialize` 只打开实现确已支持的 `AgentCapabilities` 位（§3.1）。未实现的特性一律不宣告，
  避免 client 调到不支持的路径。
- **`load_session` 受 restore 缺口约束**：mag 恢复"需审批会话"依赖 agent-lib 的 `AgentRestoreBuilder`
  审批注入口（agent-lib 后续任务 M7-F1，见其 `TODO.md`）。在其落地前，若 mag 只对纯对话/只读工具会话支持
  可靠恢复，则 `load_session` 宣告策略要与之匹配（要么不宣告，要么明确其恢复局限）。见 `DESIGN.md` §3.6/§11。
- **协议版本演进**：ACP 仍在演进；`InitializeResponse.protocol_version` 回传协商版本，能力宣告须向后兼容。
  acp crate 有可选 `unstable_protocol_v2` feature（`Agent::v2()`）在 v1 client 前自动降转——第一版用稳定 v1，
  不启该 feature。

---

## 8. 与"mag 作为 ACP client"的关系（双向不冲突）

mag 有 ACP 的**两个方向**，互不冲突（`DESIGN.md` §5.4）：
- **client 方向**（agent-lib `external-acp`）：mag 把**外部** ACP agent 当作一种来源消费（mag 发 prompt），
  见 `DESIGN.md` §6。这是 mag-core / mag-sources 的事，与 mag-acp 无关。
- **agent 方向**（本文 mag-acp）：mag 把**自己**暴露给 client（mag 收 prompt）。

二者可同时存在：一个 client（Zed）通过 mag-acp 驱动 mag，而 mag 内部又可把子任务委派给另一个外部 ACP
agent。当那个"外部 ACP agent"恰好是另一个 `mag --acp` 实例时，client 端与 server 端在同一协议上闭合——这是
两个方向能力齐备后自然可跑的形态，不作为单独的设计目标或里程碑。

---

## 9. 测试策略（离线）

mag-acp 的测试与 mag-core 一样坚持**离线**：

- **映射纯函数（§4）**：直接单测 `ServiceEvent ↔ SessionUpdate`、`ContentBlock ↔ UserInput`、outcome↔response、
  stop reason，round-trip 与边界（未知/未支持变体的保守降级）。
- **handler 级**：注入一个 fake/scripted `Arc<dyn MagService>`（脚本化 `subscribe` 事件流 + 记录方法调用），
  驱动 `session/prompt` handler，断言：泵出的 `session/update` 序列正确、`RunFinished` 使 handler 返回正确
  stop reason、`InteractionRequested` 触发 `send_request(session/request_permission)` 且 outcome 正确回灌
  `respond_interaction`、`session/cancel` 使本轮以 `Cancelled` 收尾。
- **协议级 e2e**：用 acp crate 自身的 client 端（或 agent-lib 的 ACP client）经内存管道驱动 mag-acp，跑
  `initialize → session/new → session/prompt（含流式 + 权限）→ session/cancel` 全回合，service 侧注入 fake
  LLM，全程离线。真实 Zed 联调为 `#[ignore]` / 手动。
