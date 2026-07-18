# mag 设计文档

`mag` 是一个基于 [`agent-lib`](../agent-lib) 的增强版编码 agent 应用。它把 agent-lib 的 sans-io
agent 引擎包装成一个**面向人的产品**：GUI（Tauri）与 web 提供一致的用户体验，多种 AI 能力来源
（LLM API + 本地 agent 程序）统一接入，并支持 multi-agent 混合调度。此外 mag 自身可作为
**ACP（Agent Client Protocol）server** 被 Zed 等编辑器反向驱动。

本文档描述 mag 的架构、模块划分、传输协议、以及关键设计取舍。agent-lib 的内部设计见其
[`DESIGN.md`](../agent-lib/DESIGN.md)；mag 接入 agent-lib 的具体 API 约束见 §9。

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

mag 的核心洞察是：**"命令进、事件出"的驱动面有三种，但它们背后是同一个引擎**。

```
        ┌─────────────┐   ┌─────────────┐   ┌──────────────────┐
        │ Tauri GUI   │   │  Web 浏览器  │   │  ACP client(Zed) │
        └──────┬──────┘   └──────┬──────┘   └────────┬─────────┘
               │ invoke/emit     │ WebSocket         │ stdio JSON-RPC
        ┌──────┴─────────────────┴───────────────────┴─────────┐
        │                  传输层（三种 front door）              │
        │  mag-tauri        mag-server          mag-acp          │
        └──────────────────────────┬────────────────────────────┘
                                    │  Command / Event（统一协议）
        ┌───────────────────────────┴───────────────────────────┐
        │                    mag-core :: Engine                   │
        │   SessionManager · per-session driver actor · 审批 gate  │
        │   事件总线 · 凭据存储 · 持久化 · source/tool registry     │
        └───────────────────────────┬───────────────────────────┘
                                    │  装配 + 驱动
        ┌───────────────────────────┴───────────────────────────┐
        │                       agent-lib                         │
        │   Client · Conversation · Agent(machine/effect) · facade │
        └─────────────────────────────────────────────────────────┘
```

三个"front door"——Tauri、web、ACP——把各自的传输帧翻译成 mag 内部**统一的 `Command`/`Event`
协议**（§4），交给 `mag-core::Engine`。引擎不知道自己跑在哪个 front door 后面：它只处理命令、产生
事件。这让 GUI、web、ACP server 三种形态共享 100% 的核心逻辑（agent 驱动、审批、工具、持久化、
multi-agent）。

> **ACP 的双重身份**：mag 既是 ACP **client**（通过 agent-lib 的 `external-acp` 消费外部 ACP
> agent，作为一种"来源"），也是 ACP **agent/server**（通过 `mag-acp` 把自己暴露给 Zed）。二者
> 方向相反、互不冲突：前者是 §6 的一种 source，后者是本节的第三个 front door。

---

## 2. Crate / 目录结构

mag 是一个 Cargo workspace，前后端分离，协议单独成 crate。

```
mag/
├── Cargo.toml                  # [workspace]
├── crates/
│   ├── mag-protocol/           # 传输无关的 Command/Event wire 协议（纯数据）
│   ├── mag-core/               # 引擎：session manager / driver / 审批 / 持久化 / registry
│   ├── mag-tools/              # 插件式工具 registry + 内置工具（fs/shell/...）
│   ├── mag-sources/            # AI 来源：LLM provider 配置 + 本地 agent 接入 + 凭据
│   ├── mag-server/             # web front door：axum + WebSocket
│   ├── mag-acp/                # ACP server front door：mag 作为 ACP agent 被反向驱动
│   └── mag-tauri/              # Tauri front door：src-tauri，invoke/emit 桥接
├── app/                        # React + TS 前端（Vite），喂 Tauri webview 和浏览器
│   ├── src/
│   │   ├── transport/          # ITransport：TauriTransport | WebSocketTransport
│   │   ├── protocol/           # 从 mag-protocol 生成的 TS 类型
│   │   ├── session/            # 会话状态、事件 reducer
│   │   └── components/
│   └── ...
└── DESIGN.md
```

