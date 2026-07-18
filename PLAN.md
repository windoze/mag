# 实施计划：mag-core（引擎核心）

> 唯一设计输入：[`DESIGN.md`](DESIGN.md)（尤其 §1 全局架构、§3 mag-core、§4 协议、§9 接入 agent-lib 约束）。
> 底层库 [`agent-lib`](../agent-lib) 的接入面见其 [`README.md`](../agent-lib/README.md) 与
> [`DESIGN.md`](../agent-lib/DESIGN.md)；facade 缺口与后续注入口见 agent-lib 的
> [`PLAN.md`](../agent-lib/PLAN.md) Milestone 7。
>
> **本计划只覆盖 mag-core（引擎）+ 其直接依赖的 mag-protocol / mag-tools / mag-sources 的核心部分。**
> 上层（mag-tauri / mag-server / mag-acp / app 前端）**不在本计划范围**——它们必须等 mag-core 充分测试
> 通过、契约稳定后，再按 `DESIGN.md` §10 里程碑 M1+ 单独规划。逐任务清单见 [`TODO.md`](TODO.md)。

## 目标

落地 `DESIGN.md` §3 描述的引擎核心：一个**传输无关**的 `mag-core::Engine`，能装配并驱动 agent-lib、
管理多会话、把跨传输的审批往返做成异步暂停点、持久化会话、并把这一切通过统一的 `Command`/`Event`
协议（§4）暴露——**但不绑定任何具体传输**（Tauri/web/ACP 都在上层）。具体：

- **传输无关的引擎 API**：`Engine` 只接受 `Command`、产生 `Event`，通过内存 channel 驱动，不知道自己
  跑在哪个 front door 后面。测试直接喂 `Command`、断言 `Event` 序列。
- **下沉到 agent 层自组 scope**（`DESIGN.md` §3.2、§9.1）：不用 facade `Agent::run/stream`，用
  `DefaultAgentMachine` + 自组 `HandlerScope` + `drain`，以获得异步审批控制权。
- **跨传输审批 gate**（§3.3）：`IpcApproval` 实现底层 async `InteractionHandler`，`fulfill` 里发
  `InteractionRequested`、await oneshot、收 `RespondInteraction` 后折回。machine 真正停在 `.await`。
- **流式**：自定义 `StreamingTapHandler`（逐 `Delta::Text` emit `TextDelta`），因为库自带
  `LlmClientHandler` 在流式模式内部聚合、不 tap delta（§9）。
- **会话生命周期**：per-session driver actor（§3.1），run 在独立 task 推进，`CancelRun`/
  `RespondInteraction` 走不碰 agent `&mut` 的旁路。
- **持久化**：committed 一致点取 `AgentState` snapshot 写 SQLite；`ResumeSession` 重装配（§3.6）。
- **插件式工具**（§7）：`ToolPlugin` registry，第一版最小集（read_file/list_dir/grep 只读 + shell 审批）。
- **充分测试**：mag-core 的每条关键路径都有**离线**测试（fake `LlmClient` + 脚本化工具/审批），
  这是本计划的重点交付物，不是附带。

## 非目标（本计划）

- 不做任何具体传输（Tauri invoke/emit、web WebSocket、ACP stdio）——那是上层，等 mag-core 稳定后再做。
- 不做前端（React/app）。
- 不做本地 agent 来源的 live 接入（external features / `ExternalSessionHandler` 生产实现）——第一版
  mag-core 先跑通 **LLM API 来源 + 本地工具**这条主干；本地 agent 来源作为 `DESIGN.md` §10 的 M2 单列。
  但 mag-core 的 source registry / 委派数据结构要为它**预留位置**，不写死。
- 不实现 AI-based permission / AI-based routing 的**决策逻辑**（§8）——只把接缝（`PermissionDecider`
  钩子位置、routing 配置字段）留在结构里。

## 现有代码锚点（mag-core 的精确接入面，全部已核对 agent-lib 源码）

- **自组 scope 完整样板**：`../agent-lib/examples/agent_chat.rs`——id source（`DemoIds`：`RequirementIds`
  + `ToolExecutionIds`，uuid from 单调计数器）、`AgentSpec`→`AgentState`→`DefaultAgentMachine`、
  自建 `HandlerScope`（`ChatScope`）、`drain(&mut machine, input, &scope, None, &ctx)`、`RunContext`。
