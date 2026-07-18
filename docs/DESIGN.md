# mag 设计文档

`mag` 是一个基于 [`agent-lib`](../../agent-lib) 的增强版编码 agent 应用。它把 agent-lib 的 sans-io
agent 引擎包装成一个**面向人的产品**：GUI（Tauri）与 web 提供一致的用户体验，多种 AI 能力来源
（LLM API + 本地 agent 程序）统一接入，并支持 multi-agent 混合调度。此外 mag 自身可作为
**ACP（Agent Client Protocol）server** 被 Zed 等编辑器反向驱动。

本文档描述 mag 的架构、模块划分、传输协议、以及关键设计取舍。agent-lib 的内部设计见其
[`DESIGN.md`](../../agent-lib/DESIGN.md)；mag 接入 agent-lib 的具体 API 约束见 §9。

---

## 0. 目标与非目标

### 目标

- **GUI + web 一致体验**：以 Tauri 为基础，webview 与浏览器 web 共用同一套前端代码；web 也是
  **单用户**，只是"本机能力的远程访问接口"，不做多用户 / 账户体系。
- **多种 AI 能力来源**：LLM API（Anthropic / OpenAI）与本地 agent 程序（Claude Code / Codex /
  OpenCode / 任意 ACP agent）统一为"来源（source）"，可混用。
- **multi-agent**：一个 supervisor agent 可把子任务委派给本地 LLM subagent 或本地 CLI agent，
  混合调度。
- **mag 作为 ACP server**：mag 把自己实现的 agent（含工具、审批、multi-agent）通过 ACP 暴露，
  被 Zed 等 ACP client 当作一个 coding agent 使用（见 §5）。
- **可增量扩展的工具集**：工具是插件式 registry，第一版给最小可用集，逐步加。
- **会话持久化**：会话可关闭后恢复、历史可查（第一版即做）。

### 非目标（第一版）

- 不做多用户 / 鉴权 / 云托管；web 绑定本机 loopback。
- 不自研 LLM wire 适配、会话不变量、effect 模型——这些是 agent-lib 的职责，mag 只装配。
- 不在第一版实现 AI-based permission / AI-based routing 的**决策逻辑**；但架构须为它们预留
  接缝（见 §8）。
- 不实现 ACP proxy / conductor 角色；mag 只做 ACP agent（server）与 ACP client（消费外部 agent）
  两端。

---

## 1. 全局架构

mag 的核心洞察是：**三种前端（interface）背后是同一个 service，而这个 service 的"外观"是一个 Rust
抽象接口 `MagService`，不是某种 JSON 协议**。

```
        ┌─────────────┐   ┌─────────────┐   ┌──────────────────┐
        │ Tauri GUI   │   │  Web 浏览器  │   │  ACP client(Zed) │
        └──────┬──────┘   └──────┬──────┘   └────────┬─────────┘
               │ invoke/emit     │ WebSocket         │ stdio JSON-RPC
        ┌──────┴──────┐   ┌──────┴──────┐   ┌────────┴─────────┐
        │  mag-tauri  │   │  mag-server │   │     mag-acp      │  interface adapter
        │ (Cmd/Event) │   │ (Cmd/Event) │   │  (ACP JSON-RPC)  │
        └──────┬──────┘   └──────┬──────┘   └────────┬─────────┘
               └─────────────────┼───────────────────┘
                                 │  依赖并调用
        ┌────────────────────────┴────────────────────────────┐
        │              mag-service（抽象接口 crate）             │
        │  trait MagService + service protocol（中立 serde 类型）│
        │  create/list/resume/delete_session · send_message ·   │
        │  subscribe -> Stream<ServiceEvent> ·                  │
        │  respond_interaction · cancel · probe_sources · ...   │
        └────────────────────────┬────────────────────────────┘
                                 │  实现（impl MagService for Engine）
        ┌────────────────────────┴────────────────────────────┐
        │                    mag-core :: Engine                 │
        │   SessionManager · per-session driver actor · 审批 gate│
        │   事件总线 · 凭据存储 · 持久化 · source/tool registry   │
        └────────────────────────┬────────────────────────────┘
                                 │  装配 + 驱动
        ┌────────────────────────┴────────────────────────────┐
        │                       agent-lib                       │
        │  Client · Conversation · Agent(machine/effect) · facade│
        └───────────────────────────────────────────────────────┘
```

**统一点是 `mag-service` 这层 Rust 抽象接口，而非某种通用 JSON。** 每个 interface adapter 只依赖
`mag-service`（trait + 中立类型），把自己的传输帧翻译成对 `MagService` 方法的调用、把 `ServiceEvent`
翻译回自己的传输帧：

- **Tauri / web** 共用一种 wire 编码（`Command`/`Event`，见 §4）——它是 `mag-service` 的 service protocol
  内容（`ServiceEvent` 的 serde 投影），不是独立的"通用协议层"。
- **ACP** 是**另一种平行的** wire 编码（ACP JSON-RPC，见 §5），同样映射到 `MagService`，不经 Command/Event。

`mag-core::Engine` 是 `MagService` 的唯一实现，持有 agent 驱动 / 审批 / 工具 / 持久化 / multi-agent 的全部
核心逻辑；bin 装配后作为 `dyn MagService` 注入各 interface。三种前端因此共享 100% 的核心逻辑，且彼此的
wire 形状互不牵制。

