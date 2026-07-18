# TODO：mag-core 落地任务单

> 依据 [`PLAN.md`](PLAN.md) 与唯一设计输入 [`DESIGN.md`](DESIGN.md)。
> **范围：只做 mag-core（引擎）+ 直接依赖的 mag-protocol / mag-tools / mag-sources 核心。**
> 上层（mag-tauri / mag-server / mag-acp / app 前端）不在本单内——mag-core 全部任务 `[DONE]` 且 C5 验收
> 通过后，再另起任务单按 `DESIGN.md` §10 规划。

## 通用执行规则

- **一次一个任务**：每次只执行「首个标题带 `[TODO]` 的任务」。完成后把 `[TODO]` 改为 `[DONE]`，在任务末尾
  补「完成记录」，然后停止，等待下一次调用。
- **完成的定义**：只有标题带 `[DONE]` 才算完成。仅填完成记录而标题仍 `[TODO]`，按未完成处理。review 任务
  （`C<n>-R`）是真实任务，不得跳过。
- **编号**：`C<里程碑>-<序号>`；每个里程碑末尾有独立 review 任务 `C<n>-R`。
- **不绕过 agent-lib 不变量**：会话推进必须走 `Conversation`/`DefaultAgentMachine`/`Requirement`；不自己拼
  message Vec、不重写状态机（`DESIGN.md` §9）。
- **引擎传输无关**：`mag-core` 不得依赖 tauri / axum / ACP crate。发事件只写内存事件总线。
- **离线测试纪律**：所有 mag-core 测试必须离线——fake `LlmClient`、脚本化工具/审批、内存或临时 SQLite。不依赖
  网络、真实凭据、CLI、本地登录态。每个测试须 1 分钟内完成，卡住即为 bug，须立刻修。真实 endpoint e2e 一律
  `#[ignore]`，缺环境干净跳过（绿），不输出 secret。
- **不容忍 workaround / 设计偏离**：遇到 agent-lib 缺口（缺 API、类型不匹配）不得 papering over；要么按
  `DESIGN.md` §9 的下沉方案实现，要么在本文件正确依赖位置插入最小前置任务并让被阻塞任务显式依赖它，然后
  提交并停止。
- **默认完整验证序列**（任务另有放宽以任务为准）：
  1. `cargo fmt --all -- --check`
  2. 聚焦测试（任务给出精确过滤名）
  3. `cargo clippy --all-targets -- -D warnings`
  4. `cargo test --workspace`
  5. `cargo doc --no-deps --workspace`
- **公开 API 必须带 rustdoc**（各 crate 开 `#![warn(missing_docs)]`）。
- **扩展点留位不写死**：`PermissionDecider` 钩子、routing 配置字段、source registry 对本地 agent 的位置——
  结构里留好，第一版给保守默认（见 `DESIGN.md` §8、§9）。

---

## Milestone C0 — 骨架 + 协议

目标：建 workspace，落地传输无关的 `mag-protocol`（`Command`/`Event` + payload，全 serde），以及一个空壳
`Engine` + 内存事件总线 + id source。此后所有测试可直接喂 `Command`、断言 `Event`。

### [DONE] C0-1 建 workspace 骨架 + crate 划分

**上下文**：

- mag 目前是空目录（仅 `DESIGN.md`/`PLAN.md`/`TODO.md`）。目标结构见 `DESIGN.md` §2。
- 本单只建 mag-core 主干需要的 crate：`mag-protocol`、`mag-core`、`mag-tools`、`mag-sources`。传输/前端
  crate（mag-tauri/mag-server/mag-acp/app）**本单不建**。
- agent-lib 作为 path dependency：`agent-lib = { path = "../agent-lib" }`。

**做什么**：

- 建 `Cargo.toml` workspace（`resolver = "3"`，与 agent-lib 一致的 edition 取向）。
- 建四个 crate 空骨架：`crates/mag-protocol`、`crates/mag-core`、`crates/mag-tools`、`crates/mag-sources`，
  各自 `lib.rs` 开 `#![warn(missing_docs)]` + 模块级 rustdoc。依赖关系按 `DESIGN.md` §2 表：mag-core 依赖
  其余三个 + agent-lib；mag-protocol 只依赖 serde。
- 建一个占位的 `#[test]` 确保 `cargo test --workspace` 能跑通空套件。

**验证条件**：

- `cargo build --workspace` 与 `cargo test --workspace` 绿（空套件）。
- 依赖图正确：`cargo tree -p mag-protocol` 不含 agent-lib。
- 完整验证序列 1、3、4、5（无聚焦测试）。

**完成记录（2026-07-18）**：

