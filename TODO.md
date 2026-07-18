# TODO：mag-core 落地任务单

> 依据 [`PLAN.md`](PLAN.md) 与唯一设计输入 [`docs/DESIGN.md`](docs/DESIGN.md)。
> **范围：只做 mag-core（引擎）+ 直接依赖的 mag-protocol / mag-tools / mag-sources 核心。**
> 上层（mag-tauri / mag-server / mag-acp / app 前端）不在本单内——mag-core 全部任务 `[DONE]` 且 C5 验收
> 通过后，再另起任务单按 `docs/DESIGN.md` §10 规划。

## 通用执行规则

- **一次一个任务**：每次只执行「首个标题带 `[TODO]` 的任务」。完成后把 `[TODO]` 改为 `[DONE]`，在任务末尾
  补「完成记录」，然后停止，等待下一次调用。
- **完成的定义**：只有标题带 `[DONE]` 才算完成。仅填完成记录而标题仍 `[TODO]`，按未完成处理。review 任务
  （`C<n>-R`）是真实任务，不得跳过。
- **编号**：`C<里程碑>-<序号>`；每个里程碑末尾有独立 review 任务 `C<n>-R`。
- **不绕过 agent-lib 不变量**：会话推进必须走 `Conversation`/`DefaultAgentMachine`/`Requirement`；不自己拼
  message Vec、不重写状态机（`docs/DESIGN.md` §9）。
- **引擎传输无关**：`mag-core` 不得依赖 tauri / axum / ACP crate。发事件只写内存事件总线。
- **离线测试纪律**：所有 mag-core 测试必须离线——fake `LlmClient`、脚本化工具/审批、内存或临时 SQLite。不依赖
  网络、真实凭据、CLI、本地登录态。每个测试须 1 分钟内完成，卡住即为 bug，须立刻修。真实 endpoint e2e 一律
  `#[ignore]`，缺环境干净跳过（绿），不输出 secret。
- **不容忍 workaround / 设计偏离**：遇到 agent-lib 缺口（缺 API、类型不匹配）不得 papering over；要么按
  `docs/DESIGN.md` §9 的下沉方案实现，要么在本文件正确依赖位置插入最小前置任务并让被阻塞任务显式依赖它，然后
  提交并停止。
- **默认完整验证序列**（任务另有放宽以任务为准）：
  1. `cargo fmt --all -- --check`
  2. 聚焦测试（任务给出精确过滤名）
  3. `cargo clippy --all-targets -- -D warnings`
  4. `cargo test --workspace`
  5. `cargo doc --no-deps --workspace`
- **公开 API 必须带 rustdoc**（各 crate 开 `#![warn(missing_docs)]`）。
- **扩展点留位不写死**：`PermissionDecider` 钩子、routing 配置字段、source registry 对本地 agent 的位置——
  结构里留好，第一版给保守默认（见 `docs/DESIGN.md` §8、§9）。

---

## Milestone C0 — 骨架 + 协议

目标：建 workspace，落地传输无关的 `mag-protocol`（`Command`/`Event` + payload，全 serde），以及一个空壳
`Engine` + 内存事件总线 + id source。此后所有测试可直接喂 `Command`、断言 `Event`。

### [DONE] C0-1 建 workspace 骨架 + crate 划分

**上下文**：

- mag 目前是空目录（仅 `docs/DESIGN.md`/`PLAN.md`/`TODO.md`）。目标结构见 `docs/DESIGN.md` §2。
- 本单只建 mag-core 主干需要的 crate：`mag-protocol`、`mag-core`、`mag-tools`、`mag-sources`。传输/前端
  crate（mag-tauri/mag-server/mag-acp/app）**本单不建**。
- agent-lib 作为 path dependency：`agent-lib = { path = "../agent-lib" }`。

**做什么**：

- 建 `Cargo.toml` workspace（`resolver = "3"`，与 agent-lib 一致的 edition 取向）。
- 建四个 crate 空骨架：`crates/mag-protocol`、`crates/mag-core`、`crates/mag-tools`、`crates/mag-sources`，
  各自 `lib.rs` 开 `#![warn(missing_docs)]` + 模块级 rustdoc。依赖关系按 `docs/DESIGN.md` §2 表：mag-core 依赖
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
- 依赖边界按 `docs/DESIGN.md` §2 落地：`mag-protocol` 只依赖 `serde`；`mag-core` 依赖 `agent-lib` 与三个本地 crate；`mag-tools` 依赖 `agent-lib`；`mag-sources` 依赖 `agent-lib` 与 `keyring`。
- 补充根 `README.md`，记录当前 workspace 结构、setup 与基础验证命令。
- 验证通过：`cargo fmt --all`、`cargo fmt --all -- --check`、`cargo build --workspace`、`cargo tree -p mag-protocol`（无 `agent-lib`）、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`（30 分钟上限包装，4 个单元测试 + doctest 全绿）、`cargo doc --no-deps --workspace`。

### [DONE] C0-2 `mag-protocol`：`Command` / `Event` + payload（全 serde）

**上下文**：

- 协议形状见 `docs/DESIGN.md` §4.1（Command）/§4.2（Event）。`serde(tag = "type", rename_all = "snake_case")`
  internally-tagged，便于未来 TS discriminated union。
- `mag-protocol` **不依赖 agent-lib**（`docs/DESIGN.md` §9 约束）；镜像 agent-lib 的
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

- `Engine` 是引擎入口（`docs/DESIGN.md` §3.1）：`handle_command(Command)` + 一个可订阅的 `Event` 流
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

- 核对 crate 依赖图与 `docs/DESIGN.md` §2 一致（mag-protocol 无 agent-lib 依赖）；协议变体与 §4 一致。
- 核对 `#[non_exhaustive]` 用于会增长的 enum；关键 enum serde round-trip 全覆盖。
- 汇总遗留缺口为后续任务。

**验证条件**：完整验证序列 1–5 全绿；协议对照表（§4 承诺 vs 已实现）。

**完成记录（2026-07-18）**：

- 核对 `docs/DESIGN.md` §2：workspace 只包含 C0 范围内的 `mag-protocol`、`mag-core`、`mag-tools`、
  `mag-sources` 四个 crate；`mag-core` 依赖 `agent-lib` 与三个本地核心 crate，未引入 tauri / axum / ACP
  front door crate。
- `cargo tree -p mag-protocol` 确认 `mag-protocol` 没有 `agent-lib` 依赖；其直接依赖仅为 wire
  payload 所需的 `serde`、`serde_json`、`uuid`。
- 核对 `docs/DESIGN.md` §4 时发现 `Event::DelegationMessage` 尚未实现；已在 `mag-protocol` 补齐
  `Event::DelegationMessage { id, message }` 与 `DelegationMessageWire`，并加入稳定 tag/serde round-trip 测试。
- 核对会增长的公开 enum：`Command`、`Event`、`RoutingMode`、`ToolStatusWire`、
  `InteractionKindWire`、`InteractionResponseWire`、`ApprovalRequirementWire`、
  `ApprovalDecisionWire`、`PermissionCategoryWire`、`PermissionRiskWire`、
  `PermissionDecisionWire`、`SourceKindWire` 均已标注 `#[non_exhaustive]`。
- 关键 serde 覆盖：`Command` 每个变体、`Event` 每个变体、交互 request/response、routing 默认值、
  UUID-backed wire id 均有 round-trip 或稳定 tag 单元测试。

| `docs/DESIGN.md` §4 承诺 | 已实现状态 |
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

### [DONE] C1-1 fake `LlmClient` 测试夹具 + `StreamingTapHandler`

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

**完成记录（2026-07-18）**：

- 在 `mag-core` 新增 `llm` 模块并公开导出 `StreamingTapHandler`；handler 持有 `Arc<dyn LlmClient>`、
  `EventBus` 与 `SessionId`，实现 agent-lib `LlmHandler`。
- `StreamingTapHandler::fold` 使用 agent-lib `Accumulator` 折叠 `StreamEvent` 为完整 `Response`；遇到
  `StreamEvent::BlockDelta { delta: Delta::Text(..) }` 时逐片段 emit `Event::TextDelta { id, text }`。