| crate | 职责 | 依赖 |
|---|---|---|
| **mag-protocol** | `Command` / `Event` 两个顶层 enum + 全部 payload，全 `Serialize/Deserialize`。可用 `ts-rs`/`schemars` 派生 TS 类型固化契约。**不依赖 agent-lib** | serde |
| **mag-core** | 引擎心脏：`Engine`、`SessionManager`、per-session driver actor、审批 gate、事件总线、持久化。**传输无关** | agent-lib, mag-protocol, mag-tools, mag-sources |
| **mag-tools** | `ToolPlugin` trait + 内置工具；产出 agent-lib `Tool` 声明 + async handler + 每工具的 permission 元数据 | agent-lib |
| **mag-sources** | LLM provider 配置（Anthropic/OpenAI）+ 本地 agent 接入（Claude Code/Codex/OpenCode/ACP）+ 凭据存储 | agent-lib, keyring |
| **mag-server** | web 入口：axum 静态资源 + 单条 WebSocket 承载协议；本机 loopback + token | mag-core, axum |
| **mag-acp** | ACP server：实现 ACP agent 角色，把 Engine 暴露给 Zed 等 client | mag-core, agent-client-protocol |
| **mag-tauri** | Tauri 入口：`#[tauri::command]` 收 `Command`，`emit` 发 `Event` | mag-core, tauri |
| **app** | 一套 React 代码；启动时探测宿主注入对应 transport | — |

**划分理由**：核心逻辑（agent 驱动、审批、持久化）对三个 front door 完全一致，压进 `mag-core::Engine`
的 API；三个传输 crate 各写一层薄胶水。`mag-protocol` 独立且不依赖 agent-lib，保证协议是稳定契约、
可被前端 codegen 消费、不被 agent-lib 内部类型污染。

---

## 3. mag-core：引擎与会话模型

### 3.1 Engine + SessionManager

```
Engine
 ├── SessionManager: HashMap<SessionId, SessionHandle>
 ├── event_bus: 每个 Event 带 session_id，广播给订阅的 front door
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

### 3.2 驱动 agent-lib：下沉到 agent 层自组 scope

**关键决策：mag-core 不用 facade `Agent::run/stream`，而是下沉到 agent 层自组 `HandlerScope` +
`drain`。**

原因（详见 §9）：mag 的审批必须能**跨进程/跨传输暂停等待**——把审批请求发给前端（或 ACP client），
await 用户点按钮，再折回让 agent 继续。facade 把 interaction handler 硬编码成同步的 `FacadeApproval`，
无法 await 跨进程回答。而 agent-lib 底层的 `InteractionHandler::fulfill` 是 async trait，是天然暂停点。
因此 mag 照 `agent-lib/examples/agent_chat.rs` 的骨架自组 scope，把审批 handler 换成 mag 自己的
`IpcApproval`（§3.3）。

> 若 agent-lib 落地了其 `PLAN.md` Milestone 7（`AgentBuilder::interaction_handler(..)` 注入口），
> mag 可改回留在 facade 内、注入 `IpcApproval`，减少自组 scope 的样板。在那之前走自组 scope 路径。

driver actor 一次 `SendMessage` 的处理：
1. 组装 / 复用该会话的 `DefaultAgentMachine`（`AgentSpec` + `AgentState` + mag 的 id source）。
2. 组装一层 `HandlerScope`：LLM handler（流式，emit `TextDelta`）、tool handler（mag-tools registry）、
   interaction handler（`IpcApproval`）。
3. `drain(&mut machine, input, &scope, ..)` 驱动本轮，直到 committed 或 loop 预算耗尽。
4. run 成功结束后取 `AgentState` snapshot 写库（§3.6）。

### 3.3 审批 gate：跨传输的 async InteractionHandler

审批是 mag 的架构地基（不是事后可补的特性）。核心是一个实现底层 `InteractionHandler` 的 `IpcApproval`：

```
IpcApproval {
    session_id,
    event_tx,                                  // 向 front door 发事件
    pending: HashMap<RequestId, oneshot::Sender<InteractionResponse>>,
}