> **为什么 service 是接口而非协议**：ACP 有自己完整的 wire 规范，若强求三前端共用一份 Command/Event，
> Command/Event 会被 ACP 的形状带偏。把统一点上移到 Rust trait 后，Command/Event 退化为 tauri/web 这一种
> 编码，ACP 是平行的另一种，两者只各自映射到 `MagService`。`mag-service` 只定义**抽象**（不含实现、不依赖
> agent-lib），上层直接依赖它消费——不引入额外的"可替换"间接层。

> **ACP 的双重身份**：mag 既是 ACP **client**（通过 agent-lib 的 `external-acp` 消费外部 ACP agent，作为
> 一种"来源"，§6），也是 ACP **agent/server**（通过 `mag-acp` 把自己暴露给 Zed，§5）。二者方向相反、互不
> 冲突：前者是 §6 的一种 source，后者是第一个落地的 interface。

---

## 2. Crate / 目录结构

mag 是一个 Cargo workspace。核心是把**抽象接口**（mag-service）与**实现**（mag-core）分成两个 crate，
interface adapter 只依赖前者。

```
mag/
├── Cargo.toml                  # [workspace]
├── crates/
│   ├── mag-service/            # 抽象接口：trait MagService + service protocol（中立类型）。不依赖 agent-lib
│   ├── mag-core/               # 实现：Engine impl MagService — session/driver/审批/持久化/registry
│   ├── mag-tools/              # 插件式工具 registry + 内置工具（fs/shell/...）
│   ├── mag-sources/            # AI 来源：LLM provider 配置 + 本地 agent 接入 + 凭据
│   ├── mag-acp/                # interface #1：ACP agent/server（被 Zed 等反向驱动）
│   ├── mag-server/             # interface：web（axum + WebSocket）
│   └── mag-tauri/              # interface：Tauri（invoke/emit 桥接）
├── app/                        # React + TS 前端（Vite），喂 Tauri webview 和浏览器
│   ├── src/
│   │   ├── transport/          # ITransport：TauriTransport | WebSocketTransport
│   │   ├── protocol/           # 从 mag-service 的 Command/Event 生成的 TS 类型
│   │   ├── session/            # 会话状态、事件 reducer
│   │   └── components/
│   └── ...
├── docs/                       # DESIGN.md · ACP.md（设计文档）
├── PLAN.md                     # 当前实施计划
└── TODO.md                     # 逐任务清单
```

| crate | 职责 | 依赖 |
|---|---|---|
| **mag-service** | **抽象接口**：`trait MagService`（async 方法）+ service protocol 中立类型（`ServiceEvent`、`Command`/`Event`、`SessionConfig`、交互/工具/权限 wire 类型），全 `Serialize/Deserialize`。**不含实现、不依赖 agent-lib**。可 `ts-rs`/`schemars` 派生 TS 类型 | serde, futures |
| **mag-core** | `MagService` 的实现：`Engine`、`SessionManager`、per-session driver actor、审批 gate、事件总线、持久化 | mag-service, agent-lib, mag-tools, mag-sources |
| **mag-tools** | `ToolPlugin` trait + 内置工具；产出 agent-lib `Tool` 声明 + async handler + 每工具的 permission 元数据 | agent-lib |
| **mag-sources** | LLM provider 配置（Anthropic/OpenAI）+ 本地 agent 接入（Claude Code/Codex/OpenCode/ACP）+ 凭据存储 | agent-lib, keyring |
| **mag-acp** | **第一个 interface**：实现 ACP agent 角色，把 `dyn MagService` 暴露给 Zed 等 ACP client | mag-service, agent-client-protocol |
| **mag-server** | web interface：axum 静态资源 + 单条 WebSocket 承载 Command/Event；本机 loopback + token | mag-service, axum |
| **mag-tauri** | Tauri interface：`#[tauri::command]` 收 `Command`、`emit` 发 `Event` | mag-service, tauri, mag-core（bin 装配 Engine） |
| **app** | 一套 React 代码；启动时探测宿主注入对应 transport | — |

**划分理由**：
- **接口与实现分离**：`mag-service` 是纯抽象（trait + 中立类型），`mag-core` 是唯一实现。interface adapter
  只依赖 `mag-service`，编译期不碰 agent-lib，也不碰 mag-core 内部——依赖边界清晰（ports-and-adapters：
  service 是 port，interface 是 driving adapter，mag-core 是 driven adapter）。
- **不引入多余间接层**：上层已确定消费 mag-service，直接依赖即可；不再单独抽一个"可替换协议"crate
  （原 `mag-protocol` 并入 mag-service，作为其 service protocol 内容）。
- **Engine 由 bin 装配注入**：具体 `Engine`（mag-core）在应用入口构造后，以 `Arc<dyn MagService>` 注入各
  interface。因此 interface crate 的依赖表里 mag-core 只出现在最终 bin 的装配处（如 mag-tauri 的 `main`），
  库逻辑本身只依赖 `mag-service`。

---

## 3. mag-service 接口与 mag-core 实现

`mag-service` 定义 service 外观（trait + 中立类型），`mag-core::Engine` 实现它。本节先给接口（§3.0），
再讲实现（§3.1 起）。

### 3.0 `MagService` trait（service 外观）

interface adapter 面对的唯一抽象。采用**"命令方法 + 事件流订阅"**模型：状态变更命令是普通 async 方法，
异步产出的事件（流式文本、工具、审批请求、委派进度…）经 `subscribe` 拿到的事件流获取。这样审批往返
（`respond_interaction`）与取消（`cancel`）作为独立方法，与事件流分离，且天然支持一个会话被多个订阅者
观测（GUI 多窗口 / interface 并存）。