- `LlmHandler::fulfill` 强制将请求切到 streaming 路径并调用 `chat_stream`，保持 machine 返回路径仍是
  `RequirementResult::Llm(Result<Response, ClientError>)`。
- 增加 `FakeLlmClient` 测试夹具，支持 raw script、纯文本流和 tool-use 响应脚本；同时记录 `chat` 与
  `chat_stream` 请求，供当前和后续 driver 测试复用。
- 添加 `llm::stream` 单元测试，覆盖 text delta 事件序列、完整文本 `Response` 折叠、fulfill 强制流式请求，以及
  fake client 的 tool-use 脚本响应。
- 验证通过：`cargo fmt --all`、`cargo fmt --all -- --check`、`cargo test -p mag-core llm::stream`、
  `cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`（1800 秒超时包装，18 个单元测试 +
  doctest 全绿）、`cargo doc --no-deps --workspace`。

### [DONE] C1-2 自组 scope 驱动一次对话 turn

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

**完成记录（2026-07-18）**：

- 新增 `mag-core` driver 模块，按 `agent_chat.rs` 模式自组 `AgentSpec`、`AgentState`、
  `DefaultAgentMachine`、`RunContext` 与 `drain`，会话内机器保持跨轮复用，未绕过 `Conversation`/machine。
- 实现 `MagScope`，当前只挂 `StreamingTapHandler` 作为 LLM handler；无工具、无审批路径保持未接入，留给 C2/C3。
- `SessionDriver::send_message` 为每轮生成 run id，emit `RunStarted`，由 tap handler 逐片段 emit
  `TextDelta`，drain 完成后从 `machine.state().conversation()` 的最后提交 turn 提取最终 assistant 文本与 usage，
  emit `RunFinished`。
- `Engine` 增加 `with_llm_client(Arc<dyn LlmClient>)` 注入点；`CreateSession` 建立 per-session driver；
  `SendMessage` 路由到该 driver。未配置 LLM client 或未知 session 时按会话命令语义 emit `RunError`。
- 添加 `engine::chat` 单元测试，覆盖 `CreateSession` + `SendMessage("hi")` 的有序
  `RunStarted` → `TextDelta*` → `RunFinished` 事件、最终文本与非空 usage，并覆盖同一会话多轮历史累积
  （第二轮 fake client 请求包含第一轮 user/assistant 上下文）。
- 验证通过：`cargo fmt --all`、`cargo fmt --all -- --check`、`cargo test -p mag-core engine::chat`、
  `cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`（1800 秒上限包装，20 个单元测试 +
  doctest 全绿）、`cargo doc --no-deps --workspace`。

### [DONE] C1-3 切换到 facade `Agent` 注入路径（替换 C1-1/C1-2 的自组 scope）

**上下文**：

- **架构变更**：agent-lib **Milestone 7 已落地**（`AgentBuilder::interaction_handler(..)` 注入口、
  `WireRunEvent` 序列化投影、`default_external_session_handler` 等，全部 `[DONE]`）。这使 mag **不再需要**
  下沉自组 `HandlerScope`/`drain`——原因见 `docs/DESIGN.md` §3.2 与 `PLAN.md` R-A/R-B。C1-1（自建
  `StreamingTapHandler`）与 C1-2（自组 `MagScope`+`drain`+`AgentSpec` 装配）是 M7 之前的实现，本任务把它们
  切到 facade 路径。M7 之后 agent-lib 又完成一轮 refinement（M1–M6 + **M7-F1**，全 `[DONE]`）：M7-F1 补齐
  `AgentRestoreBuilder::interaction_handler(..)`（消解 R-B）；M1 流式 drop 自动 abandon、M2 非流式
  `RunOutput.events` 含 `ApprovalRequested`、M3 协作原语 snapshot/restore 真正保存数据——巩固了 mag 依赖的
  一致点/可恢复契约。
- 目标接入面（`docs/DESIGN.md` §9、`PLAN.md` 锚点，已核对 agent-lib 源码）：
  - `facade::Agent::builder().provider(..).model(..).build()`（纯对话；带工具/审批见 C3）。
  - `Agent::stream(input) -> AgentRunStream`：逐 `RunEvent` 消费；`RunEvent::TextDelta` 直接产出。
  - `RunEvent::to_wire() -> WireRunEvent`（`../agent-lib/src/facade/run.rs:329`）序列化，再映射进 mag
    `Event`；`RawStream`/`RawNotification` → `WireRunEvent::Raw`。
  - id 由 facade 内建 `FacadeIds` 管理（`MagIds` 降级为可选/仅测试，评估去留）。

**做什么**：

- 重写 `SessionDriver`：用 facade `Agent`（`Agent::builder()...build()` + `agent.stream(input)`）替换
  `DefaultAgentMachine`+`MagScope`+`drain` 装配。逐 `RunEvent` → `to_wire()` → mag `Event`（`RunStarted`/
  `TextDelta`/`RunFinished`），语义与现有事件序列保持一致。
- 移除自建 `StreamingTapHandler`（C1-1）——facade `Agent::stream` 已产 `TextDelta`。`FakeLlmClient` 夹具
  保留（仍用于离线测试，经 `AgentBuilder::client(..)` 或等价注入）。
- `Engine::with_llm_client` 注入点保留/调整为向 facade `Agent` 提供 client/provider。
- 评估 `MagIds`（C0-3）去留：facade 路径下默认 `FacadeIds`；若无确定性测试需求，`MagIds` 可移到 test-only
  或删除，在完成记录中说明决定。

**验证条件**：

- C1-2 的两个既有单元测试（`CreateSession`+`SendMessage` 的有序 `RunStarted`→`TextDelta*`→`RunFinished`；
  多轮历史累积）**在 facade 路径下继续通过**，事件序列不回退。
- 单元测试：`RunEvent::to_wire()` 映射覆盖已产出的变体；`WireRunEvent` round-trip（可复用 mag-protocol 或
  在 mag-core 断言）。
- 聚焦：`cargo test -p mag-core engine::chat`。
- 完整验证序列 1–5。删除的自组 scope 代码不留死代码（clippy 干净）。

**完成记录（2026-07-18）**：

- 重写 `mag-core` `driver.rs`：`SessionDriver` 现持有 facade `Agent`（`Agent::builder().client(..)
  .model(..).max_tokens(..).max_steps(..).build()`），`send_message` 改为消费 `agent.stream(text)` 产出的
  `RunEvent`，逐个经官方 `RunEvent::to_wire() -> WireRunEvent` 投影后映射进 mag `Event`
  （`TextDelta` → `Event::TextDelta`；终态 `Done(WireRunOutput)` 折叠成 mag `RunOutput` 后 emit
  `RunFinished`）。会话内 `Agent` 跨轮复用，历史由 facade `Conversation` 自然累积。
- 删除自建 `StreamingTapHandler`（C1-1）——facade `Agent::stream` 内部已产 `TextDelta`；同时删除
  `MagScope`+`DefaultAgentMachine`+`drain` 自组装配（C1-2）。`FakeLlmClient` 夹具保留，迁到独立
  `#[cfg(test)] mod test_support`，经 `AgentBuilder::client(..)` 注入。
- `MagIds`（C0-3）去留决定：**删除** `ids.rs` 与 `pub use ids::MagIds`。facade 内建 `FacadeIds` 接管全部身份
  铸造，`MagIds` 的 `RequirementIds`/`ToolExecutionIds` 实现已成自组 scope 残留死代码；mag 仅为
  `RunStarted` 信封保留一个 per-session run-id 计数器（facade 不外露 run id）。若 C4 恢复需要确定性
  high-water 续号，再按 facade snapshot/restore 语义单独引入。
- `Engine`/`SessionManager`：driver 改为延迟构建（首个 `SendMessage` 时按注入的 client 构建并缓存于
  `Arc<Mutex<Option<SessionDriver>>>`），保持 `Engine::new()`（无 client）仍能创建/列出会话；`with_llm_client`
  注入点保留，向 facade `Agent` 提供 client。