async fn fulfill(&self, req: &Interaction, _ctx) -> RequirementResult {
    let request_id = RequestId::new();
    let (tx, rx) = oneshot::channel();
    self.pending.insert(request_id, tx);
    // 逐变体映射 Interaction.kind → InteractionKindWire，带 request_id
    self.event_tx.send(Event::InteractionRequested { session_id, request_id, kind });
    // 天然暂停点：machine 停在这里，直到 front door 送回决定
    select! {
        resp = rx => RequirementResult::Interaction(resp),
        _ = cancel_token.cancelled() => RequirementResult::Interaction(deny_default()),
    }
}
```

- `RespondInteraction { request_id, response }` 命令进 actor → 从 `pending` 取出 sender →
  `tx.send(response)` → 唤醒 driver。
- **对三个 front door 完全一致**：不管审批请求走 Tauri emit、WebSocket、还是 ACP 的
  `session/request_permission`，往返都是同一份 `InteractionRequested` / `RespondInteraction`
  协议，只是管道不同（§5.3 说明 ACP 侧如何桥接）。
- `Interaction` / `InteractionResponse` 全 serde 友好，专为跨进程设计（agent-lib `interaction.rs`）。
- **`InteractionKind::Permission`**（本地 agent / 特权动作，带 category / risk / subject / summary）走
  **同一条通道**——这正是未来 AI-permission 的接缝（§8）。

### 3.4 事件桥接

driver 内所有 emit 点（`TextDelta` / `ToolStarted` / `InteractionRequested` / `DelegationStarted` /
...）写 `event_bus`。因为自组 scope，mag 直接在 handler 里 emit **mag 自己的 `Event`**，绕开了
agent-lib `RunEvent`（后者整体不派生 serde，含逃生舱变体，见 §9）。front door 订阅 event_bus 把
`Event` 序列化发出。

### 3.5 凭据存储

`mag-sources` 定义 `CredentialStore` trait：第一版实现优先 OS keyring（`keyring` crate），回退到
`~/.config/mag/credentials`（600 权限，可选加密）。凭据**绝不进 snapshot**（agent-lib snapshot 本就
data-only 无 secret）；恢复会话时从 store 重新注入 `ProviderConfig`。

### 3.6 持久化

SQLite。表：`sessions`（id / config / created_at）、`snapshots`（session_id / agent_snapshot_json /
committed_at）、`messages`（可选冗余，供列表预览）。

- **快照时机**：agent-lib 的 `AgentState` snapshot 只能在 **committed 一致点**取。actor 在每次 run
  成功结束后取快照写库。run 进行中（审批挂起、tool round 中途）不能安全 snapshot。
- **恢复**：`ResumeSession` → 读快照 → 重新装配 machine + scope（client / 工具闭包 / 审批 handler
  不在快照里，须重建）。本地 agent 委派用 agent-lib 的 `RestoreExternal::MarkInterrupted` 保守默认
  （coding agent 可能已改工作区，盲目重启有风险）。

---

## 4. 传输无关协议（mag-protocol）

两个顶层 enum，`serde(tag = "type")` internally-tagged，便于 TS 侧 discriminated union。

### 4.1 Command（front door → 引擎）

```
Command
├── 会话管理  CreateSession { config } · ListSessions · ResumeSession { id } · DeleteSession { id }
├── 对话      SendMessage { session_id, text, attachments? } · CancelRun { session_id }
├── 审批往返  RespondInteraction { session_id, request_id, response: InteractionResponseWire }
└── 来源/能力 ListSources · ProbeLocalAgents        // 探测 claude-code/codex/... 二进制与能力
```

`InteractionResponseWire` 逐变体镜像 agent-lib `InteractionResponse`（`Approval` / `Answer` /
`Choice` / `Permission`）——底层类型本身 serde 友好，可薄封装或直接复用。

### 4.2 Event（引擎 → front door）

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

### 4.3 一份协议，四种管道

| front door | Command 入 | Event 出 |
|---|---|---|
| Tauri | `invoke("command", { command })` → `#[tauri::command]` | `app_handle.emit("mag://event", event)` |
| web | WebSocket text frame = Command JSON | WebSocket text frame = Event JSON |
| ACP（§5） | ACP 请求翻译成 Command | Event 翻译成 ACP `session/update` / `session/request_permission` |

前端 `ITransport` 只有 `send(cmd)` + `subscribe(handler)` 两个方法；`TauriTransport` 与
`WebSocketTransport` 实现同一接口，session store 与 UI 组件完全不感知传输。**审批往返是这套协议的
试金石**：`InteractionRequested { request_id }` → 前端渲染 → 用户点击 → `RespondInteraction
{ request_id, response }` → 引擎唤醒 driver，四种管道上是同一份 JSON。

---

## 5. mag 作为 ACP server

除了 Tauri 与 web，mag 的第三个 front door 是 **ACP agent**：mag 把自己（含工具、审批、multi-agent）
通过 ACP 暴露，被 Zed 等 ACP client 当作一个 coding agent 使用。

### 5.1 角色与传输

- ACP 定义四种角色：**client**（IDE / 编辑器）、**agent**（AI 服务）、proxy、conductor。mag-acp 实现
  **agent** 角色。