- 建立 root Cargo workspace，使用 `resolver = "3"` 与 edition 2024。
- 新建 `crates/mag-protocol`、`crates/mag-core`、`crates/mag-tools`、`crates/mag-sources` 四个库 crate。
- 四个 crate 的 `src/lib.rs` 均开启 `#![warn(missing_docs)]` 并提供模块级 rustdoc；每个 crate 含一个占位单元测试，确保 workspace 测试可跑通。
- 依赖边界按 `DESIGN.md` §2 落地：`mag-protocol` 只依赖 `serde`；`mag-core` 依赖 `agent-lib` 与三个本地 crate；`mag-tools` 依赖 `agent-lib`；`mag-sources` 依赖 `agent-lib` 与 `keyring`。
- 补充根 `README.md`，记录当前 workspace 结构、setup 与基础验证命令。
- 验证通过：`cargo fmt --all`、`cargo fmt --all -- --check`、`cargo build --workspace`、`cargo tree -p mag-protocol`（无 `agent-lib`）、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`（30 分钟上限包装，4 个单元测试 + doctest 全绿）、`cargo doc --no-deps --workspace`。

### [DONE] C0-2 `mag-protocol`：`Command` / `Event` + payload（全 serde）

**上下文**：

- 协议形状见 `DESIGN.md` §4.1（Command）/§4.2（Event）。`serde(tag = "type", rename_all = "snake_case")`
  internally-tagged，便于未来 TS discriminated union。
- `mag-protocol` **不依赖 agent-lib**（`DESIGN.md` §9 约束）；镜像 agent-lib 的
  `InteractionResponse`/`InteractionKind`/`PermissionRequest` 为 mag 自己的 wire 类型
  （`InteractionResponseWire`/`InteractionKindWire`），字段对齐但类型独立。
- 标识类型：`SessionId`、`RequestId`、`RunId`（wrap uuid，serde）。

**做什么**：

- 定义顶层 `Command` enum：`CreateSession`/`ListSessions`/`ResumeSession`/`DeleteSession`/`SendMessage`/
  `CancelRun`/`RespondInteraction`/`ListSources`/`ProbeLocalAgents`（后二者可先留变体，body 最小）。
- 定义顶层 `Event` enum：`SessionCreated`/`RunStarted`/`RunFinished`/`RunError`/`TextDelta`/`ToolStarted`/
  `ToolFinished`/`InteractionRequested`/`DelegationStarted`/`DelegationFinished`/`DelegationFailed`/
  `LocalAgentsProbed`（委派/来源变体先定义占位，body 最小、`#[non_exhaustive]`）。
- 定义 payload：`SessionConfig`（provider/model/tool 档/routing 字段——routing 留 `model_routed` 默认，见
  §8.2）、`ToolTrace`、`InteractionKindWire`（Approval/Question/Choice/Permission，后者带 category/risk/
  summary/subject）、`InteractionResponseWire`、`SourceInfo`。全部 `Serialize + Deserialize`，关键 enum
  `#[non_exhaustive]`。

**验证条件**：

- 单元测试：每个 `Command`/`Event` 变体 `serde_json` round-trip 保真；tag 字段命名稳定。
- 聚焦：`cargo test -p mag-protocol`。
- 完整验证序列 1–5。

**完成记录（2026-07-18）**：

- 在 `mag-protocol` 中定义传输无关的顶层 `Command` / `Event` enum，使用
  `#[serde(tag = "type", rename_all = "snake_case")]`，并为未来扩展标注 `#[non_exhaustive]`。
- 增加 `SessionId`、`RequestId`、`RunId` 三个 UUID 透明包装 ID，并补充交互/工具追踪所需的独立 wire ID 类型。
- 定义 `SessionConfig`（含 provider/model/tool_profile/routing，routing 默认 `model_routed`）、`RunOutput`、
  `UsageInfo`、`ToolTrace`、`DelegationTrace`、`InteractionKindWire`、`InteractionResponseWire`、
  `SourceInfo` 等 payload；交互与权限类型镜像 agent-lib 字段语义但不依赖 agent-lib。
- 为所有公开协议类型、字段和变体补 rustdoc，保持 `#![warn(missing_docs)]` 干净。
- 添加 serde 单元测试，覆盖每个 `Command` / `Event` 变体 round-trip 与稳定 tag 断言，并覆盖交互 wire 类型、
  routing 默认值和 UUID ID 透明序列化。