- **审批暂停点**：`agent::InteractionHandler`（`../agent-lib/src/agent/drive.rs:155`，`#[async_trait]
  async fn fulfill(&self, req: &Interaction, ctx: &RunContext) -> RequirementResult`）。样板
  `StdinApproval`（`agent_chat.rs:212`）。`Interaction`/`InteractionKind`/`InteractionResponse`/
  `ApprovalResponse` 均 serde 友好（`src/agent/interaction.rs`、`approval.rs`）。
- **流式 tap 参考**：facade `StreamingTapHandler`（`../agent-lib/src/facade/agent/stream.rs:436`）——
  `chat_stream` + 逐 `StreamEvent::BlockDelta{delta: Delta::Text}` emit + `Accumulator` 折叠。
- **参考 handler**：库自带 `LlmClientHandler`（`src/agent/drive/reference.rs:73`，非流式或内部聚合）、
  `ToolRegistryHandler`（:122）；`drain`（`src/agent/drive.rs:369`）。
- **工具**：`agent::ToolRegistry` trait（`WeatherRegistry` 模式，`agent_chat.rs:130`）或 facade
  `Tool::function_with_schema`（`src/facade/tool.rs`，无需 feature）。`ToolContext` 带
  `worktree`/`cancel`/`tool_call_id`。
- **权限模型**：`agent::permission::{PermissionRequest, PermissionCategory, PermissionRisk,
  PermissionDecision, PermissionResponse}`（`src/agent/permission.rs`，全 serde；risk 有序）。
- **持久化**：`AgentState` snapshot / restore（committed 一致点）；`facade::ChatSession` 的
  `ConversationSnapshot` 路径可作纯对话备选参考。
- **纯对话备选（M0 脚手架）**：facade `Chat`/`ChatSession`（`src/facade/chat.rs`，无工具，tool-use 报错）。

## 里程碑

逐层增加概念、每层可独立**离线**验证。C0–C2 是引擎主干，C3 加工具+审批，C4 持久化，C5 收官验收。

| 里程碑 | 主题 | 主要产出 | 默认测试形态 |
|---|---|---|---|
| C0 | 骨架 + 协议 | workspace、`mag-protocol`（`Command`/`Event` + payload，全 serde）、`Engine` 空壳 + 内存事件总线、id source | 单元（协议 round-trip、Engine 起停） |
| C1 | 纯对话流式 | 自组 scope 驱动一次 LLM turn、`StreamingTapHandler` emit `TextDelta`、`SendMessage`→`RunFinished` | 单元（fake `LlmClient` 脚本化 delta，离线） |
| C2 | 会话生命周期 | per-session driver actor、`CreateSession`/`ListSessions`/`CancelRun`、多会话隔离、cancel 旁路 | 单元（并发会话、run 中途 cancel，离线） |
| C3 | 工具 + 交互审批 | `mag-tools` registry + 最小集、`IpcApproval` 异步暂停、`InteractionRequested`/`RespondInteraction` 往返、`ToolStarted`/`ToolFinished` | 单元（脚本化工具 + 审批 channel，验证 machine 真停到 resolve） |
| C4 | 持久化 | SQLite、committed 点 snapshot、`ResumeSession` 重装配、凭据 store（不进 snapshot） | 单元（snapshot round-trip、跨"重启"恢复历史，离线临时库） |
| C5 | 收官验收 | 端到端离线主干（建会话→多轮带工具+审批→cancel→持久化→恢复）、契约冻结 review | 集成（离线全链路）+ review |

每个里程碑末尾隐含一次自检：全部离线测试绿、无网络/凭据/CLI 依赖、契约变更向后兼容（只加 enum 变体/
`#[non_exhaustive]` 加字段）。C5 通过后方可推进 `DESIGN.md` §10 的 M1+（传输/前端/本地 agent）。

## 关键设计约束（落地时必须守）

- **引擎传输无关**：`mag-core` 不依赖 tauri / axum / ACP crate；只依赖 agent-lib、mag-protocol、
  mag-tools、mag-sources。任何"发事件"都写内存事件总线，由（未来的）传输层订阅。