```rust
// mag-service（示意签名；object-safe，供 Arc<dyn MagService> 注入）
#[async_trait]
pub trait MagService: Send + Sync {
    // —— 会话管理 ——
    async fn create_session(&self, cfg: SessionConfig) -> Result<SessionId, ServiceError>;
    async fn list_sessions(&self) -> Result<Vec<SessionInfo>, ServiceError>;
    async fn resume_session(&self, id: SessionId) -> Result<(), ServiceError>;
    async fn delete_session(&self, id: SessionId) -> Result<(), ServiceError>;

    // —— 对话 / 运行 ——
    async fn send_message(&self, id: SessionId, input: UserInput) -> Result<RunId, ServiceError>;
    async fn cancel(&self, id: SessionId) -> Result<(), ServiceError>;

    // —— 审批 / 交互往返 ——
    async fn respond_interaction(
        &self, id: SessionId, request_id: RequestId, resp: InteractionResponseWire,
    ) -> Result<(), ServiceError>;

    // —— 事件流（每会话或全局；按 session_id 过滤）——
    fn subscribe(&self, id: Option<SessionId>) -> BoxStream<'static, ServiceEvent>;

    // —— 来源 / 能力 ——
    async fn list_sources(&self) -> Result<Vec<SourceInfo>, ServiceError>;
    async fn probe_local_agents(&self) -> Result<Vec<SourceInfo>, ServiceError>;
}
```

`ServiceEvent` 是中立、可序列化的事件枚举（`TextDelta` / `ToolStarted` / `ToolFinished` /
`InteractionRequested` / `DelegationStarted…` / `RunFinished` / `RunError` …），由 mag-core 从 agent-lib 的
`WireRunEvent`（§9）+ 审批 gate 映射而来。**Command/Event（§4）是这些方法与 `ServiceEvent` 面向 tauri/web
的 wire 编码**，不是另一套模型。

**能力覆盖近全集，interface 各取子集**：trait 一次按 GUI 的完整需求成型（多会话管理、审批、委派、source
探测都在内）。第一个落地的 ACP interface（§5）只映射其中一个子集（单会话 turn/stream/permission/cancel），
其余方法有实现、由 mag-core 覆盖，只是 ACP 暂不调用；GUI/web 接入时无需再改 trait。

### 3.1 Engine + SessionManager（`impl MagService`）

```
Engine
 ├── SessionManager: HashMap<SessionId, SessionHandle>
 ├── event_bus: 每个 Event 带 session_id，广播给订阅的 interface
 ├── source registry     : LLM provider + 本地 agent（§6）
 ├── tool registry       : 插件式工具（§7）
 ├── credential store     : keyring / 加密文件（§3.5）
 └── persistence          : SQLite（§3.6）
```

- **每个会话一个 driver actor**：持有 `mpsc<SessionCommand>` 入口。带 `session_id` 的命令路由到对应
  actor；actor 独占该会话的 agent 状态，规避 agent-lib `Agent`（`&mut self`）的并发问题。
- **为什么用 actor 而非 Mutex**：一次 run 会长时间借用 agent 状态；若用 Mutex，`CancelRun` /
  `RespondInteraction` 这类控制命令会被 run 阻塞。actor 循环里，run 在独立 task 内推进，控制命令走
  **不碰 agent `&mut` 的旁路**（cancel token / pending oneshot），因此永不饿死。

### 3.2 驱动 agent-lib：facade `Agent` + 注入 `IpcApproval`

**关键决策：mag-core 用 facade `Agent`（`Agent::builder()` + `Agent::stream`），并通过
`AgentBuilder::interaction_handler(Arc<dyn InteractionHandler>)` 注入 mag 自己的异步审批 handler
`IpcApproval`（§3.3）。**

背景：mag 的审批必须能**跨进程/跨传输暂停等待**——把审批请求发给前端（或 ACP client），await 用户点
按钮，再折回让 agent 继续。agent-lib 底层的 `InteractionHandler::fulfill` 是 async trait，是天然暂停点；
**agent-lib 的 Milestone 7 已把这个注入口透出到 facade**（`AgentBuilder::interaction_handler(..)`，同步
`run`/`run_full` 与流式 `stream` 两条路径都生效）。因此 mag **不再需要**下沉到 agent 层自组
`HandlerScope`/`drain`——那套样板（自建 id source / spec 装配 / drain 循环 / 逐 delta tap）由 facade 承担。

> **历史注记**：本设计早期版本因 facade 把 interaction handler 硬编码成同步 `FacadeApproval` 而计划"下沉
> 自组 scope"；agent-lib M7-1 落地注入口后此约束消解，改为 facade 注入路径。mag 早期 C1 曾按自组 scope
> 实现，随 C1 重做切到本方案（见 `../TODO.md`）。

driver actor 一次 `SendMessage` 的处理：
1. 组装 / 复用该会话的 facade `Agent`：`Agent::builder().provider(..).model(..).tool(..)
   .approval(policy).interaction_handler(ipc_approval).build()`。
   审批 gate 由 `ApprovalPolicy` 控制（哪些工具暂停），被暂停的交互由注入的 `IpcApproval` **应答**——
   要让某工具走前端审批，用 `ApprovalPolicy::ask_tool(name)`（或对该工具 `Approval::auto_deny()` 兜底），
   read/grep 等只读工具用 `auto_allow` 不打扰（见 §7）。