- 验证通过：`cargo fmt --all`、`cargo fmt --all -- --check`、`cargo test -p mag-protocol`（5 个单元测试 +
  doctest 全绿）、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`（30 分钟上限包装，8 个单元测试 +
  doctest 全绿）、`cargo doc --no-deps --workspace`。

### [DONE] C0-3 `Engine` 空壳 + 内存事件总线 + id source

**上下文**：

- `Engine` 是引擎入口（`DESIGN.md` §3.1）：`handle_command(Command)` + 一个可订阅的 `Event` 流
  （`tokio::sync::broadcast` 或 mpsc）。本任务不接 agent-lib，只搭壳与总线，让测试能喂命令、收事件。
- id source：参考 `../agent-lib/examples/agent_chat.rs` 的 `DemoIds`——实现 agent-lib 的 `RequirementIds`
  + `ToolExecutionIds`（uuid from per-session 单调计数器，从 1 起）。放 `mag-core`，供后续 C1 装配 machine。
  须支持恢复续号（`PLAN.md` R-E），本任务先做基础版 + 预留续号入口。

**做什么**：

- `Engine`：持有 `SessionManager`（先空 map）+ `event_bus`。`handle_command` 先只处理 `CreateSession`
  （产 `SessionCreated`）/`ListSessions`，其余变体返回明确的"未实现"错误事件（`RunError`/占位）。
- `event_bus`：`subscribe() -> impl Stream<Item = Event>`；`emit(Event)` 内部广播。
- `MagIds`：实现 `RequirementIds` + `ToolExecutionIds` + 各 id 构造（`ConversationId`/`TurnId`/`MessageId`/
  `StepId`/`RunId`/`AgentId`/`ToolSetId`/`TraceNodeId`），per-session 计数器，`continuing_after(high_water)`
  续号入口（参考 facade `FacadeIds`）。

**验证条件**：

- 单元测试：`CreateSession` → 收到 `SessionCreated`；`ListSessions` 反映已建会话；多个订阅者都收到事件。
- 单元测试：`MagIds` 产的 id 唯一；`continuing_after` 从给定水位续号不回退。
- 聚焦：`cargo test -p mag-core engine::skeleton`（或相应模块名）。
- 完整验证序列 1–5。

**完成记录（2026-07-18）**：

- 在 `mag-core` 新增 `Engine` 空壳，持有内存 `SessionManager` 与 `EventBus`；`CreateSession` 生成稳定
  `SessionId`、登记会话并 emit `Event::SessionCreated`。
- `ListSessions` 通过 `CommandOutput::Sessions(Vec<SessionInfo>)` 返回当前内存会话列表；会话级未实现命令
  统一 emit `Event::RunError`，非会话级未实现命令返回 `EngineError::UnsupportedCommand`。
- 新增基于 `tokio::sync::broadcast` 的内存事件总线，`subscribe()` 返回实现 `Stream<Item = Event>` 的
  `EventStream`，多个订阅者可同时收到后续事件。
- 新增 `MagIds`，使用 per-source 共享单调计数器从 1 开始生成 UUID-backed agent-lib ID；实现
  `RequirementIds` 与 `ToolExecutionIds`，并提供 `seeded` / `continuing_after(high_water)` 续号入口及
  `ConversationId`、`TurnId`、`MessageId`、`StepId`、`RunId`、`AgentId`、`ToolSetId`、`TraceNodeId`
  等构造方法。
- 添加 `mag-core` 单元测试覆盖 `CreateSession` 事件、`ListSessions`、多订阅者广播、会话级未实现错误事件、
  `MagIds` 唯一性、clone 共享计数器、零种子钳制和高水位续号。
- 验证通过：`cargo fmt --all`、`cargo fmt --all -- --check`、`cargo test -p mag-core engine::skeleton`、
  `cargo test -p mag-core ids`、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`（30 分钟上限包装，
  15 个单元测试 + doctest 全绿）、`cargo doc --no-deps --workspace`。

### [DONE] C0-R Review：骨架 + 协议一致性

**上下文**：C0-1..C0-3 收官 review。

**做什么**：

- 核对 crate 依赖图与 `DESIGN.md` §2 一致（mag-protocol 无 agent-lib 依赖）；协议变体与 §4 一致。
- 核对 `#[non_exhaustive]` 用于会增长的 enum；关键 enum serde round-trip 全覆盖。
- 汇总遗留缺口为后续任务。

**验证条件**：完整验证序列 1–5 全绿；协议对照表（§4 承诺 vs 已实现）。

**完成记录（2026-07-18）**：

- 核对 `DESIGN.md` §2：workspace 只包含 C0 范围内的 `mag-protocol`、`mag-core`、`mag-tools`、
  `mag-sources` 四个 crate；`mag-core` 依赖 `agent-lib` 与三个本地核心 crate，未引入 tauri / axum / ACP
  front door crate。