- 传输：stdio 上的 JSON-RPC（Zed 以子进程方式 spawn `mag --acp`，通过 stdin/stdout 通信）。
- 依赖：`agent-client-protocol` crate（agent-lib 已用它做 ACP **client** 方向；mag-acp 用它的 **agent**
  端 API）。协议线版本以该 crate 版本为准。

### 5.2 请求映射：ACP ↔ mag Command/Event

mag-acp 是一层**协议翻译器**：把 ACP agent 端要处理的请求翻译成 mag 内部 `Command`，把引擎产生的
`Event` 翻译成 ACP 的通知 / 请求。核心映射：

| ACP（client → mag agent） | mag 内部动作 |
|---|---|
| `initialize` | 协商能力：宣告 mag 支持的 prompt / 工具 / 权限能力 |
| `session/new` | `Command::CreateSession`（用 ACP 提供的 cwd / mcp servers 组 SessionConfig）|
| `session/load` | `Command::ResumeSession`（若 mag 持久化里有该会话）|
| `session/prompt` | `Command::SendMessage`；本轮 agent 运行直到 stopReason |
| `session/cancel` | `Command::CancelRun` |

| mag `Event`（引擎 → mag agent → client） | ACP（mag agent → client） |
|---|---|
| `TextDelta` | `session/update` `agent_message_chunk` |
| `ToolStarted` / `ToolFinished` | `session/update` `tool_call` / `tool_call_update` |
| `RunFinished` | `session/prompt` result（`stopReason`）|
| `InteractionRequested`（审批）| `session/request_permission`（见 §5.3）|
| `DelegationStarted/...`（multi-agent）| `session/update`（plan / tool_call 语义映射）|

### 5.3 审批在 ACP 侧的桥接（复用同一 gate）

mag 的审批 gate（§3.3）是传输无关的。当 mag 作为 ACP agent 运行时，`IpcApproval` emit 的
`InteractionRequested` 被 mag-acp 翻译成 ACP 的 **`session/request_permission`** 请求发给 client
（Zed）；Zed 弹出权限 UI，用户决定后回 response，mag-acp 翻译回 `RespondInteraction`，唤醒 driver。

也就是说：**同一套审批往返，在 Tauri 里是弹窗、在 web 里是 WebSocket 消息、在 ACP 里是
`session/request_permission`**。审批逻辑一行不改。

### 5.4 双向 ACP 不冲突

- mag 作为 ACP **client**（agent-lib `external-acp`）：把外部 ACP agent 当**来源**（§6），mag 在其中
  是发 prompt 的一方。
- mag 作为 ACP **agent**（mag-acp）：把自己暴露给 Zed，mag 是收 prompt 的一方。

二者是独立的 front door / source，可同时存在：Zed 通过 ACP 驱动 mag，而 mag 内部又可把某个子任务
委派给另一个外部 ACP agent。

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
- **需生产级 session handler**：agent-lib 目前只有 test double，把 live adapter wire 成 registry-backed
  `ExternalSessionHandler` 的最后一公里由 mag 承担（或待 agent-lib M7-4 提供后直接用，见 §9）。

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

- **位置**：`IpcApproval::fulfill` 内处理 `InteractionKind::Permission` 的分支。
- **结构**：第一版就把它写成 `PermissionDecider` trait 的调用点：`RuleDecider`（命中规则直接
  allow/deny）→ 未命中 → 第一版 fallback 到"问前端 / ACP client"。
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

以下是 mag 装配 agent-lib 时必须遵守的硬约束（已核对 agent-lib 源码）：

1. **动态审批必须下沉到 agent 层**。facade 把 interaction handler 硬编码成同步 `FacadeApproval`
   （`Approval::ask` 是同步 `Fn`），无法 await 跨进程回答。正确做法：实现底层 async
   `InteractionHandler`（`agent/drive.rs`，`async fn fulfill`），在 `fulfill` 里发请求 → await
   oneshot → 收回答 → 返回。自组 `HandlerScope` + `drain`（样板 `examples/agent_chat.rs`）。
   *（若 agent-lib M7-1 落地 `interaction_handler(..)` 注入口，可改回留在 facade。）*
2. **agent-lib `RunEvent` 整体不派生 serde**（含 `RawStream` / `RawNotification` 逃生舱）。mag 定义
   自己的 `Event`；自组 scope 时直接 emit mag `Event`，绕开 RunEvent 映射。
3. **`Agent` 是会话对象**（`&mut self`）→ 放 per-session actor 后（§3.1）。纯对话可用 facade
   `ChatSession`（无工具）；带工具 / 审批 / 委派用自组 scope。