2. `let mut stream = agent.stream(user_input).await?;` 逐 `RunEvent` 消费。
3. 每个 `RunEvent` 经官方 `RunEvent::to_wire() -> WireRunEvent` 投影后，映射进 mag 的 `Event` 写事件总线
   （`TextDelta`/`ToolStarted`/`ToolFinished`/`ApprovalRequested`/`Done`；`Raw*` 逃生舱折叠为 opaque
   标记，mag 一般忽略）。流式文本由 facade `Agent::stream` 直接产出，mag 不再自建 tap handler。
4. run 成功结束（committed 一致点）后取 `Agent::snapshot()` 写库（§3.6）。

### 3.3 审批 gate：跨传输的 async InteractionHandler

审批是 mag 的架构地基（不是事后可补的特性）。核心是一个实现底层 `InteractionHandler` 的 `IpcApproval`：

```
IpcApproval {
    session_id,
    event_tx,                                  // 向 interface 发事件
    pending: HashMap<RequestId, oneshot::Sender<InteractionResponse>>,
}

async fn fulfill(&self, req: &Interaction, _ctx) -> RequirementResult {
    let request_id = RequestId::new();
    let (tx, rx) = oneshot::channel();
    self.pending.insert(request_id, tx);
    // 逐变体映射 Interaction.kind → InteractionKindWire，带 request_id
    self.event_tx.send(Event::InteractionRequested { session_id, request_id, kind });
    // 天然暂停点：machine 停在这里，直到 interface 送回决定
    select! {
        resp = rx => RequirementResult::Interaction(resp),
        _ = cancel_token.cancelled() => RequirementResult::Interaction(deny_default()),
    }
}
```

- `RespondInteraction { request_id, response }` 命令进 actor → 从 `pending` 取出 sender →
  `tx.send(response)` → 唤醒 driver。
- **对所有 interface 完全一致**：不管审批请求走 Tauri emit、WebSocket、还是 ACP 的
  `session/request_permission`，往返都是同一份 `InteractionRequested` / `RespondInteraction`
  协议，只是管道不同（§5.3 说明 ACP 侧如何桥接）。
- `Interaction` / `InteractionResponse` 全 serde 友好，专为跨进程设计（agent-lib `interaction.rs`）。
- **`InteractionKind::Permission`**（本地 agent / 特权动作，带 category / risk / subject / summary）走
  **同一条通道**——这正是未来 AI-permission 的接缝（§8）。

### 3.4 事件桥接

driver 消费 facade `Agent::stream` 产出的 `RunEvent`，逐个经官方 `RunEvent::to_wire() -> WireRunEvent`
（agent-lib M7-2）投影，再映射进 mag 的 `Event` 写 `event_bus`（`TextDelta` / `ToolStarted` /
`ToolFinished` / `ApprovalRequested` / `DelegationStarted` / `Done` ...）。`WireRunEvent` 已是可序列化的
稳定投影，`RawStream` / `RawNotification` 逃生舱折叠为 opaque 的 `WireRunEvent::Raw`（只记哪个逃生舱触发、
丢载荷），mag 一般忽略。interface 订阅 event_bus 把 `Event` 序列化发出。

> `IpcApproval::fulfill`（§3.3）在 driver 之外**独立** emit `InteractionRequested`——审批是 handler 内的
> 异步暂停点，不经 `RunEvent` 流（facade 的 `RunEvent::ApprovalRequested` 只是 fire-and-forget 通知，mag
> 以 `IpcApproval` 的暂停语义为准）。

### 3.5 凭据存储

`mag-sources` 定义 `CredentialStore` trait：第一版实现优先 OS keyring（`keyring` crate），回退到
`~/.config/mag/credentials`（600 权限，可选加密）。凭据**绝不进 snapshot**（agent-lib snapshot 本就
data-only 无 secret）；恢复会话时从 store 重新注入 `ProviderConfig`。

### 3.6 持久化

SQLite。表：`sessions`（id / config / created_at）、`snapshots`（session_id / agent_snapshot_json /
committed_at）、`messages`（可选冗余，供列表预览）。

- **快照时机**：agent-lib 的 `AgentState` snapshot 只能在 **committed 一致点**取。actor 在每次 run
  成功结束后取快照写库。run 进行中（审批挂起、tool round 中途）不能安全 snapshot。
- **恢复**：`ResumeSession` → 读快照 → `Agent::restore()` builder 重注入 provider / 工具 / approval
  （client / 工具闭包 / 审批 handler 不在快照里，须重建）。本地 agent 委派用 agent-lib 的
  `RestoreExternal::MarkInterrupted` 保守默认（coding agent 可能已改工作区，盲目重启有风险）。
- **⚠ 已知缺口（跟踪中）**：agent-lib 的 `Agent::restore()`（`AgentRestoreBuilder`）**目前没有
  `interaction_handler(..)` 注入口**，恢复出的 `Agent` 强制回落到同步 `FacadeApproval`——即恢复后的会话
  暂时无法用 `IpcApproval` 做跨进程审批。已在 agent-lib 追加后续任务补齐该注入口（与 `AgentBuilder` 对齐）。
  在其落地前，mag 的持久化恢复只对**纯对话 / 只读工具（auto-allow，不触发审批）**的会话完全可用；需审批的
  会话恢复要等该缺口修复（见 `../PLAN.md` R-B、`../TODO.md` C4-1 依赖）。

---

## 4. Command / Event：tauri/web 的 wire 编码