- `cargo tree -p mag-protocol` 确认 `mag-protocol` 没有 `agent-lib` 依赖；其直接依赖仅为 wire
  payload 所需的 `serde`、`serde_json`、`uuid`。
- 核对 `DESIGN.md` §4 时发现 `Event::DelegationMessage` 尚未实现；已在 `mag-protocol` 补齐
  `Event::DelegationMessage { id, message }` 与 `DelegationMessageWire`，并加入稳定 tag/serde round-trip 测试。
- 核对会增长的公开 enum：`Command`、`Event`、`RoutingMode`、`ToolStatusWire`、
  `InteractionKindWire`、`InteractionResponseWire`、`ApprovalRequirementWire`、
  `ApprovalDecisionWire`、`PermissionCategoryWire`、`PermissionRiskWire`、
  `PermissionDecisionWire`、`SourceKindWire` 均已标注 `#[non_exhaustive]`。
- 关键 serde 覆盖：`Command` 每个变体、`Event` 每个变体、交互 request/response、routing 默认值、
  UUID-backed wire id 均有 round-trip 或稳定 tag 单元测试。

| `DESIGN.md` §4 承诺 | 已实现状态 |
|---|---|
| `Command::CreateSession { config }` | 已实现，serde tag `create_session` |
| `Command::ListSessions` | 已实现，serde tag `list_sessions` |
| `Command::ResumeSession { id }` | 已实现，serde tag `resume_session` |
| `Command::DeleteSession { id }` | 已实现，serde tag `delete_session` |
| `Command::SendMessage { session_id, text, attachments? }` | 已实现，`attachments` 默认空且空时不序列化，serde tag `send_message` |
| `Command::CancelRun { session_id }` | 已实现，serde tag `cancel_run` |
| `Command::RespondInteraction { session_id, request_id, response }` | 已实现，serde tag `respond_interaction` |
| `Command::ListSources` | 已实现，serde tag `list_sources` |
| `Command::ProbeLocalAgents` | 已实现，serde tag `probe_local_agents` |
| `Event::SessionCreated { id, config }` | 已实现，serde tag `session_created` |
| `Event::RunStarted { id, run_id }` | 已实现，serde tag `run_started` |
| `Event::RunFinished { id, output }` | 已实现，serde tag `run_finished` |
| `Event::RunError { id, message }` | 已实现，serde tag `run_error` |
| `Event::TextDelta { id, text }` | 已实现，serde tag `text_delta` |
| `Event::ToolStarted { id, trace }` | 已实现，serde tag `tool_started` |
| `Event::ToolFinished { id, trace }` | 已实现，serde tag `tool_finished` |
| `Event::InteractionRequested { id, request_id, kind }` | 已实现，serde tag `interaction_requested` |
| `Event::DelegationStarted { id, trace }` | 已实现，serde tag `delegation_started` |
| `Event::DelegationFinished { id, trace }` | 已实现，serde tag `delegation_finished` |
| `Event::DelegationFailed { id, trace }` | 已实现，serde tag `delegation_failed` |
| `Event::DelegationMessage { id, .. }` | 已实现为 `{ id, message: DelegationMessageWire }`，serde tag `delegation_message` |
| `Event::LocalAgentsProbed { available }` | 已实现，serde tag `local_agents_probed` |
| `InteractionKindWire::{Approval, Question, Choice, Permission}` | 已实现；`Permission` 含 `category`、`risk`、`summary`、`subject` |
| `InteractionResponseWire::{Approval, Answer, Choice, Permission}` | 已实现，独立 wire 类型，不依赖 `agent-lib` |
| `SessionConfig` provider/model/tool profile/routing | 已实现；`routing` 默认 `model_routed` |
| `ToolTrace` / `DelegationTrace` / `SourceInfo` | 已实现为传输无关 serde payload |

- C0 review 未发现需要插入 `TODO.md` 的遗留阻塞缺口；后续纯对话流式、会话 actor、审批、工具、持久化等能力仍按
  C1+ 既有任务推进。
- 验证通过：`cargo fmt --all`、`cargo fmt --all -- --check`、`cargo test -p mag-protocol`、
  `cargo tree -p mag-protocol`、`cargo tree -p mag-core` 依赖边界核对、
  `cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`（30 分钟上限包装，15 个单元测试 +
  doctest 全绿）、`cargo doc --no-deps --workspace`。

---

## Milestone C1 — 纯对话流式

目标：自组 `HandlerScope` + `drain` 驱动一次真实 LLM turn（用 fake client 离线），`StreamingTapHandler`
逐 delta emit `TextDelta`，`SendMessage` → `TextDelta*` → `RunFinished`。这是引擎主干的第一次贯通。