- 测试：C1-2 两个既有单元测试（有序 `RunStarted`→`TextDelta*`→`RunFinished`、多轮历史累积、fake client
  收到 `stream=true` 且第二轮含首轮上下文）在 facade 路径下继续通过；新增 `driver::tests` 覆盖
  `RunEvent::to_wire()` 对 `TextDelta` / `Done` 的映射与 `WireRunEvent` serde round-trip；`test_support`
  自测覆盖 fake client 的 tool-use 脚本响应。
- 验证通过：`cargo fmt --all -- --check`、`cargo test -p mag-core engine::chat`（含 driver 聚焦）、
  `cargo clippy --all-targets -- -D warnings`（无死代码/警告）、`cargo test --workspace`（1800 秒上限包装，
  mag-core 9 + mag-protocol 5 + mag-sources 1 + mag-tools 1，doctest 全绿）、`cargo doc --no-deps --workspace`。

### [DONE] C1-R Review：纯对话流式贯通（facade 路径）

**做什么**：核对 mag-core 已切到 facade `Agent`+`Agent::stream`（C1-3）、未绕过 `Conversation`/machine（facade
内部仍走）；`TextDelta` 语义与 `docs/DESIGN.md` §3.4 一致；`WireRunEvent` 映射无丢事件；确认自组 scope 残留代码
已清理。汇总缺口。

**验证条件**：完整验证序列 1–5 全绿。

**完成记录（2026-07-18）**：

- **facade 路径核对**：`crates/mag-core/src/driver.rs` 的 `SessionDriver` 持有 facade `Agent`
  （`Agent::builder().client(..).model(..).max_tokens(..).max_steps(..).build()`），`send_message` 消费
  `agent.stream(text)` 产出的 `RunEvent`，逐个经官方 `RunEvent::to_wire() -> WireRunEvent` 投影再映射进 mag
  `Event`。确认已切到 C1-3 目标接入面，无自组 `DefaultAgentMachine`/`HandlerScope`/`drain` 装配。
- **未绕过 `Conversation`/machine**：driver 不自拼 message `Vec`、不重写状态机；多轮历史由 facade 内部
  `Conversation` 自然累积（`chat::send_message_accumulates_history_in_one_session` 断言第二轮请求含首轮
  user/assistant 消息）。符合 `docs/DESIGN.md` §9 不变量。
- **`TextDelta` 语义与 §3.4 一致**：§3.4 要求「driver 逐 `RunEvent` → `to_wire()` → 映射进 mag `Event` 写
  `event_bus`」。`map_wire_event` 将 `WireRunEvent::TextDelta(text)` 直接映射为 `Event::TextDelta { id, text }`，
  终态 `WireRunEvent::Done` 折叠成 `RunOutput` 后由 driver emit `RunFinished`，事件序列
  `RunStarted → TextDelta* → RunFinished` 与 §3.4/§4.2 一致，未回退。
- **`WireRunEvent` 映射无丢事件**：纯对话路径仅产出 `TextDelta` 与终态 `Done`，两者均被 `map_wire_event` 覆盖，
  无遗漏；`driver::tests` 覆盖 `TextDelta`/`Done` 映射与 `WireRunEvent` serde round-trip。`Tool`/`Approval`/
  `Delegation`/`Raw` 变体在 C1 纯对话不产出，代码以显式注释延后到 C3+（`ToolStarted`/`ToolFinished` 见 C3-1/
  C3-3、`InteractionRequested` 见 C3-2、委派见后续里程碑），非丢事件而是已排期的前向缺口。
- **自组 scope 残留清理**：确认已无 `StreamingTapHandler`（C1-1）、`MagScope`/`DefaultAgentMachine`/`drain`
  自组装配（C1-2）、`ids.rs`/`MagIds`（C0-3 残留）、`llm.rs`；全仓 grep 仅在 `driver.rs` 文档注释中提及这些已
  下沉到 facade。clippy `-D warnings` 无死代码/未用符号告警。
- **缺口汇总**：C1-R 未发现需插入 `TODO.md` 的新前置/阻塞缺口。前向未覆盖的 `WireRunEvent` 变体
  （工具/审批/委派/Raw）映射均已由 C3+ 既有任务安排；持久化恢复的 `interaction_handler` 注入口缺口已在
  `docs/DESIGN.md` §3.6 记录并由 C4-1 依赖跟踪。
- 验证通过（完整序列 1–5）：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`（干净）、
  `cargo test --workspace`（mag-core 9 + mag-protocol 5 + mag-sources 1 + mag-tools 1，doctest 全绿）、
  `cargo doc --no-deps --workspace`。C1-R 仅改动文档（`TODO.md`/`memory`），编译产物自 C1-3 绿以来未变。

---

## Milestone CS — 抽取 `mag-service` 抽象接口

目标：把 service 的"外观"从 mag-core 内部提升为独立的 `mag-service` crate（`MagService` trait + service
protocol 中立类型），`mag-core::Engine` 实现该 trait。原 `mag-protocol` crate 并入 `mag-service`。**此后
C2–C5 直接对着 `MagService` trait 写、测试经 trait 调用。** 设计依据 `docs/DESIGN.md` §1/§2/§3.0/§4。

> 顺序说明：CS 在 C1（含 C1-3 切 facade、C1-R）之后、C2 之前执行——此时 Engine 表面积最小，抽接口最便宜。
> 本 milestone 会重构 C0-2 建立的 `mag-protocol`（迁移类型 + 删 crate），但不改任何 `[DONE]` 任务的记录。

### [DONE] CS-1 新建 `mag-service` crate + 迁移 Command/Event（并入 mag-protocol）

**上下文**：

- `docs/DESIGN.md` §2：`mag-protocol` 并入 `mag-service`，不再单独存在；`mag-service` 承载 `MagService` trait
  与全部 service protocol 中立类型（Command/Event/SessionConfig/交互·工具·权限 wire 类型），**不依赖
  agent-lib**，仅 serde/futures。
- C0-2 已在 `mag-protocol` 实现 `Command`/`Event` + payload（全 serde，`[DONE]`）——类型可整体复用，仅换
  crate 归属。

**做什么**：

- 新建 `crates/mag-service`，把 `mag-protocol` 的 `Command`/`Event`/payload/ID 类型迁入（保持 serde 契约与
  tag 命名不变）。删除 `crates/mag-protocol`，更新 workspace 成员与所有 `use` 路径。
- 调整依赖：`mag-core` 依赖 `mag-service`（替换对 `mag-protocol` 的依赖）。
- 保持 `mag-service` 不依赖 agent-lib（`cargo tree -p mag-service` 不含 agent-lib）。

**验证条件**：

- 迁移后既有协议 round-trip 测试全绿（从 mag-protocol 移到 mag-service）。
- `cargo tree -p mag-service` 不含 agent-lib；workspace 无 `mag-protocol` 残留。
- 完整验证序列 1–5。

**完成记录**：

- `git mv crates/mag-protocol crates/mag-service` 整体迁移，保留文件历史；协议类型（`Command`/`Event`/
  payload/ID 类型、`define_id!` 生成的 6 个 ID、`SessionConfig`/各 wire 枚举/`SourceInfo` 等）与 serde
  契约、tag 命名逐字不变；协议 round-trip 测试（`mod tests`，5 个用例）随文件迁入 mag-service。
- crate 改名：`mag-service/Cargo.toml` `name = "mag-service"`（依赖仍仅 serde/serde_json/uuid，不含
  agent-lib）；crate 级 rustdoc 更新为「service protocol 载体，后续任务在此加 `MagService` trait」。
- workspace `Cargo.toml` 成员 `crates/mag-protocol` → `crates/mag-service`；`mag-core/Cargo.toml` 依赖
  `mag-protocol` → `mag-service = { path = "../mag-service" }`。
- 源码 `use` 路径 `mag_protocol::` → `mag_service::`：`engine.rs`×3、`driver.rs`×2、`event_bus.rs`×1。
  `README.md` crate 列表同步更新为 `mag-service`。历史/已 `[DONE]` 记录（TODO.md、PLAN.md、docs、memory）
  中的 mag-protocol 字样按规则保持不变。
- 删除 `crates/mag-protocol`（随 git mv 迁走，无残留）；`grep mag[_-]protocol crates/ Cargo.toml README.md`
  无匹配；`Cargo.lock` 无 `mag-protocol`（cargo 已重生成含 `mag-service`）。
- 完整验证序列全绿：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`（干净）、
  `cargo test --workspace`（mag-core 9 + mag-service 5 + mag-sources 1 + mag-tools 1，doctest 全绿）、
  `cargo doc --no-deps --workspace`；`cargo tree -p mag-service` 依赖仅 serde/serde_json/uuid（agent-lib
  计数 0）。