Command/Event 是 `mag-service` 的 service protocol 内容——**`MagService` 方法与 `ServiceEvent` 面向
tauri/web 的一种 wire 编码**（定义在 `mag-service` crate），不是独立的"通用协议层"，也不承载 ACP（ACP 是
§5 的平行编码，直接映射到 `MagService`，不经此）。两个顶层 enum，`serde(tag = "type")` internally-tagged，
便于 TS 侧 discriminated union。`Command` 对应 trait 的命令方法，`Event` 是 `ServiceEvent` 的投影。

### 4.1 Command（tauri/web → `MagService` 方法）

```
Command
├── 会话管理  CreateSession { config } · ListSessions · ResumeSession { id } · DeleteSession { id }
├── 对话      SendMessage { session_id, text, attachments? } · CancelRun { session_id }
├── 审批往返  RespondInteraction { session_id, request_id, response: InteractionResponseWire }
└── 来源/能力 ListSources · ProbeLocalAgents        // 探测 claude-code/codex/... 二进制与能力
```

`InteractionResponseWire` 逐变体镜像 agent-lib `InteractionResponse`（`Approval` / `Answer` /
`Choice` / `Permission`）——底层类型本身 serde 友好，可薄封装或直接复用。

### 4.2 Event（`ServiceEvent` → tauri/web）

```
Event
├── 生命周期  SessionCreated { id, config } · RunStarted { id, run_id } · RunFinished { id, output }
│             · RunError { id, message }
├── 流式      TextDelta { id, text }
├── 工具      ToolStarted { id, trace } · ToolFinished { id, trace }
├── 审批往返  InteractionRequested { id, request_id, kind: InteractionKindWire }
├── 委派      DelegationStarted/Finished/Failed { id, trace } · DelegationMessage { id, .. }
└── 来源      LocalAgentsProbed { available: Vec<SourceInfo> }
```

归一化 payload（`ToolTrace` / `DelegationTrace` 等）在 agent-lib 里已 serde 友好，mag 可复用；
`InteractionKindWire` 覆盖 Approval / Question / Choice / Permission（后者带 category / risk /
summary / subject），供 UI 渲染有意义的审批框。

### 4.3 一份编码，两种管道（tauri/web）

Command/Event 服务 tauri 与 web 两个 interface（ACP 走 §5 的平行编码，不在此表）：

| interface | Command 入 | Event 出 |
|---|---|---|
| Tauri | `invoke("command", { command })` → adapter 调 `MagService` 方法 | `app_handle.emit("mag://event", event)` |
| web | WebSocket text frame = Command JSON → `MagService` 方法 | WebSocket text frame = Event JSON |

adapter 把入站 `Command` 翻译成对 `MagService` 方法的调用，把 `subscribe` 流里的 `ServiceEvent` 投影为
`Event` 发出。前端 `ITransport` 只有 `send(cmd)` + `subscribe(handler)`；`TauriTransport` 与
`WebSocketTransport` 实现同一接口，session store 与 UI 完全不感知传输。**审批往返是这套编码的试金石**：
`InteractionRequested { request_id }` → 前端渲染 → 用户点击 → `RespondInteraction { request_id, response }`
→ adapter 调 `MagService::respond_interaction` → 引擎唤醒 driver；两种管道上是同一份 JSON。

---

## 5. mag 作为 ACP server（第一个落地 interface）

mag 的**第一个落地 interface** 是 **ACP agent**：mag 把自己（含工具、审批、multi-agent）通过 ACP 暴露，
被 Zed 等 ACP client 当作一个 coding agent 使用。选它先行的理由：ACP 是**公开规范**，其 turn / stream /
permission / cancel 的完整回合能反过来校验 `MagService` 接口是否完整（用规范"勾勒 service 外观"）；且它
headless、可脚本化，无需先写 React/Tauri 就能端到端验证 service。

> 本节是概览。**实现级设计（acp crate v1.2.0 的 builder/handler 模型、`session/prompt` 请求 ↔ 事件流的泵、
> 审批桥接、类型映射、测试策略）见 [`ACP.md`](ACP.md)。**

### 5.1 角色与传输

- ACP 定义四种角色：**client**（IDE / 编辑器）、**agent**（AI 服务）、proxy、conductor。mag-acp 实现
  **agent** 角色。
- 传输：stdio 上的 JSON-RPC（Zed 以子进程方式 spawn `mag --acp`，通过 stdin/stdout 通信）。
- 依赖：`agent-client-protocol` crate（agent-lib 已用它做 ACP **client** 方向；mag-acp 用它的 **agent**
  端 API）。协议线版本以该 crate 版本为准。mag-acp **只依赖 `mag-service`**（+ 该 crate），面对
  `dyn MagService`。

### 5.2 请求映射：ACP ↔ `MagService`

mag-acp 是一层**协议翻译器**：把 ACP agent 端收到的请求翻译成对 `MagService` 方法的调用，把 `subscribe`
事件流里的 `ServiceEvent` 翻译成 ACP 的通知 / 请求。**不经 Command/Event**（那是 tauri/web 的平行编码）。
核心映射：

| ACP（client → mag agent） | `MagService` 调用 |
|---|---|
| `initialize` | 协商能力：宣告 mag 支持的 prompt / 工具 / 权限能力（§5.5）|
| `session/new` | `create_session(cfg)`（用 ACP 提供的 cwd / mcp servers 组 `SessionConfig`）|
| `session/load` | `resume_session(id)`（若 mag 持久化里有该会话）|
| `session/prompt` | `send_message(id, input)`，并消费 `subscribe(Some(id))` 直到本轮 stopReason |
| `session/cancel` | `cancel(id)` |