### [TODO] C1-1 fake `LlmClient` 测试夹具 + `StreamingTapHandler`

**上下文**：

- 库自带 `LlmClientHandler`（`../agent-lib/src/agent/drive/reference.rs:73`）在流式模式**内部聚合、不逐
  delta tap**（已核对）。mag 要 emit `TextDelta` 必须自建 tap handler。参考 facade `StreamingTapHandler`
  （`../agent-lib/src/facade/agent/stream.rs:436`）：`chat_stream` + 逐 `StreamEvent::BlockDelta{delta:
  Delta::Text}` emit + `Accumulator::push/finish` 折叠成 `Response`。
- 测试需要离线 fake `LlmClient`：实现 agent-lib `client::LlmClient`，按脚本吐 `StreamEvent` 序列
  （text delta + 终态）。参考 agent-lib facade 单元测试里的 fake client 模式。

**做什么**：

- 在 `mag-core`（或 dev 支持模块）实现 `StreamingTapHandler`：持有 `Arc<dyn LlmClient>` + 事件发射器，
  `impl LlmHandler`，`fold` 逐 text delta emit `Event::TextDelta`，用 `Accumulator` 折成 `Response` 回填。
- 实现一个 `FakeLlmClient`（test 夹具）：可脚本化「纯文本流」与「带 tool-use 的响应」两类结果，供 C1/C3 用。

**验证条件**：

- 单元测试：喂脚本化 text delta，`StreamingTapHandler.fold` emit 的 `TextDelta` 序列与脚本一致，且折叠出的
  `Response` 文本完整、可 commit。
- 聚焦：`cargo test -p mag-core llm::stream`。
- 完整验证序列 1–5。

### [TODO] C1-2 自组 scope 驱动一次对话 turn

**上下文**：

- 完整样板：`../agent-lib/examples/agent_chat.rs`——`AgentSpec::new(agent_id, WorktreeRef, system,
  ToolSetRef, ModelRef, LoopPolicy)` → `AgentState::new(spec, Conversation)` → `DefaultAgentMachine::new(
  state, LlmStepMode::Streaming, ids)` → 自建 `HandlerScope`（`llm`/`tool`/`interaction`）→ `drain(&mut
  machine, AgentInput::user_message(..), &scope, None, &ctx)`；`RunContext::new_root(run_id, BudgetLimits,
  trace_root)`。
- 本任务先做**无工具、无审批**的纯对话：scope 只挂 `StreamingTapHandler`（llm），tool/interaction 可空或
  空注册；`ModelRef` 用 fake client 的模型名。

**做什么**：

- 在 `mag-core` 实现 `MagScope`（`impl HandlerScope`，先只 `llm()` 返回 `StreamingTapHandler`）。
- 实现 driver 的单 turn 驱动函数：接 user text → 组 `AgentInput` → `drain` → emit `RunStarted`（turn 开始）、
  `TextDelta*`（tap）、`RunFinished`（携最终文本 + usage，从 `machine.state().conversation()` 取）。
- 把 `Engine::handle_command` 的 `SendMessage` 接到这条路径（先单会话、无 actor，同步驱动即可；actor 在 C2）。

**验证条件**：

- 单元测试：`CreateSession` + `SendMessage("hi")`（fake client 脚本化回复）→ 收到有序 `RunStarted` →
  `TextDelta*` → `RunFinished`，最终文本与脚本一致，usage 非空。
- 单元测试：多轮 `SendMessage` 在同一会话累积历史（第二轮能看到第一轮上下文——通过 fake client 断言收到的
  `ChatRequest.messages` 含首轮）。
- 聚焦：`cargo test -p mag-core engine::chat`。
- 完整验证序列 1–5。

### [TODO] C1-R Review：纯对话流式贯通

**做什么**：核对自组 scope 与 `agent_chat.rs` 模式一致、未绕过 `Conversation`/machine；`TextDelta` 语义与
`DESIGN.md` §3.4 一致；确认流式 tap 未破坏 `Accumulator` 折叠。汇总缺口。

**验证条件**：完整验证序列 1–5 全绿。

---

## Milestone C2 — 会话生命周期

目标：per-session driver actor（`DESIGN.md` §3.1），多会话隔离，`CancelRun` 走不碰 agent `&mut` 的旁路。

### [TODO] C2-1 per-session driver actor

**上下文**：

- `DESIGN.md` §3.1：每会话一个 actor，持 `mpsc<SessionCommand>` 入口；run 在 actor 内独立 task 推进；
  控制命令（cancel/respond）走旁路。用 actor 而非 Mutex，避免 run 长借用阻塞控制命令。