4. **工具**用 `Tool::function_with_schema`（无需 feature），handler 是 async 闭包 + `ToolContext`。
5. **snapshot/restore** 只在 committed 一致点取；snapshot data-only 无 secret；恢复重注入
   client / 工具 / 审批 handler。
6. **Permission 通道**（`agent/permission.rs`）带 category / 有序 risk / subject，是 AI-permission 落点
   （§8.1）；facade 默认 deny。
7. **本地 agent** behind feature flags；需生产级 `ExternalSessionHandler`（agent-lib 目前只有 test
   double，见 §6.2）。
8. **AI routing / verifier** 接缝在 `agent::external::{TaskEvaluator, Verifier}`（§8.2）。

> agent-lib 侧已在其 `PLAN.md` / `TODO.md` 规划了 **Milestone 7（宿主嵌入接入面）**，专门补齐上述
> 约束 1、2、6、7、8 对应的 facade 注入口。届时 mag 可减少下沉重写的样板，更多留在 facade 内。

---

## 10. 分阶段里程碑

每阶段可独立 demo，协议向后兼容累加（只加 enum 变体）。

| 阶段 | 主题 | 产出 | 验证 |
|---|---|---|---|
| **M0** | 管道打通 | workspace 骨架、mag-protocol 最小 enum、单会话纯对话流式（先一个 front door，可先用 facade `ChatSession::stream`）、最简 React UI | 起 app 输入问题看到流式输出；web 侧 JSON 命令收到流 |
| **M1** | 工具 agent + 交互审批 | 切到自组 scope + `IpcApproval`；mag-tools 最小集（read/grep 自动 + shell 审批）；两个 front door（Tauri + web）跑同一前端；持久化上线 | shell 命令触发前端审批 → 允许/拒绝 → driver 正确恢复/取消；重启后 ResumeSession 看到历史；审批在两传输都往返 |
| **M2** | 本地 agent 来源 | 打开 external features；`ProbeLocalAgents`；把 live adapter wire 成 `ExternalSessionHandler`；`.external_agent(..)` 委派 | 单个本地 agent 在 worktree 跑改代码任务，前端看到 delegation trace + artifact；缺二进制时 UI 显示 skip |
| **M3** | multi-agent | 同时挂本地 LLM subagent + 本地 CLI agent，共享 `ask_<name>` 委派；前端展示 delegation 树 | supervisor 把"审查"派给 LLM subagent、"改代码"派给 codex，trace 正确，usage 聚合，审批各层受控 |
| **M4** | ACP server | mag-acp front door：`initialize` / `session/new` / `session/prompt` / `session/update` streaming / `session/request_permission`；`mag --acp` 子进程模式 | Zed 配置 mag 为 ACP agent，发 prompt 看到流式回复；工具审批通过 `session/request_permission` 弹到 Zed；`session/cancel` 生效 |

---

## 11. 难点与风险

1. **审批异步暂停是地基，不能事后补**。已通过下沉到 agent 层 `InteractionHandler::fulfill` 解决；要求
   第一版（M1）即自组 scope。M0 用 facade `ChatSession` 只是临时脚手架。
2. **snapshot 只能在 committed 一致点**。run 进行中崩溃/退出，会话回到上一个 committed 点，进行中的
   turn 丢失；本地 agent 更麻烦（可能已改工作区）→ `MarkInterrupted` 保守默认。
3. **本地 agent 无生产 handler**。M2 要自己 wire adapter → `ExternalSessionHandler`；probe + 保守 skip
   降低"环境没装"的风险。
4. **web / ACP 的安全边界**。web 绑 `127.0.0.1` 不够（DNS-rebinding / 本机其它进程）→ 加启动时生成的
   loopback token（WebSocket 握手校验）+ Origin 检查。ACP 经 stdio 由父进程（Zed）控制，信任边界是
   spawn mag 的进程。**shell 工具审批是最后防线，任何 front door 都不能省。**
5. **actor 长借用 vs 控制命令**。run 长时间占用 agent `&mut`，`CancelRun` / `RespondInteraction` 必须走
   不碰 `&mut` 的旁路（cancel token + pending oneshot），否则命令饿死（§3.1 / §3.3 已规避）。
6. **前后端协议漂移**。用 `ts-rs` / `schemars` 从 mag-protocol 生成 TS 类型，CI 校验，避免手写 union
   与 Rust enum 不一致。
7. **ACP 协议版本与能力协商**。ACP 仍在演进；`initialize` 的能力协商要如实、向后兼容；mag 宣告的能力
   必须与实际实现一致，否则 client 会调用不支持的路径。