| `ServiceEvent`（`subscribe` 流 → mag-acp → client） | ACP（mag agent → client） |
|---|---|
| `TextDelta` | `session/update` `agent_message_chunk` |
| `ToolStarted` / `ToolFinished` | `session/update` `tool_call` / `tool_call_update` |
| `RunFinished` | `session/prompt` result（`stopReason`）|
| `InteractionRequested`（审批）| `session/request_permission`（见 §5.3）|
| `DelegationStarted/...`（multi-agent）| `session/update`（plan / tool_call 语义映射）|

**ACP 只覆盖 `MagService` 的一个子集**（单会话 turn/stream/permission/cancel）。trait 里 GUI 才需要的方法
（多会话 `list`/`delete`、delegation 树细粒度事件、`probe_local_agents` 运维面）ACP 暂不映射——它们在
mag-core 有实现，等 GUI/web interface 落地时再逐步验证（§10）。

### 5.3 审批在 ACP 侧的桥接（复用同一 gate）

mag 的审批 gate（§3.3）是 interface 无关的。当 mag 作为 ACP agent 运行时，`IpcApproval` emit 的
`InteractionRequested`（经 `subscribe` 流到达 mag-acp）被翻译成 ACP 的 **`session/request_permission`**
请求发给 client（Zed）；Zed 弹出权限 UI，用户决定后回 response，mag-acp 调 `MagService::respond_interaction`
唤醒 driver。

也就是说：**同一套审批往返，在 Tauri 里是弹窗、在 web 里是 WebSocket 消息、在 ACP 里是
`session/request_permission`**——因为它们都只是 `MagService` 的 `InteractionRequested` / `respond_interaction`
往返的不同 wire 编码。审批逻辑一行不改。

### 5.4 双向 ACP 不冲突

- mag 作为 ACP **client**（agent-lib `external-acp`）：把外部 ACP agent 当**来源**（§6），mag 在其中
  是发 prompt 的一方。
- mag 作为 ACP **agent**（mag-acp）：把自己暴露给 Zed，mag 是收 prompt 的一方。

二者是独立的 interface / source，可同时存在：Zed 通过 ACP 驱动 mag，而 mag 内部又可把某个子任务委派给
另一个外部 ACP agent。当"外部 ACP agent"恰好又是另一个 mag 实例时，client 端与 server 端在同一协议上
闭合——这是能力完整后自然可支持的形态，不作为单独的设计目标。

### 5.5 能力如实宣告

`initialize` 时 mag 只宣告**确实支持**的能力（工具、权限桥接、会话 load 等）。这与 agent-lib 的保守
capability 原则一致：不假装支持未实现的特性。哪些工具在 ACP 会话里可用、是否支持 `session/load`，
由 mag 的实际实现决定，如实上报。

---

## 6. AI 能力来源（source）

mag 把所有 AI 能力抽象为"来源"，统一注册、可混用。两大类：

### 6.1 LLM API 来源

Anthropic / OpenAI（经 agent-lib `ProviderConfig`）。用于纯对话会话、以及作为工具 agent /
supervisor / 本地 LLM subagent 的后端。凭据经 `CredentialStore`（§3.5）。

### 6.2 本地 agent 来源

Claude Code / Codex / OpenCode / 任意 ACP agent（经 agent-lib `ManagedExternalAgent`，behind
feature flags `external-claude-code` / `external-codex` / `external-opencode` / `external-acp`）。

- **发现与探测**：`ProbeLocalAgents` 命令运行时 probe 二进制与能力（agent-lib 各 adapter 的 probe）。
  缺失二进制 / 未登录 / 能力不支持 → **保守 skip**，UI 显示"不可用"而非崩溃。
- **worktree 隔离**：本地 agent 在临时 git worktree 里跑，agent-lib 负责其生命周期
  （`EphemeralGitWorktree`，干净关闭才拆除，异常保留供检查）。
- **生产级 session handler 已就绪**：agent-lib M7-4 提供了
  `facade::default_external_session_handler(&agent) -> Arc<RegistryExternalSessionHandler>`，把 live
  adapter（ClaudeCode/Codex/OpenCode/ACP）+ registry 接好，mag 经
  `ManagedExternalAgentBuilder::session_handler(..)` 直接注入，不再自己 wire。对应 feature 未开时该函数
  fail-fast 返回 `FacadeError::ExternalAgent`（指明要开哪个 feature），mag 据此走"不可用" skip。

---

## 7. 工具：插件式 registry

`mag-tools` 定义 `ToolPlugin` trait：

```
trait ToolPlugin {
    fn declaration(&self) -> Tool;                    // name / desc / json_schema（agent-lib Tool::function_with_schema）
    async fn invoke(&self, ctx: ToolContext, args) -> ToolResult;
    fn permission(&self) -> Option<PermissionSpec>;   // category / risk，用于审批
}
```

- **第一版最小集**：`read_file` / `list_dir` / `grep`（只读，auto-allow）+ `shell`（需审批）。
  agent-lib 只有工具框架无具体实现——fs/shell 全由 mag 自建。
- registry 收集所有 plugin → 产出 `ToolSetRef`（declarations）注入 `AgentSpec`，并产出一个
  `ToolRegistry` 实现（按 name dispatch 到 plugin）注入 scope。**增量扩展 = 注册新 plugin**，不碰
  driver / 协议。