- agent-lib `Agent`/machine 是 `&mut self`，一个会话内 run 必须串行；actor 天然串行化会话内命令。

**做什么**：

- 实现 `SessionActor`：拥有该会话的 `DefaultAgentMachine` + `MagIds` + `RunContext` 工厂 + cancel token。
  循环收 `SessionCommand`：`SendMessage`（spawn run task，驱动 `drain`，事件写总线）、`CancelRun`（触发
  cancel token）、`RespondInteraction`（C3 用，先留钩子）。
- `SessionManager`：`CreateSession` 起一个 actor 并登记；命令按 `session_id` 路由；`DeleteSession` 停 actor。
- `RunContext` 用 agent-lib `BudgetLimits` + cancel token；cancel 时 `drain` 应观察到取消并干净收尾。

**验证条件**：

- 单元测试：两个会话并行 `SendMessage`，`TextDelta`/`RunFinished` 按各自 `session_id` 正确分流、不串。
- 单元测试：一次长 run（fake client 脚本化"慢"流）中途 `CancelRun` → run 干净终止、emit `RunError`/取消态、
  会话仍可用（后续 `SendMessage` 正常）；另一会话不受影响。
- 聚焦：`cargo test -p mag-core engine::session`。
- 完整验证序列 1–5。

### [TODO] C2-R Review：会话隔离与 cancel 正确性

**做什么**：核对 actor 模型无死锁/饿死（cancel 不被 run 阻塞，§3.1）；多会话事件分流正确；会话状态在 run
失败/取消后一致。汇总缺口。

**验证条件**：完整验证序列 1–5 全绿；并发/取消用例覆盖。

---

## Milestone C3 — 工具 + 交互审批

目标：插件式工具 registry + 最小集，`IpcApproval` 异步暂停，`InteractionRequested`/`RespondInteraction`
往返。**这是 mag-core 最关键、最需充分测试的部分（`DESIGN.md` §3.3、§9.1）。**

### [TODO] C3-1 `mag-tools`：`ToolPlugin` registry + 最小工具集

**上下文**：

- `DESIGN.md` §7：`ToolPlugin` trait（`declaration()`→agent-lib `Tool`；`invoke(ctx, args)`；
  `permission()`→category/risk）。registry 产 `ToolSetRef`（declarations）+ 一个 `agent::ToolRegistry`
  实现（`execute` 按 name dispatch）。参考 `agent_chat.rs:130` 的 `WeatherRegistry` 与 facade
  `Tool::function_with_schema`（`../agent-lib/src/facade/tool.rs`）。
- `ToolContext`（agent-lib）带 `worktree`/`cancel`/`tool_call_id`：shell 用 cancel 支持中断、worktree 约束路径。
- 最小集：`read_file`/`list_dir`/`grep`（只读，permission=None/auto）+ `shell`（permission=Shell/risk 按命令）。

**做什么**：

- 定义 `ToolPlugin` trait + `ToolRegistry`（mag 侧）收集 plugin → 实现 agent-lib `agent::ToolRegistry`
  （`declarations`/`execute`）。
- 实现四个内置工具 plugin（read_file/list_dir/grep/shell），fs 操作用 `ToolContext.worktree` 约束、
  shell 用 `ToolContext.cancel` 可中断。
- registry 暴露构造 `ToolSetRef`（供 C3-2 注入 `AgentSpec`）。

**验证条件**：

- 单元测试（离线，临时目录）：`read_file`/`list_dir`/`grep` 在临时目录返回正确结果；`shell` 跑简单命令
  返回 stdout；`shell` 的 cancel token 触发时中断。
- 单元测试：registry `declarations()` 含四工具；`execute` 未知工具报 `UnknownTool`。
- 聚焦：`cargo test -p mag-tools`。
- 完整验证序列 1–5。

### [TODO] C3-2 `IpcApproval`：跨传输异步审批暂停点

**上下文**：

- **本单最关键任务**。`DESIGN.md` §3.3、§9.1：实现底层 async `InteractionHandler`
  （`../agent-lib/src/agent/drive.rs:155`），`fulfill` 里发 `Event::InteractionRequested{request_id}` →
  `await oneshot` → 收前端 `RespondInteraction` → 返回 `RequirementResult::Interaction`。machine **真正停
  在 `.await`**（对比 facade「先 emit 再同步决策」是错的，§9）。样板 `StdinApproval`（`agent_chat.rs:212`）。
- 逐变体映射 `Interaction.kind`（Approval/Question/Choice/Permission）↔ `InteractionKindWire` /
  `InteractionResponseWire`。`Interaction`/`InteractionResponse`/`ApprovalResponse` 全 serde。