### [DONE] CS-2 定义 `MagService` trait（近全集）+ `ServiceEvent`

**上下文**：

- `docs/DESIGN.md` §3.0：object-safe async trait（`#[async_trait]`），"命令方法 + `subscribe` 事件流 +
  `respond_interaction`/`cancel`"模型。**一次按近全集成型**（含多会话管理、审批、委派、source 探测），
  即便第一个 interface（ACP，I1）只用子集——避免后续 interface 改接口（`docs/DESIGN.md` §11 风险 6、
  `PLAN.md` 关键约束）。
- `ServiceEvent` 是中立可序列化事件枚举（`TextDelta`/`ToolStarted`/`ToolFinished`/`InteractionRequested`/
  `DelegationStarted…`/`RunFinished`/`RunError`…），Command/Event（§4）是它面向 tauri/web 的 wire 编码。

**做什么**：

- 在 `mag-service` 定义 `trait MagService`（签名照 `docs/DESIGN.md` §3.0）：`create_session`/`list_sessions`/
  `resume_session`/`delete_session`/`send_message`/`cancel`/`respond_interaction`/`subscribe`/
  `list_sources`/`probe_local_agents`；`ServiceError`、`ServiceEvent`、`UserInput`、`SessionInfo`、
  `SourceInfo` 等配套类型。确保 object-safe（供 `Arc<dyn MagService>`）。
- `subscribe(Option<SessionId>) -> BoxStream<'static, ServiceEvent>`（全局或按会话过滤）。
- 为 trait 与所有类型补 rustdoc；`ServiceEvent` 派生 serde。

**验证条件**：

- 编译期断言 `MagService` object-safe（`fn _assert(_: &dyn MagService) {}` 或 `Arc<dyn MagService>` 构造）。
- `ServiceEvent` serde round-trip 单元测试。
- 完整验证序列 1–5。

**完成记录（2026-07-18）**：

- 新增 `crates/mag-service/src/service.rs` 模块，`lib.rs` 加 `mod service;` 并 `pub use` 导出
  `MagService`/`ServiceError`/`ServiceEvent`/`SessionInfo`/`UserInput`；crate 级 rustdoc 更新为「trait +
  中立类型载体，`mag-core::Engine` 为实现」。保持既有 protocol 类型与已迁移文件历史不动（modular 拆分）。
- 定义 `#[async_trait]` object-safe `trait MagService: Send + Sync`，签名照 `docs/DESIGN.md` §3.0：
  `create_session`/`list_sessions`/`resume_session`/`delete_session`/`send_message`/`cancel`/
  `respond_interaction`/`subscribe(Option<SessionId>) -> BoxStream<'static, ServiceEvent>`/`list_sources`/
  `probe_local_agents`。全部命令方法返回 `Result<_, ServiceError>`。
- 配套类型：`ServiceEvent`（中立事件枚举，变体与 `Event` 对齐的近全集，`#[serde(tag="type",
  rename_all="snake_case")]` + `#[non_exhaustive]`，附 `session_id()` 便于 subscribe 过滤）；`ServiceError`
  （`#[non_exhaustive]` 枚举 + serde + `Display`/`Error`，含 `SessionNotFound`/`InteractionNotFound`/
  `InvalidInput`/`Unsupported`/`Backend`）；`UserInput { text, attachments }`（含 `UserInput::text`）；
  `SessionInfo { id, config }`（中立 serde）；`SourceInfo` 复用既有类型。均补 rustdoc，保持
  `#![warn(missing_docs)]` 干净。
- 依赖：`mag-service/Cargo.toml` 增加 `async-trait`、`futures`（workspace 版本）；`cargo tree -p mag-service`
  agent-lib 计数 0（仍不依赖 agent-lib）。
- 测试：编译期 `const _: fn()` object-safe 断言 + `DummyService` 经 `Arc<dyn MagService>` 调用（用
  `futures::executor::block_on`，不引入 tokio）；`ServiceEvent` 全 13 变体 round-trip + 稳定 tag；
  `session_id()` 分流；`ServiceError` round-trip/Display。
- 完整验证序列全绿：`cargo fmt --all -- --check`、`cargo test -p mag-service`（9 用例）、
  `cargo clippy --all-targets -- -D warnings`（干净）、`cargo test --workspace`（mag-core 9 + mag-service 9 +
  mag-sources 1 + mag-tools 1，doctest 全绿）、`cargo doc --no-deps --workspace`。
- 说明：本任务仅定义抽象，`Engine impl MagService` 与既有路径改经 trait 属 CS-3，未在此实现。

### [DONE] CS-3 `Engine impl MagService` + 既有路径改经 trait

**上下文**：

- `Engine`（C0-3/C1-2/C1-3）已有 `CreateSession`/`ListSessions`/`SendMessage` 等处理与事件总线。本任务把
  这些行为收敛为 `impl MagService for Engine`，事件总线经 `subscribe` 暴露。C1 的既有测试改为经 trait 调用。
- Command/Event 降为 tauri/web wire 编码：Engine 不再以 Command/Event 为内部 API，而是实现 trait 方法；
  Command/Event ↔ trait 的映射留给（未来的）tauri/web adapter，本任务不做 adapter。

**做什么**：

- `impl MagService for Engine`：把现有 `handle_command` 的分发改写为 trait 各方法；`subscribe` 返回事件流
  （包装现有 broadcast EventBus）；`ServiceEvent` 由现有 `Event`/`WireRunEvent` 映射产生。
- 既有 C1 单元测试（`engine::chat` 等）改为调用 `MagService` 方法 + 消费 `subscribe` 流断言事件序列，
  保持断言等价、仍绿。
- 评估 `MagIds`（C0-3）去留（facade 路径下 `FacadeIds` 为主，见 `PLAN.md` R-E），在完成记录说明。

**验证条件**：

- 既有 C1 测试改经 `MagService` trait 后仍绿（事件序列不回退）。
- 单元测试：经 `Arc<dyn MagService>` 调 `create_session` + `send_message` + `subscribe`，断言
  `RunStarted`→`TextDelta*`→`RunFinished`。
- 聚焦：`cargo test -p mag-core`（trait 路径用例）。
- 完整验证序列 1–5。

**完成记录（2026-07-18）**：

- `crates/mag-core/src/engine.rs`：删除 `handle_command`/`CommandOutput`/`EngineError`/引擎本地 `SessionInfo`/
  inherent `subscribe`，改为 `#[async_trait] impl MagService for Engine`。方法映射：`create_session`（铸 id、
  存 `SessionConfig`、emit `SessionCreated`）、`list_sessions`、`send_message`（查会话→`SessionNotFound`；无
  client→`Backend "no LLM client configured"`；否则交 run loop）、`subscribe(Option<SessionId>)`（包装既有
  broadcast `EventBus`，`Event`→`ServiceEvent` 投影，`Some(id)` 时按 `session_id()` 过滤）；`resume_session`/
  `delete_session`/`cancel`/`respond_interaction`/`list_sources`/`probe_local_agents` 暂返
  `ServiceError::Unsupported`（分属 C2/C3/C4）。`SessionManager` 简化为 `Mutex<BTreeMap<SessionId,
  SessionConfig>>`（仅元数据）。
- `crates/mag-service/src/service.rs`：新增 `impl From<Event> for ServiceEvent`（变体逐一映射，同 crate 无需
  通配）+ 单测 `event_projects_into_matching_service_event`，供 `subscribe` 投影复用。
- `crates/mag-core/src/driver.rs`：`send_message` 现返回 `RunId` 并在内部 emit `RunStarted` 与终态
  `RunFinished`/`RunError`；抽出私有 `drive()` 消费 `agent.stream(..)`。