- **协议独立**：`mag-protocol` 不依赖 agent-lib；需要的底层类型（`InteractionResponse` 等）薄封装或
  重定义为 mag 自己的 wire 类型，不把 agent-lib 内部类型泄漏进公开协议。
- **审批是异步暂停点，不是通知**：`IpcApproval::fulfill` 必须真正 await；决不能像 facade 那样"先 emit
  再同步决策"。测试必须断言 machine 在 resolve 前不前进。
- **run 与控制命令解耦**：actor 里 run 占用 agent `&mut`；`CancelRun`/`RespondInteraction` 只碰 cancel
  token / pending oneshot map，永不阻塞在 run 上。
- **snapshot 无 secret**：agent-lib snapshot 本就 data-only；mag 的持久层同样不存凭据/闭包/handle。
  凭据只在 `CredentialStore`，恢复时重注入。
- **扩展点留位不写死**：`PermissionDecider` 钩子（§8.1）、routing 配置字段（§8.2）、source registry 对
  本地 agent 的位置——结构里留好，第一版给保守默认（问前端 / model-routed / 无本地 agent）。
- **不绕过 agent-lib 不变量**：会话推进必须走 `Conversation`/`DefaultAgentMachine`/`Requirement`，
  不自己拼 message Vec、不重写状态机。

## 测试策略（本计划的重点）

**mag-core 的测试充分性是推进上层的前置门槛。** 默认序列（cheap → expensive）：

1. `cargo fmt --all -- --check`
2. 聚焦测试：任务给出精确过滤名（如 `cargo test -p mag-core engine::approval`）
3. `cargo clippy --all-targets -- -D warnings`
4. `cargo test --workspace`（完整套件）
5. `cargo doc --no-deps --workspace`（公开 API 带 rustdoc）

补充约束：

- **所有 mag-core 测试必须离线**：用 fake `LlmClient`（脚本化 response / delta）、脚本化工具、内存/临时
  SQLite、内存审批 channel。不依赖网络、真实凭据、CLI、本地登录态。每个测试 1 分钟内完成，卡住即 bug。
- **审批往返必须被显式测试暂停语义**：脚本化 `InteractionHandler` await test channel，断言 machine 在
  test 侧 resolve 前 future 不完成，approve/deny/cancel 三条路径都覆盖。
- **并发会话隔离**：多个 session 并行 run，事件按 session_id 正确分流，一个会话的 cancel 不影响另一个。
- **契约测试**：`Command`/`Event` 全变体 serde round-trip；若启用 `ts-rs`/`schemars`，校验 TS 类型不漂移。
- 需要真实 LLM endpoint 的端到端验证一律 `#[ignore]`，缺环境干净跳过（绿），不输出 secret。

## 风险与待确认

- **R-A 流式 tap 自建**：库自带 `LlmClientHandler` 流式模式内部聚合、不逐 delta tap（已核对
  `reference.rs`）。mag 必须自建 `StreamingTapHandler`（参考 facade `stream.rs:436`）。取向：在 mag-core
  内实现，约 50 行，换取流式 + 完全审批控制。
- **R-B facade 注入口未就绪**：当前必须下沉自组 scope（agent-lib facade 无 `interaction_handler(..)`
  注入口）。若 agent-lib Milestone 7（M7-1）先落地，mag-core 可改为留在 facade 内注入 `IpcApproval`，
  减少 id source / spec / drain 样板。取向：先按自组 scope 落地（不阻塞）；M7-1 就绪后作为化简任务回收。
- **R-C 本地 agent 来源推迟**：mag-core 主干先只做 LLM API + 本地工具。source registry 与委派结构必须
  为本地 agent 预留位置（enum 变体 / trait 对象槽），但不实现 live 接入。避免主干被 external feature
  与 CLI 环境依赖拖慢。
- **R-D 持久化 schema 演进**：SQLite schema 首版可能变。取向：snapshot 存 JSON blob（agent-lib
  `AgentState` 的 serde），schema 只存 id/config/时间戳等稳定列，降低迁移成本；预留 schema version 列。
- **R-E id source 一致性**：mag 的 `RequirementIds`+`ToolExecutionIds` 实现须保证会话内 id 唯一且恢复后
  不冲突（参考 facade `FacadeIds::continuing_after`）。取向：per-session 单调计数器，恢复时从快照记录的
  高水位续号。