- `Permission` 分支预留 `PermissionDecider` 钩子（§8.1）：第一版默认"问前端"（发 InteractionRequested）；
  规则/LLM decider 留待后续，本任务只留 trait 调用点 + 默认实现。
- cancel：`fulfill` 的 await 用 `select!` 配合 session cancel token，取消时返回 deny/cancel。

**做什么**：

- 实现 `IpcApproval`（`impl InteractionHandler`）：`pending: Mutex<HashMap<RequestId, oneshot::Sender<
  InteractionResponse>>>`，`fulfill` 注册 pending + emit + await（含 cancel select）。
- 在 `SessionActor` 接 `RespondInteraction`：取 pending sender → `send(response)` → 唤醒 driver。
- 定义 `PermissionDecider` trait + 默认 `AskFrontendDecider`（对 `InteractionKind::Permission` 走 emit+await），
  在 `IpcApproval` 的 Permission 分支调用它。

**验证条件**：

- 单元测试（**核心**）：脚本化驱动一个需审批的工具 turn；断言 machine 在 test 侧 `RespondInteraction` 送达
  **之前** driver future **不完成**（用 `tokio::time` 或 poll 断言暂停）；送达 approve 后工具执行、送达 deny 后
  工具被拒并回灌模型、送达期间 cancel 则干净终止。三条路径全覆盖。
- 单元测试：`Interaction.kind` ↔ wire 类型双向映射保真（Approval/Question/Choice/Permission）。
- 单元测试：`Permission` 走默认 decider 时正确 emit `InteractionRequested` 且 await。
- 聚焦：`cargo test -p mag-core engine::approval`。
- 完整验证序列 1–5。

### [TODO] C3-3 工具事件 + 审批接入完整 turn

**上下文**：

- 把 C3-1 registry、C3-2 `IpcApproval` 接进 `MagScope`（`tool()`/`interaction()`），组成带工具+审批的完整
  scope。工具执行 bracket emit `ToolStarted`/`ToolFinished`（参考 facade tap 模式，`stream.rs`）。
- 审批策略：用 agent-lib `ToolApprovalPolicy`（`RequireApproval` 模式，`agent_chat.rs:190`）决定哪些工具
  需审批——read/grep auto、shell 需审批。

**做什么**：

- `MagScope` 补 `tool()`（bracket emit ToolStarted/Finished）+ `interaction()`（`IpcApproval`）。
- 实现 `ToolApprovalPolicy`：按 plugin 的 `permission()` 元数据决定 `ApprovalRequirement`。
- `Engine`/actor 把带工具的 `SendMessage` 走完整 scope。

**验证条件**：

- 单元测试（端到端离线）：fake client 脚本化"调用 shell 工具"的响应 → 收到 `InteractionRequested` →
  test 回 approve → 收到 `ToolStarted`/`ToolFinished` → 工具结果回灌 → `RunFinished`。deny 路径亦覆盖。
- 单元测试：read_file（auto-allow）不触发 `InteractionRequested`，直接 `ToolStarted`/`Finished`。
- 聚焦：`cargo test -p mag-core engine::tool_turn`。
- 完整验证序列 1–5。

### [TODO] C3-R Review：工具 + 审批正确性（重点）

**做什么**：**重点核对审批暂停语义**（machine 真停到 resolve，非 facade 式先 emit 再同步决策）；工具
worktree/cancel 约束生效；`PermissionDecider` 钩子留位正确（§8.1）；auto/ask 策略与 plugin 元数据一致。
汇总缺口。

**验证条件**：完整验证序列 1–5 全绿；审批三路径（approve/deny/cancel）+ auto/ask 均有测试。

---

## Milestone C4 — 持久化

目标：SQLite，committed 一致点取 `AgentState` snapshot，`ResumeSession` 重装配；凭据 store 不进 snapshot。

### [TODO] C4-1 持久化层 + snapshot/restore

**上下文**：

- `DESIGN.md` §3.6：SQLite 表 `sessions`/`snapshots`/`messages`；snapshot 只在 committed 一致点取
  （agent-lib `AgentState` snapshot 约束，run 中途不可 snapshot）。恢复重装配 machine+scope（client/工具/
  审批 handler 不在 snapshot 里，须重建）。
- `PLAN.md` R-D：snapshot 存 JSON blob（agent-lib `AgentState` serde），schema 只存稳定列 + version 列。
- `PLAN.md` R-E：恢复时 `MagIds` 从快照记录的高水位续号（`continuing_after`）。

**做什么**：

- 实现持久层（`rusqlite`/`sqlx`）：建表、`save_session`/`save_snapshot`/`load_snapshot`/`list_sessions`/
  `delete_session`。snapshot 存 agent-lib `AgentState`（或会话 `ConversationSnapshot`）的 JSON。