- **关键：Send 正确性**。agent-lib facade `Agent::stream` 借用 `&mut agent` 且返回的 `AgentRunStream` 非
  `Send`，无法在 `#[async_trait]`（默认 `Send`）的 `impl MagService` future 内联驱动，也无法搬到多线程执行器。
  故新增 `crates/mag-core/src/run_loop.rs`：`Engine`（仅在 `with_llm_client` 时）拥有一条专用 OS 线程，线程内
  跑 `current_thread` runtime + `block_on` 循环，独占 `HashMap<SessionId, SessionDriver>`（agent 持久化以累积
  历史）。`send_message` 只经 `mpsc`/`oneshot`（均 `Send`、跨 runtime 唤醒有效）把 `RunLoopCommand::Run` 交给
  线程；`RunLoop: Drop` 关闭 channel 并 join 线程（测试不泄漏线程）。这是 `docs/DESIGN.md` §3.1 per-session
  actor 机制的最小实现，**非 workaround**；C2-1 在此之上重构为 per-session actor 并加 cancel 旁路/并发隔离。
- `MagIds`（C0-3）评估：已于 C1-3 随 facade 接入移除，改由 facade `FacadeIds` owns ids（见 `PLAN.md` R-E），
  本任务代码无 `MagIds` 残留，仅 `SessionId` 由引擎单调铸造。
- 既有 C1 单元测试全部改经 `MagService` trait（`create_session`/`list_sessions`/`send_message`/
  `subscribe(Some(id))`）并保持事件序列断言等价；新增 `unimplemented_methods_return_unsupported`、
  `send_message_to_unknown_session_reports_session_not_found`、`arc_dyn_service_streams_ordered_run_events`、
  `subscribe_filters_events_by_session`（双流分流）。
- 完整验证序列全绿：`cargo fmt --all -- --check`、`cargo test -p mag-core -p mag-service`（mag-core 12 +
  mag-service 10）、`cargo clippy --all-targets -- -D warnings`（干净）、`cargo test --workspace --all-targets`
  （mag-core 12 + mag-service 10 + mag-sources 1 + mag-tools 1）、`cargo doc --no-deps --workspace`（无警告）；
  `cargo tree -p mag-service` 仍不含 agent-lib。

**做什么**：核对 `mag-service` 与 `docs/DESIGN.md` §2/§3.0/§4 一致——纯抽象不依赖 agent-lib、object-safe、
近全集覆盖（多会话/审批/委派/source 都在 trait 里，即便暂无 interface 用）；`Engine` 实现无遗漏；
Command/Event 归属 mag-service 且仍是 `ServiceEvent` 的投影。汇总缺口。

**验证条件**：完整验证序列 1–5 全绿；`MagService` 方法 vs `docs/DESIGN.md` §3.0 对照表。

---

## Milestone C2 — 会话生命周期

目标：per-session driver actor（`docs/DESIGN.md` §3.1），多会话隔离，`CancelRun` 走不碰 agent `&mut` 的旁路。

### [DONE] C2-1 per-session driver actor

**上下文**：

- `docs/DESIGN.md` §3.1：每会话一个 actor，持 `mpsc<SessionCommand>` 入口；run 在 actor 内独立 task 推进；
  控制命令（cancel/respond）走旁路。用 actor 而非 Mutex，避免 run 长借用阻塞控制命令。
- agent-lib `Agent`/machine 是 `&mut self`，一个会话内 run 必须串行；actor 天然串行化会话内命令。
- 起点：CS-3 已建单条全局 run-executor 线程（`crates/mag-core/src/run_loop.rs`，`current_thread` runtime +
  `HashMap<SessionId, SessionDriver>`，run 串行、`send_message` 经 `mpsc`/`oneshot` 交付）。本任务把它重构为
  per-session actor（每会话一线程/task），并叠加 cancel 旁路与跨会话并发隔离；沿用其非 `Send` stream 只在
  持有 agent 的线程内驱动这一约束。

**做什么**：

- 实现 `SessionActor`：拥有该会话的 facade `Agent`（C1-3 后）+ cancel token。循环收 `SessionCommand`：
  `SendMessage`（spawn run task，驱动 `agent.stream(..)`，事件写总线）、`CancelRun`（触发 cancel token）、
  `RespondInteraction`（C3 用，先留钩子）。
- `SessionManager`：`CreateSession` 起一个 actor 并登记；命令按 `session_id` 路由；`DeleteSession` 停 actor。
- cancel：facade `Agent::stream` 消费循环用 `select!` 配合 cancel token；取消时丢弃 stream（agent-lib 保证
  committed 历史不变），emit 取消态。`CancelRun`/`RespondInteraction` 不碰 `Agent` 的 `&mut`（走 cancel
  token / pending oneshot 旁路，§3.1）。

**验证条件**：

- 单元测试：两个会话并行 `SendMessage`，`TextDelta`/`RunFinished` 按各自 `session_id` 正确分流、不串。
- 单元测试：一次长 run（fake client 脚本化"慢"流）中途 `CancelRun` → run 干净终止、emit `RunError`/取消态、
  会话仍可用（后续 `SendMessage` 正常）；另一会话不受影响。
- 聚焦：`cargo test -p mag-core engine::session`。
- 完整验证序列 1–5。

**完成记录**：

- 用 `crates/mag-core/src/session.rs`（新增，替换 CS-3 的 `run_loop.rs`，后者已 `git rm`）实现 per-session
  actor 架构：
  - `SessionManager`：`create_session` 为每个会话起一条独立 OS 线程（`current_thread` runtime + `LocalSet`），
    在返回前同步登记该会话的命令 sender（避免与后续路由竞争）；命令按 `session_id` 路由；`delete_session`
    丢弃 sender 并 join 线程；`Drop` join 所有线程。跨会话彻底隔离（各自线程/runtime/agent）。
  - `SessionActor`：状态机 `Idle(Box<SessionDriver>)`/`Running`/`Failed`。`run()` 循环 `select!` 收
    `SessionCommand` 与 run task 归还 driver 的 `run_done_rx`——即使 run 正在进行，actor 仍在等命令，
    `CancelRun` 不会被 run 借用阻塞（§3.1 旁路）。`SendMessage`（Idle）铸 run_id、emit `RunStarted`、
    提前回 run_id（使调用方可中途 cancel），再 `spawn_local` 驱动 `driver.run_turn(..)`；`SendMessage`
    （Running）入 `deferred` 队列，空闲后经 `take_idle_deferred` 重放；`RespondInteraction` 留钩子（回
    `Unsupported`，C3 用）。
  - `CancelToken`（`Arc<AtomicBool>` + `Arc<Notify>`）：`cancelled()` 快路径查标志，否则 `enable()` 后再查
    标志才 await，`cancel()` 置标志 + `notify_waiters()`，无丢唤醒。
- `crates/mag-core/src/driver.rs`：`run_turn` 用 `tokio::select!` 在 `stream.next()` 与 `cancel.cancelled()`
  间取舍；取消时先 drop stream（agent-lib `AgentRunStream::Drop` 干净 abandon，committed 历史不变、agent
  可复用）再 emit `RunError{message:"run cancelled"}`。cancel/respond 均不碰 agent `&mut`。
- `crates/mag-core/src/engine.rs`：`EngineInner` 改持 `SessionManager`；接通 `cancel`/`delete_session`/
  `send_message`/`respond_interaction`。`crates/mag-core/src/test_support.rs`：加 `StreamScript`（
  `Complete`/`Stall`）+ `stalling_text_stream` 供取消用例造"慢"流。
- 测试（`engine::session`）：`two_sessions_route_events_by_session_id`（两会话并行，事件按 `session_id`
  分流不串）；`cancel_mid_run_terminates_and_session_stays_usable`（长 run 中途 `CancelRun` → emit
  `RunError`、会话续用 `SendMessage` 正常、另一会话不受影响）。另加
  `cancel_and_delete_unknown_session_report_session_not_found` 与
  `cancel_and_delete_known_session_succeed`，并更新 `unimplemented_methods_return_unsupported`。