- `ToolContext` 带 `worktree` / `cancel` / `tool_call_id`：shell 用 `cancel` 支持中断、用 `worktree`
  约束路径。
- 工具的 `permission()` 元数据决定它是否触发审批 gate（§3.3），并携带 category / risk 供 UI 与未来
  AI-permission 使用。

---

## 8. 后续增强的扩展点（第一版不实现，但预留位置）

### 8.1 AI-based permission policy

- **位置**：因为 mag 注入了整体的 `IpcApproval`（§3.2），它是被暂停交互的唯一应答方——包括
  `InteractionKind::Permission`（本地 agent / 特权动作）。所以 permission 决策的落点在 `IpcApproval` 内
  处理 `Permission` 的分支（而非 agent-lib 的 `ApprovalPolicy::on_permission` 钩子——后者仅在**未**注入
  整体 handler 时生效，是给不下沉宿主的备选）。
- **结构**：第一版就把它写成 `PermissionDecider` 的调用点：`RuleDecider`（命中规则直接 allow/deny）→
  未命中 → fallback 到"问前端 / ACP client"。
- **未来**：插入 `LlmDecider` 作为兜底（把 `PermissionRequest` 的 category / risk / subject / summary
  喂 LLM 判定安全性），再不行才问人。`PermissionRequest` 全 serde 且 risk 有序，天然适合喂模型。这个
  接缝**不改协议、不改 driver**，只换 `PermissionDecider` 实现。

### 8.2 AI-based subagent routing

- **位置**：agent-lib 底层 `agent::external::{TaskEvaluator, Verifier}` trait —— 接 LLM 的正式接缝，
  `Dispatcher` / `Escalator` 已实现 budget-aware 路由 / 升级。
- **第一版**：用 model-routed delegation（每 delegate 一个 `ask_<name>` 工具，由 supervisor 模型自己
  决定派给谁）。
- **未来**：切到 dispatcher-routed，注入 mag 自己的 LLM-backed `TaskEvaluator`（用 AI 决定任务给哪个
  后端）。mag 侧预留 = `SessionConfig` 里放 `routing: model_routed | dispatcher`，delegate 注册表结构
  对两者通用。

### 8.3 其它扩展点

- **新工具** = 注册 `ToolPlugin`。
- **新 AI 来源** = 在 source registry 注册（LLM provider 或本地 agent），统一走 `ask_<name>` 委派。

---

## 9. 接入 agent-lib 的关键约束

以下是 mag 装配 agent-lib 时必须遵守的硬约束（已核对 agent-lib 源码；agent-lib **Milestone 7 已落地**，
下列约束反映 M7 之后的接入面）：

1. **动态审批用 facade 注入口**。经 `AgentBuilder::interaction_handler(Arc<dyn InteractionHandler>)`
   （agent-lib M7-1）注入 mag 的 `IpcApproval`，`run`/`run_full`/`stream` 三条路径都生效；`fulfill` 里发
   请求 → await oneshot → 收回答 → 返回，machine 真正停在 `.await`。注入的 handler 是被暂停交互的**唯一
   应答方**，但**哪些工具会暂停仍由 `ApprovalPolicy` 控制**（用 `ask_tool` / `auto_deny` 让工具过审批，
   `auto_allow` 不打扰）。`InteractionHandler` 在 `agent_lib::agent`（不在 prelude，需显式 import）。
2. **用官方 `WireRunEvent` 序列化**（agent-lib M7-2）。`RunEvent` 本身不派生 serde；用
   `RunEvent::to_wire() -> WireRunEvent`（`WireRunEvent`/`WireRunOutput` 在 prelude）投影后再映射进 mag
   `Event`。`RawStream`/`RawNotification` 折叠为 opaque `WireRunEvent::Raw`。
3. **`Agent` 是会话对象**（`&mut self`）→ 放 per-session actor 后（§3.1）。纯对话可用 facade
   `ChatSession`（无工具）；带工具 / 审批 / 委派用 `Agent` + 注入 `IpcApproval`。
4. **工具**用 `Tool::function_with_schema`（无需 feature），handler 是 async 闭包 + `ToolContext`；经
   `AgentBuilder::tool(..)` 注册。agent-lib 只有工具框架无具体实现——fs/shell 全由 mag 自建（§7）。
5. **snapshot/restore**：`Agent::snapshot()` 只在 committed 一致点取，data-only 无 secret；
   `Agent::restore()` builder 重注入 provider / 工具 / approval。**⚠ restore builder 目前无
   `interaction_handler` 注入口**（恢复会话回落到同步 `FacadeApproval`）——已反馈 agent-lib 补齐；在其
   落地前，需审批的会话恢复受限（见 §3.6、`../PLAN.md` R-B）。
6. **富化的 `ApprovalRequest`**（agent-lib M7-3，在 prelude）带 `tool_name`/`call_id`/`reason`/`input`
   （已脱敏摘要），UI 可据此渲染有意义的审批框。Permission 通道（`agent/permission.rs`）带 category /
   有序 risk / subject，是 AI-permission 落点（§8.1）。
7. **本地 agent** behind feature flags；用 agent-lib M7-4 的
   `facade::default_external_session_handler(&agent)` 直接得到生产级 `ExternalSessionHandler`（§6.2）。
8. **AI routing / verifier** 接缝：facade `Delegation::dispatcher_evaluator(..)` /
   `dispatcher_verifier(..)`（agent-lib M7-5，接自定义 `TaskEvaluator`/`Verifier`）；permission 决策
   备选钩子 `ApprovalPolicy::on_permission(..)`（但 mag 整体注入 `IpcApproval` 时以后者为准，§8.1）。