- actor 在每次 run 成功结束（committed）后取快照写库；记录 id 高水位。
- `ResumeSession`：读快照 → 重装配 machine（重注入 fake/real client、工具 registry、`IpcApproval`）→
  `MagIds::continuing_after(高水位)` → 会话可继续。

**验证条件**：

- 单元测试（临时 SQLite）：run 后 snapshot 写库；`load_snapshot` round-trip 与内存态一致。
- 单元测试（跨"重启"）：会话 A 跑两轮 → 取快照 → 丢弃内存 Engine → 新 Engine `ResumeSession(A)` → 历史
  可见、第三轮 `SendMessage` 能看到前两轮上下文；id 不冲突。
- 单元测试：snapshot JSON 不含任何凭据/secret（断言）。
- 聚焦：`cargo test -p mag-core persist`。
- 完整验证序列 1–5。

### [TODO] C4-2 凭据存储（`mag-sources`）

**上下文**：

- `DESIGN.md` §3.5：`CredentialStore` trait，优先 OS keyring（`keyring` crate），回退加密文件。凭据绝不进
  snapshot；恢复时重注入 `ProviderConfig`。
- `mag-sources` 也承载 provider 配置构造（Anthropic/OpenAI，agent-lib `ProviderConfig`）与 source registry
  （为本地 agent 预留位置，`PLAN.md` R-C，本任务不实现 live 接入）。

**做什么**：

- 定义 `CredentialStore` trait + 内存实现（测试用）+ keyring 实现（生产，`#[cfg]`/feature 隔离，测试不依赖真
  keyring）。
- `mag-sources` 提供 `provider_config(source_id, creds) -> agent-lib ProviderConfig`。
- source registry：`register_llm(..)` + 预留 `register_local_agent(..)` 占位（返回未实现/留 trait 槽）。

**验证条件**：

- 单元测试：内存 `CredentialStore` 存取；`provider_config` 从 store 取凭据组 `ProviderConfig`（不打印 secret）。
- 单元测试：source registry 列出已注册 LLM 来源；本地 agent 槽存在但明确未实现。
- 聚焦：`cargo test -p mag-sources`。
- 完整验证序列 1–5。

### [TODO] C4-R Review：持久化与凭据安全

**做什么**：核对 snapshot 无 secret（§9.5）、只在 committed 点取；恢复重装配正确、id 续号无冲突；凭据只在
store、恢复重注入。汇总缺口。

**验证条件**：完整验证序列 1–5 全绿；snapshot-no-secret 断言存在。

---

## Milestone C5 — 收官验收

### [TODO] C5-1 端到端离线主干集成测试

**上下文**：把 C1–C4 串成一条离线全链路，作为 mag-core 稳定的证据，也是推进上层的前置门槛。

**做什么**：

- 写集成测试（`crates/mag-core/tests/`，fake client + 脚本化工具 + 临时 SQLite + 内存审批 channel）：
  建会话 → 多轮对话（含流式）→ 调 read 工具（auto）→ 调 shell 工具（审批 approve）→ 一次 deny → run 中途
  cancel → 取快照 → 新 Engine 恢复 → 继续对话。全程断言事件序列与状态。
- 覆盖并发多会话隔离。

**验证条件**：

- 集成测试全绿，离线、无网络/凭据/CLI 依赖，1 分钟内完成。
- 聚焦：`cargo test -p mag-core --test e2e_offline`。
- 完整验证序列 1–5。

### [TODO] C5-R Review：mag-core 整体验收 + 契约冻结

**上下文**：mag-core 全部里程碑收官 review，决定是否放行上层（`DESIGN.md` §10 M1+）。

**做什么**：

- 逐条对照 `DESIGN.md` §3（引擎）/§4（协议）/§9（agent-lib 约束）：引擎传输无关、审批异步暂停、run/控制
  解耦、snapshot 无 secret、扩展点留位（§8）——是否全部满足。
- 冻结 `Command`/`Event` 契约（后续只加变体/字段，不改既有语义）；若启用 TS codegen，产出并校验前端类型。
- 汇总所有里程碑遗留缺口；确认无未调度失败测试；记录哪些 R（风险）已消解、哪些转为上层任务
  （如 R-B facade 注入口回收、R-C 本地 agent 接入 = §10 M2）。
- **放行判据**：C0–C5 全 `[DONE]`、完整验证序列全绿、端到端离线主干测试稳定通过。满足后方可另起上层任务单。

**验证条件**：完整验证序列 1–5 全绿；`DESIGN.md` §3/§4/§9 逐条对照表；契约冻结说明。