- 验证：`cargo fmt --all --check` ✓；`cargo test -p mag-core engine::session`（多次稳定）✓；
  `cargo clippy --all-targets -- -D warnings` 0 警告 ✓；`cargo test --workspace` 全绿（mag-core 16 /
  mag-service 10 / mag-sources 1 / mag-tools 1，doctests 0）✓；`cargo doc --no-deps --workspace` 无警告 ✓。

### [DONE] C2-R Review：会话隔离与 cancel 正确性

**做什么**：核对 actor 模型无死锁/饿死（cancel 不被 run 阻塞，§3.1）；多会话事件分流正确；会话状态在 run
失败/取消后一致。汇总缺口。

**验证条件**：完整验证序列 1–5 全绿；并发/取消用例覆盖。

**完成记录（2026-07-18）**：

- 纯 review 任务，代码核对无阻塞缺口，无需改动源码。

- **无死锁/饿死（cancel 不被 run 阻塞，§3.1）**：
  - `SessionActor::run` 用 `select!` 同时等 `commands.recv()` 与 run task 归还 driver 的 `run_done_rx`；
    run 在独立 `spawn_local` task 推进，因此即使 run 在途，actor 循环仍随时可服务 `CancelRun`——cancel 永不饿死。
  - `CancelRun` 只翻 `CancelToken`（`AtomicBool` + `Notify`），`RespondInteraction` 只碰 reply oneshot；
    两者都不借用 agent `&mut`，符合 §3.1「旁路」约束。
  - `CancelToken::cancelled()` 在第二次读标志前先 `enable()` 注册 waiter，`cancel()` 落在 check→await 窗口内
    也不会丢唤醒（无 lost wakeup）。
  - single-thread runtime + `LocalSet`：run task 与 actor 循环协作让点。有限（`Complete`）流会 drain 后结束；
    stalling 流在 `stream::pending()` 处 park，让出执行器给控制命令，无忙轮询饿死。
- **多会话事件分流**：每会话独占 OS 线程/runtime/agent（`SessionManager::create_session`），命令 sender 在
  返回前同步登记，`send_message` 不与 actor 线程竞态；事件写共享广播 `EventBus`（各带 `session_id`），
  `subscribe(Some(id))` 过滤。`two_sessions_route_events_by_session_id` 证明两 actor 不串话。
- **run 失败/取消后状态一致**：`run_turn` 总会 drop stream（agent-lib 丢弃在途 turn、committed 历史不变）并
  emit 恰好一个终止事件（`RunFinished` / `RunError` / `"run cancelled"`），随后经 `run_done_tx` 交还 driver，
  actor 收回 → `Idle`、清 `cancel`，会话续用。`cancel_mid_run_terminates_and_session_stays_usable` 证明：
  中途取消 → `RunError(cancelled)`、无 `RunFinished`、会话可续用、另一会话不受影响。driver 构建失败 →
  `Failed`，每次 `SendMessage` 回 `Backend` 且不 emit `RunStarted`。`delete_session` drop sender → actor 循环
  退出 → join 线程（在途 run 经 `LocalSet` 拆除而 abandon）；`send_message` 在 park 前 drop 克隆 sender，
  避免 deferred run 撑开命令通道阻塞 join。
- **前向缺口（非阻塞，已被后续里程碑覆盖或超出 C2 范围）**：
  - `RespondInteraction` 现回 `Unsupported`——C3 落地交互往返（actor 钩子已就位）。
  - `cancel(session_id)` 只取消在途 run，不移除已排队/deferred 的 `SendMessage`；符合规范「active run, if any」，
    无用例依赖丢弃 deferred；若 C3+ 引入按 run_id 取消语义再补。
  - `next_run_id` 为每会话单调（per-driver 计数器铸 uuid），会话内唯一、跨会话可能相同；因 run id 恒与
    `session_id` 成对且 `cancel` 以会话为目标，当前安全；若未来引入按 run_id 取消/全局关联，应换全局唯一 id 源。
  - committed 一致点 snapshot（§3.6）尚未接线——属 C4 持久化，明确后续里程碑。
  - 以上均非阻塞，也无未排期失败测试，无需插入新前置任务。
- **验证（完整序列 1–5 全绿）**：`cargo fmt --all -- --check` ✓；`cargo test -p mag-core engine::session`
  （2 用例，连跑 3 次稳定）✓；`cargo clippy --all-targets -- -D warnings`（0 警告）✓；
  `cargo test --workspace`（mag-core 16 / mag-service 10 / mag-sources 1 / mag-tools 1，doctests 0）✓；
  `cargo doc --no-deps --workspace`（无警告）✓。

---

## Milestone C3 — 工具 + 交互审批

目标：插件式工具 registry + 最小集，`IpcApproval` 异步暂停，`InteractionRequested`/`RespondInteraction`
往返。**这是 mag-core 最关键、最需充分测试的部分（`docs/DESIGN.md` §3.3、§9.1）。**

### [DONE] C3-1 `mag-tools`：`ToolPlugin` registry + 最小工具集

**上下文**：

- `docs/DESIGN.md` §7：`ToolPlugin` trait（`declaration()`→agent-lib `Tool`；`invoke(ctx, args)`；
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

**完成记录（2026-07-18）**：

- **`ToolPlugin` trait（`plugin.rs`）**：`name()` / `declaration() -> model::tool::Tool`（name/desc/JSON
  schema，与 facade `Tool::function_with_schema` 同形）/ `async invoke(ctx, args) -> ToolResult` /
  `permission() -> Option<PermissionSpec>`。另加默认 `permission_for(args)`（默认回 `permission()`）作为
  「risk 按命令」的留位钩子——read/list/grep 用默认，shell 覆写按命令细化 risk（DESIGN §7）。
  `PermissionSpec { category: ToolCategory, risk: ToolRisk }` 全 serde，`ToolRisk` 有序（Low<Medium<High）
  供 §8.1 未来 AI-permission。
- **mag 侧 `ToolRegistry`（`registry.rs`）**：传输无关收集器，持 `Vec<Arc<dyn ToolPlugin>>`；产
  `declarations()` 与 `tool_set(id) -> ToolSetRef`（供 C3-2 注入 `AgentSpec`）、`permission(name)`。
  `bind(ToolContextParts) -> PluginToolRegistry`，后者 `impl agent::ToolRegistry`：`declarations()` 汇总声明、
  `execute(call_id, call)` 按 name dispatch 建 `ToolContext`（worktree/cancel/tool_call_id 逐调用戳入）→
  plugin `invoke` → 用公开 getter 转 `ToolResponse`（`into_response` 私有）；未知工具 → `UnknownTool`。
- **四内置工具（`tools/*.rs`）**：`read_file` / `list_dir` / `grep`（只读，permission=None/auto，路径经
  `safe_join` 词法归一约束在 worktree 内、禁 `..` 逃逸与绝对路径；grep 为字面子串搜索、`spawn_blocking`
  递归遍历不跟 symlink、限 1000 命中）+ `shell`（permission=Shell/baseline Medium，`sh -c`、cwd=worktree、
  piped stdout/stderr 并发 drain、20ms 轮询 poll-based `cancel` token → `start_kill` 中断；非零退出→Error）。
- **验证（完整验证序列 1–5 全绿）**：`cargo fmt --all -- --check` ✓；`cargo test -p mag-tools`
  （6 单测 + 13 集成测：read/list/grep 结果、worktree 逃逸拒绝、shell stdout/worktree/非零退出、
  **shell cancel 预取消 sleep 30 <5s 中断**、declarations 四工具、execute 未知→`UnknownTool`、
  permission gate 元数据、tool_set 声明、risk 分级、safe_join）全绿 ✓；
  `cargo clippy --all-targets -- -D warnings`（0 警告）✓；`cargo test --workspace`
  （mag-core 16 / mag-service 10 / mag-sources 1 / mag-tools 6+13，doctests 0）✓；
  `cargo doc --no-deps --workspace`（无警告）✓。
- **前向留位（非阻塞）**：C3-3 将各 plugin `declaration()` 经 `Tool::function_with_schema` 接进 facade
  `Agent::builder().tool(..)`，并按 `permission()` 配 `ApprovalPolicy` 的 auto/ask gate；本任务只交付
  registry/plugin/工具与 `agent::ToolRegistry` 投影，未接 driver（符合 C3-1 边界）。