> agent-lib 的 **Milestone 7（宿主嵌入接入面，已 `[DONE]`）** 补齐了上述注入口，使 mag **无需下沉自组
> `HandlerScope`/`drain`**，绝大部分留在 facade 内。唯一残留缺口是约束 5 的 restore 注入口，已在 agent-lib
> 追加后续任务跟踪。

---

## 10. 分阶段里程碑

**先做 service（mag-core，见 `../PLAN.md`/`../TODO.md` 的 C 系列引擎主干），再做 interface。** service 稳定后，
**第一个 interface 走 ACP**（公开规范，headless 可脚本化验证 service 外观），GUI/web 后置。每阶段可独立
demo，协议向后兼容累加。

| 阶段 | 主题 | 产出 | 验证 |
|---|---|---|---|
| **S** | service 主干 | mag-core 引擎（会话 / 流式 / 工具 / 审批 / 持久化）+ 抽取 `mag-service` 抽象接口、`Engine impl MagService`。见 `../PLAN.md` C 系列 | 全离线单元/集成测试（fake `LlmClient`）；`MagService` trait 一次按近全集成型 |
| **I1** | ACP interface（第一个） | mag-acp：`dyn MagService` ↔ ACP（`initialize` / `session/new` / `session/prompt` / `session/update` streaming / `session/request_permission` / `session/cancel`）；`mag --acp` 子进程模式 | Zed（或 agent-lib 的 ACP client）驱动 mag：发 prompt 看到流式回复；工具审批经 `session/request_permission` 往返；cancel 生效。ACP 只用 `MagService` 子集 |
| **I2** | 本地 agent 来源 | 打开 external features；`probe_local_agents`；用 `default_external_session_handler(..)`；`.external_agent(..)` 委派 | 单个本地 agent 在 worktree 跑改代码任务，看到 delegation trace + artifact；缺二进制时 skip |
| **I3** | multi-agent | 同时挂本地 LLM subagent + 本地 CLI agent，共享 `ask_<name>` 委派 | supervisor 把"审查"派给 LLM subagent、"改代码"派给 codex，trace 正确，usage 聚合，审批各层受控 |
| **I4** | GUI / web interface | mag-tauri + mag-server + React app（一套前端两宿主）；映射 `MagService` 的全集（含多会话管理、delegation 树可视化、source 面板）| 起 app 多会话对话/工具/审批/委派可视化；web 侧同一前端；Command/Event wire + TS codegen |

> 顺序取向：I1（ACP）用公开规范把 `MagService` 的核心回合（turn/stream/permission/cancel）验证扎实；
> I4（GUI/web）才引入重前端，并补齐 ACP 未覆盖的全集能力（多会话管理、委派可视化、source 运维面）。
> "外部 ACP agent 恰好是另一个 mag"这种闭环形态，是 I1 + I2 能力齐备后自然可跑的，不单列为里程碑。

---

## 11. 难点与风险

1. **审批异步暂停是地基，不能事后补**。通过 facade 注入口 `AgentBuilder::interaction_handler(IpcApproval)`
   解决（agent-lib M7-1）；`fulfill` 里真正 await，machine 停在暂停点。service 主干（阶段 S）早期用 facade
   `ChatSession` 作纯对话脚手架，随后切到 `Agent` + 注入。
2. **restore 无审批注入口（残留缺口）**。`Agent::restore()` 目前回落到同步 `FacadeApproval`，恢复后的会话
   暂时无法跨进程审批。已反馈 agent-lib 补齐；在其落地前，需审批的会话恢复受限（纯对话 / 只读工具会话不受
   影响）。见 §3.6、`../PLAN.md` R-B。
3. **snapshot 只能在 committed 一致点**。run 进行中崩溃/退出，会话回到上一个 committed 点，进行中的
   turn 丢失；本地 agent 更麻烦（可能已改工作区）→ `MarkInterrupted` 保守默认。
4. **web / ACP 的安全边界**。web 绑 `127.0.0.1` 不够（DNS-rebinding / 本机其它进程）→ 加启动时生成的
   loopback token（WebSocket 握手校验）+ Origin 检查。ACP 经 stdio 由父进程（Zed）控制，信任边界是
   spawn mag 的进程。**shell 工具审批是最后防线，任何 interface 都不能省。**
5. **actor 长借用 vs 控制命令**。run 长时间占用 agent `&mut`，`cancel` / `respond_interaction` 必须走
   不碰 `&mut` 的旁路（cancel token + pending oneshot），否则命令饿死（§3.1 / §3.3 已规避）。
6. **`MagService` 一次成型 vs 逐步验证**。trait 按近全集设计，但 I1（ACP）只验证子集——GUI 才用到的方法
   （多会话管理、委派可视化、source 面板）在 I1 时**有实现但缺端到端验证**，到 I4 才被真正行使。风险是这些
   方法的签名可能在 I4 才发现不合用。缓解：I1 设计 trait 时就带上 GUI 视角评审，避免 ACP 视角把签名带窄。
7. **前后端协议漂移**。用 `ts-rs` / `schemars` 从 `mag-service` 的 Command/Event 生成 TS 类型，CI 校验，
   避免手写 union 与 Rust enum 不一致。
8. **ACP 协议版本与能力协商**。ACP 仍在演进；`initialize` 的能力协商要如实、向后兼容；mag 宣告的能力
   必须与实际实现一致，否则 client 会调用不支持的路径。