### [TODO] C3-2 `IpcApproval`：跨传输异步审批暂停点

**上下文**：

- **本单最关键任务**。`docs/DESIGN.md` §3.3、§9.1：实现 async `InteractionHandler`
  （`agent::InteractionHandler`，`../agent-lib/src/agent/drive.rs:154`，从 `agent_lib::agent` import——
  **不在 prelude**），`fulfill` 里发 `Event::InteractionRequested{request_id}` → `await oneshot` → 收前端
  `RespondInteraction` → 返回 `RequirementResult::Interaction`。machine **真正停在 `.await`**（这是 mag
  异步审批的核心，与 facade 内建 `FacadeApproval` 的同步「先 emit 再决策」不同）。样板可参考
  `agent_chat.rs:212` 的 `StdinApproval`（同一 trait 的实现示例）。
- **注入方式**（agent-lib M7-1）：通过 `AgentBuilder::interaction_handler(Arc<IpcApproval>)` 注入到 facade
  `Agent`（C1-3 已切到 facade），`run`/`stream` 两路都生效。注入的 handler 是被暂停交互的**唯一应答方**；
  **哪些工具暂停由 `ApprovalPolicy` 控制**（C3-3 用 `ask_tool`/`auto_allow` 配置），故 `IpcApproval` 只需
  应答，不需自己判 gate。
- 逐变体映射 `Interaction.kind`（Approval/Question/Choice/Permission）↔ `InteractionKindWire` /
  `InteractionResponseWire`。`Interaction`/`InteractionResponse`/`ApprovalResponse` 全 serde。
- `Permission` 分支预留 `PermissionDecider` 钩子（§8.1）：第一版默认"问前端"（发 InteractionRequested）；
  规则/LLM decider 留待后续，本任务只留 trait 调用点 + 默认实现。（注：mag 整体注入 `IpcApproval` 时，
  permission 决策在此分支处理，而非 agent-lib 的 `ApprovalPolicy::on_permission` 钩子——后者是不注入整体
  handler 时的备选，见 §8.1。）
- cancel：`fulfill` 的 await 用 `select!` 配合 session cancel token，取消时返回 deny/cancel。

**做什么**：

- 实现 `IpcApproval`（`impl agent::InteractionHandler`）：`pending: Mutex<HashMap<RequestId,
  oneshot::Sender<InteractionResponse>>>`，`fulfill` 注册 pending + emit + await（含 cancel select）。
  经 `AgentBuilder::interaction_handler(Arc<..>)` 注入。
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

- 把 C3-1 的工具与 C3-2 的 `IpcApproval` 接进 facade `Agent` builder：`.tool(..)` 注册每个工具、
  `.interaction_handler(Arc<IpcApproval>)` 注入审批、`.approval(policy)` 配 gate。工具事件
  `ToolStarted`/`ToolFinished` 从 `Agent::stream` 的 `RunEvent`（经 `to_wire()`）取得，mag 不再自己 bracket。
- 审批 gate：用 facade `ApprovalPolicy`（`ask_tool(name)`/`auto_allow`）决定哪些工具需审批——read/grep/
  list auto-allow、shell `ask_tool`。被暂停的交互由 `IpcApproval` 应答（C3-2）。

**做什么**：

- driver 组装带工具+审批的 `Agent`：`Agent::builder().provider(..).model(..).tool(read).tool(grep)
  .tool(shell)..approval(ApprovalPolicy::default().ask_tool("shell")).interaction_handler(ipc).build()`。
- 工具声明来自 mag-tools registry（C3-1 的 `ToolPlugin` → `Tool::function_with_schema`）；每工具的
  `permission()` 元数据映射到 `ApprovalPolicy` 的 gate（auto vs ask）。
- `Engine`/actor 把带工具的 `SendMessage` 走该 `Agent`；`RunEvent::{ToolStarted,ToolFinished,
  ApprovalRequested}` 经 `to_wire()` 映射进 mag `Event`。

**验证条件**：

- 单元测试（端到端离线）：fake client 脚本化"调用 shell 工具"的响应 → 收到 `InteractionRequested`
  （`IpcApproval` 发）→ test 回 approve → 收到 `ToolStarted`/`ToolFinished` → 工具结果回灌 → `RunFinished`。
  deny 路径亦覆盖。
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

- `docs/DESIGN.md` §3.6：SQLite 表 `sessions`/`snapshots`/`messages`；snapshot 只在 committed 一致点取
  （facade `Agent::snapshot()` 约束，run 中途不可 snapshot）。恢复用 `Agent::restore()` builder 重注入
  provider/工具/approval（client/工具闭包/审批 handler 不在 snapshot 里）。
- `PLAN.md` R-D：snapshot 存 JSON blob（agent-lib `AgentSnapshot` serde），schema 只存稳定列 + version 列。
- **R-B 依赖已满足**：agent-lib **M7-F1（`29c4e2a`，`[DONE]`）** 已给 `Agent::restore()`
  （`AgentRestoreBuilder`）补上 `interaction_handler(Arc<dyn InteractionHandler>)` 注入口，签名 / 相对
  `.approval(..)` 优先级 / 同步 + 流式两路生效均与 `AgentBuilder::interaction_handler` 对齐。恢复时重注入
  `IpcApproval` 即可跨进程审批，**本任务可直接完整支持"需审批会话恢复"，不再受限、不再需要 `#[ignore]` 占位**。
  snapshot 仍 data-only 不带该句柄，故恢复**必须**重注入；未重注入才回落同步 `FacadeApproval`。

**做什么**：

- 实现持久层（`rusqlite`/`sqlx`）：建表、`save_session`/`save_snapshot`/`load_snapshot`/`list_sessions`/
  `delete_session`。snapshot 存 facade `Agent::snapshot() -> AgentSnapshot` 的 JSON。
- actor 在每次 run 成功结束（committed）后取快照写库。
- `ResumeSession`：读快照 → `Agent::restore()` 重注入 provider/工具/approval（审批经
  `AgentRestoreBuilder::interaction_handler(Arc<IpcApproval>)` 重注入，R-B 已满足）→ 会话可继续。

**验证条件**：

- 单元测试（临时 SQLite）：run 后 snapshot 写库；`load_snapshot` round-trip 与内存态一致。
- 单元测试（跨"重启"，纯对话/只读工具会话）：会话 A 跑两轮 → 取快照 → 丢弃内存 Engine → 新 Engine
  `ResumeSession(A)` → 历史可见、第三轮 `SendMessage` 能看到前两轮上下文；id 不冲突。
- 单元测试：snapshot JSON 不含任何凭据/secret（断言）。
- 单元测试（跨"重启"，**需审批会话**，R-B 已满足）：会话跑到审批挂起 → 取快照（在挂起前的 committed 点）→
  新 Engine `ResumeSession` 重注入 `IpcApproval` → 后续 run 触发的审批仍走跨进程 `IpcApproval` 往返（离线用
  scripted handler 断言 restore 后审批暂停点仍在 handler 侧应答，而非同步 `FacadeApproval`）。
- 聚焦：`cargo test -p mag-core persist`。
- 完整验证序列 1–5。

### [TODO] C4-2 凭据存储（`mag-sources`）

**上下文**：

- `docs/DESIGN.md` §3.5：`CredentialStore` trait，优先 OS keyring（`keyring` crate），回退加密文件。凭据绝不进
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

**上下文**：把 C1–C4 串成一条离线全链路，作为 service 稳定的证据，也是推进 interface（I1/ACP）的前置门槛。
全链路**经 `MagService` trait**（`Arc<dyn MagService>`）驱动，不碰 Engine 内部。

**做什么**：

- 写集成测试（`crates/mag-core/tests/`，fake client + 脚本化工具 + 临时 SQLite + 内存审批 channel）：经
  `Arc<dyn MagService>` 建会话 → 多轮对话（含流式，消费 `subscribe`）→ 调 read 工具（auto）→ 调 shell 工具
  （审批 approve）→ 一次 deny → run 中途 `cancel` → 取快照 → 新 Engine `resume_session` → 继续对话。全程
  断言 `subscribe` 事件序列与状态。
- 覆盖并发多会话隔离（`subscribe(Some(id))` 按会话过滤正确）。

**验证条件**：

- 集成测试全绿，离线、无网络/凭据/CLI 依赖，1 分钟内完成。
- 聚焦：`cargo test -p mag-core --test e2e_offline`。
- 完整验证序列 1–5。

### [TODO] C5-R Review：mag-core 整体验收 + 契约冻结

**上下文**：service 全部里程碑收官 review，决定是否放行 interface 层（`docs/DESIGN.md` §10 I1，ACP 优先）。

**做什么**：

- 逐条对照 `docs/DESIGN.md` §3（接口+实现）/§4（Command/Event 编码）/§9（agent-lib 约束）：service interface 无关、
  接口与实现分离（`mag-service` 不依赖 agent-lib）、`MagService` object-safe 且近全集、审批异步暂停、
  run/控制解耦、snapshot 无 secret、扩展点留位（§8）——是否全部满足。
- **冻结 `MagService` trait 契约**（后续只加方法/变体/字段，不改既有语义）；`Command`/`Event` 作为其 wire
  编码一并冻结；若启用 TS codegen，产出并校验前端类型。
- 汇总所有里程碑遗留缺口；确认无未调度失败测试；记录哪些 R（风险）已消解、哪些转为上层任务
  （**R-B facade restore 注入口 = agent-lib M7-F1 已落地、缺口已消解**；R-C 本地 agent 接入 = §10 I2）。
- **放行判据**：C0–C1 + CS + C2–C5 全 `[DONE]`、完整验证序列全绿、端到端离线主干测试（经 `MagService`）
  稳定通过。满足后方可另起 interface 任务单，**第一个是 mag-acp（I1）**。

**验证条件**：完整验证序列 1–5 全绿；`docs/DESIGN.md` §3/§4/§9 逐条对照表 + `MagService` 方法 vs §3.0 对照；
契约冻结说明。

---

## 交接（service 收官后转入 ACP interface）

### [TODO] H-1 归档 service 计划并为 ACP interface 起草新 PLAN.md + TODO.md

**上下文**：

- 本任务在 **C5-R 通过、`MagService` 契约冻结之后**执行（放行判据见 C5-R）。此时 service 主干（`mag-service`
  接口 + `mag-core` 实现 + mag-tools/mag-sources 核心）已全部 `[DONE]`，进入第一个 interface：**mag-acp**
  （`docs/DESIGN.md` §10 的 I1）。
- 唯一设计输入是 [`docs/ACP.md`](docs/ACP.md)（mag-acp 的实现级设计），并参照 [`docs/DESIGN.md`](docs/DESIGN.md) §5（ACP 概览）、
  §3.0（`MagService` trait，mag-acp 面对的接口）。底层协议库形状见 `docs/ACP.md` §1（`agent-client-protocol`
  v1.2.0 的 builder + typed-handler 模型、schema crate、`ConnectionTo<Client>` 句柄）。
- 本任务是**计划编制任务**，不写产品代码——只归档旧计划、产出新 PLAN.md 与新 TODO.md。产出的任务单要能让
  后续 coding agent 直接照做、无需反复检索代码库。

**做什么**：

1. **归档当前计划**：
   - 建目录 `docs/archive/<YYYY-MM-DD>-mag-service/`（日期用当天），把当前的 `PLAN.md` 与 `TODO.md` 移入
     （保留全部 `[DONE]` 记录作为历史）。
   - 在归档目录放一个简短 `README.md`：一句话说明这是 service 主干（`mag-service`+`mag-core`）的已完成计划，
     并链接回根 `docs/DESIGN.md` / `docs/ACP.md`。
2. **为 ACP interface 写新的 `PLAN.md`**（放回仓库根，覆盖归档后的空位）：
   - 唯一设计输入指向 `docs/ACP.md`（+ `docs/DESIGN.md` §5/§3.0）；范围限定为 **mag-acp crate**（第一个 interface），
     明确非目标（不做 web/tauri/前端、不改已冻结的 `MagService` 契约、不碰 mag-core 实现细节）。
   - 现有代码锚点：`docs/ACP.md` §1 的 acp crate API（方法↔类型表、handler 闭包签名、`Stdio`/`connect_to`、
     `ConnectionTo<Client>` 的 `send_notification`/`send_request().block_task()`）；`MagService` trait 的
     方法（`docs/DESIGN.md` §3.0）；schema 类型出处（`agent_client_protocol::schema::v1::*`，来自
     `agent-client-protocol-schema` v1.4.0）。锚点要带足够信息（类型名、方法名），避免实现时再翻库。
   - 里程碑划分参照 `docs/ACP.md` 的结构，建议顺序：M1 crate 骨架 + `initialize`/`session/new` + stdio 跑通；
     M2 `session/prompt` 泵（`docs/ACP.md` §3.4）+ 类型映射（§4）；M3 审批桥接（§5）；M4 cancel（§3.5）+
     `session/load`（§3.3，含 restore 缺口约束）+ 能力宣告收口（§7）；M5 协议级 e2e（§9）+ 收官验收。
     （最终以 `docs/ACP.md` 为准细化，milestone 数量可调。）
   - 关键设计约束：mag-acp 只依赖 `mag-service` + acp crate（不依赖 mag-core/agent-lib）；能力如实宣告
     （§7）；审批复用同一 gate（§5）；shell 等特权工具审批不可省（§6）。
   - 测试策略：全离线——映射纯函数单测、handler 级用 scripted `Arc<dyn MagService>`、协议级 e2e 用 acp
     crate 自身 client（或 agent-lib ACP client）经内存管道驱动（`docs/ACP.md` §9）；真实 Zed 联调 `#[ignore]`。
   - 验证序列沿用当前 PLAN.md 的形态（fmt / 聚焦测试 / clippy / `cargo test --workspace` / doc）。
3. **为 ACP interface 写新的 `TODO.md`**（放回仓库根），满足以下**硬性格式要求**：
   - 任务按**实现顺序**排列并编号：`M<里程碑>-<序号>`（如 `M1-1` = milestone 1 第一个任务），依此类推。
   - 每个任务**标题带 `[TODO]` 标记**（形如 `### [TODO] M1-1 <标题>`），供 coding agent 识别未完成。
   - 每个任务含**足够的细节与上下文**（沿用本文件的「上下文 / 做什么 / 验证条件」三段式），带精确锚点
     （`docs/ACP.md` 的节号、acp crate 的类型/方法名、`MagService` 方法名），使实现时**无需反复检索代码库**。
   - 每个任务定义**完整的验证条件**（聚焦测试的精确过滤名 + 完整验证序列；离线、可判定绿/红）。
   - **每个 milestone 结尾加一个单独的 review 任务**（`M<n>-R`），核对本阶段正确性与完整性（对照 `docs/ACP.md`
     对应节、确认无遗漏映射/未调度失败测试）。
   - 顶部保留「通用执行规则」块（一次一个任务、`[TODO]`→`[DONE]`、离线测试纪律、不改已冻结 `MagService`
     契约、发现 acp crate 缺口的处理约定等），与本文件风格一致。

**验证条件**：

- 归档：`docs/archive/<date>-mag-service/{PLAN.md,TODO.md,README.md}` 存在且内容为原文件；根 `PLAN.md` /
  `TODO.md` 已被新内容替换。
- 新 `PLAN.md`：唯一设计输入为 `docs/ACP.md`；范围/非目标/锚点/里程碑/约束/测试策略/验证序列齐备；无悬空引用
  （引用的 `docs/ACP.md` 节号、acp crate 类型均真实存在）。
- 新 `TODO.md`：全部任务标题带 `[TODO]`；编号连续且按实现顺序；每个任务三段式 + 完整验证条件；每个
  milestone 末尾有 `M<n>-R` review 任务；顶部有通用执行规则块。
- 纯文档任务：无需 `cargo` 验证；`git diff --check` 干净（无行尾空白/冲突标记）。
- 自检：新 TODO.md 的 M1-1 可被一个 coding agent 直接开始（上下文自足，不依赖本次对话的口头背景）。
