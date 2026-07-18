# 实施计划：mag service（`mag-service` 接口 + `mag-core` 实现）

> 唯一设计输入：[`docs/DESIGN.md`](docs/DESIGN.md)（尤其 §1 全局架构、§3 mag-service 接口与 mag-core 实现、
> §4 Command/Event 编码、§9 接入 agent-lib 约束）。
> 底层库 [`agent-lib`](../agent-lib) 的接入面见其 [`README.md`](../agent-lib/README.md) 与
> [`docs/DESIGN.md`](../agent-lib/DESIGN.md)；facade 缺口与后续注入口见 agent-lib 的
> [`PLAN.md`](../agent-lib/PLAN.md) Milestone 7。
>
> **本计划覆盖 service 主干：`mag-service`（抽象接口）+ `mag-core`（实现 `MagService` 的引擎）+ 直接依赖的
> mag-tools / mag-sources 核心。** 各 interface（mag-acp / mag-server / mag-tauri / app 前端）**不在本计划
> 范围**——它们必须等 service 充分测试通过、`MagService` 契约稳定后，再按 `docs/DESIGN.md` §10 的 I1（ACP 优先）
> 起单独规划。逐任务清单见 [`TODO.md`](TODO.md)。

## 目标

落地 `docs/DESIGN.md` §3 描述的 service：一个 **interface 无关**的 service——抽象为 `mag-service::MagService`
trait（trait + 中立 serde 类型，含 Command/Event 编码），由 `mag-core::Engine` 实现。它能装配并驱动
agent-lib、管理多会话、把跨 interface 的审批往返做成异步暂停点、持久化会话——**但不绑定任何具体
interface**（ACP/web/Tauri 都在上层）。具体：

- **interface 无关的 service 接口**：`MagService` 是 object-safe 的 async trait（命令方法 + `subscribe`
  事件流 + `respond_interaction`/`cancel`，`docs/DESIGN.md` §3.0）；`Engine` 是唯一实现。测试直接调 trait 方法、
  断言 `subscribe` 事件流序列。Command/Event（§4）是它面向 tauri/web 的 wire 编码，随接口一起定义在
  `mag-service`。
- **facade `Agent` + 注入 `IpcApproval`**（`docs/DESIGN.md` §3.2、§9.1）：用 facade `Agent::builder()` +
  `Agent::stream`，经 `AgentBuilder::interaction_handler(Arc<IpcApproval>)`（agent-lib M7-1）注入异步审批
  handler。**不再下沉自组 scope**（M7 已把注入口透出到 facade）。
- **跨传输审批 gate**（§3.3）：`IpcApproval` 实现 async `InteractionHandler`，`fulfill` 里发
  `InteractionRequested`、await oneshot、收 `RespondInteraction` 后折回。machine 真正停在 `.await`。
  审批 gate 由 `ApprovalPolicy`（`ask_tool`/`auto_allow`）控制，`IpcApproval` 是被暂停交互的应答方。
- **流式**：facade `Agent::stream` 直接产出 `RunEvent::TextDelta`；用官方 `RunEvent::to_wire()
  -> WireRunEvent`（agent-lib M7-2）序列化后映射进 mag `Event`。**不再自建 tap handler**。
- **会话生命周期**：per-session driver actor（§3.1），run 在独立 task 推进，`CancelRun`/
  `RespondInteraction` 走不碰 agent `&mut` 的旁路。
- **持久化**：committed 一致点取 `AgentState` snapshot 写 SQLite；`ResumeSession` 重装配（§3.6）。
- **插件式工具**（§7）：`ToolPlugin` registry，第一版最小集（read_file/list_dir/grep 只读 + shell 审批）。
- **充分测试**：mag-core 的每条关键路径都有**离线**测试（fake `LlmClient` + 脚本化工具/审批），
  这是本计划的重点交付物，不是附带。

## 非目标（本计划）

- 不做任何具体 interface（ACP stdio、web WebSocket、Tauri invoke/emit）——那是上层，等 service 稳定、
  `MagService` 契约冻结后再做（第一个是 ACP，`docs/DESIGN.md` §10 I1）。**但 `MagService` trait 本计划就要按
  近全集一次成型**（含多会话管理、审批、委派、source 探测），使后续 interface 无需改接口。
- 不做前端（React/app）。
- 不做本地 agent 来源的 live 接入（打开 external features、用 `default_external_session_handler`）——
  service 主干先跑通 **LLM API 来源 + 本地工具**；本地 agent 来源作为 `docs/DESIGN.md` §10 的 I2 单列。但
  source registry / 委派数据结构要为它**预留位置**，不写死。
- 不实现 AI-based permission / AI-based routing 的**决策逻辑**（§8）——只把接缝（`PermissionDecider`
  钩子位置、routing 配置字段）留在结构里。

## 现有代码锚点（mag-core 的精确接入面，全部已核对 agent-lib 源码）

> **接入面已随 agent-lib Milestone 7（已 `[DONE]`）更新**：mag-core 走 facade 注入路径，不再下沉自组
> scope。下列锚点以 facade M7 API 为准。

- **facade Agent 构造 + 审批注入**：`facade::Agent::builder().provider(..).model(..).tool(..)
  .approval(policy).interaction_handler(Arc<dyn InteractionHandler>).build()`
  （`AgentBuilder::interaction_handler` = `../agent-lib/src/facade/agent.rs:1015`；同步/流式两路都生效）。
  `Agent::stream(input) -> AgentRunStream`（逐 `RunEvent`）。
- **审批 handler trait**：`agent::InteractionHandler`（`../agent-lib/src/agent/drive.rs:154`，`#[async_trait]
  async fn fulfill(&self, req: &Interaction, ctx: &RunContext) -> RequirementResult`；**不在 prelude，需从
  `agent_lib::agent` import**）。`IpcApproval` 实现它，在 `fulfill` 里 emit + await oneshot。
  `Interaction`/`InteractionKind`/`InteractionResponse` 均 serde 友好（`src/agent/interaction.rs`）。
- **事件序列化**：`RunEvent::to_wire() -> WireRunEvent`（`../agent-lib/src/facade/run.rs:329`）；
  `WireRunEvent`（`run.rs:392`，`serde(tag="type",content="data")`）与 `WireRunOutput` 在 prelude。
  `RawStream`/`RawNotification` 折叠为 `WireRunEvent::Raw(RawEventKind)`。
- **富化审批请求**：`facade::ApprovalRequest`（`run.rs:464`，prelude）带
  `tool_name`/`call_id`/`reason`/`input`（脱敏摘要）。
- **工具**：`facade::Tool::function_with_schema(name, desc, json_schema, handler)`（`src/facade/tool.rs`，
  无需 feature），async 闭包 `|ctx: ToolContext, args| async {..}`；`AgentBuilder::tool(..)` 注册。
  `ToolContext` 带 `worktree`/`cancel`/`tool_call_id`。审批 gate 用 `ApprovalPolicy::ask_tool`/`auto_allow`。
- **权限模型**：`agent::permission::{PermissionRequest, PermissionCategory, PermissionRisk,
  PermissionDecision, PermissionResponse}`（全 serde；risk 有序）；AI-permission 落点在 `IpcApproval` 的
  Permission 分支（`docs/DESIGN.md` §8.1）。
- **持久化**：`Agent::snapshot() -> AgentSnapshot`（committed 一致点，data-only）/ `Agent::restore()`
  builder 重注入 provider/工具/approval，审批经 `AgentRestoreBuilder::interaction_handler(..)`（agent-lib
  M7-F1，与 `AgentBuilder` 对齐）重注入 `IpcApproval`（见 R-B，缺口已消解）。
- **纯对话备选（C0/C1 早期脚手架）**：facade `Chat`/`ChatSession`（`src/facade/chat.rs`，无工具）。
- **id source**：facade 内建 `FacadeIds`；mag 若需确定性可选 `AgentBuilder::ids(..)`。自组 scope 时代的
  `MagIds`（已在 C0-3 实现）在 facade 路径下降级为可选/仅测试用，切换时评估去留。

## 里程碑

逐层增加概念、每层可独立**离线**验证。C0–C1 是引擎主干起步，**CS 抽取 `mag-service` 接口**，C2 会话
生命周期，C3 工具+审批，C4 持久化，C5 收官验收。

> **执行顺序**：C0 → C1（含 C1-3 切 facade、C1-R）→ **CS**（抽 `mag-service`、`Engine impl MagService`）→
> C2 → C3 → C4 → C5。CS 插在 C1 之后、C2 之前，因为此时 Engine 表面积最小、抽接口最便宜；C2 起的会话/
> 工具/审批都直接对着 `MagService` trait 写。

| 里程碑 | 主题 | 主要产出 | 默认测试形态 |
|---|---|---|---|
| C0 | 骨架 + 协议 | workspace、`mag-protocol`（`Command`/`Event` + payload，全 serde）、`Engine` 空壳 + 内存事件总线、id source。**[DONE]（mag-protocol 后由 CS 并入 mag-service）** | 单元（协议 round-trip、Engine 起停） |
| C1 | 纯对话流式 | facade `Agent::stream` 驱动一次 LLM turn、经 `WireRunEvent` 映射 emit `TextDelta`、`SendMessage`→`RunFinished`（早期按自组 scope 实现，随 agent-lib M7 落地切到 facade，见 `TODO.md` C1-3） | 单元（fake `LlmClient` 脚本化 delta，离线） |
| **CS** | **抽取 `mag-service` 接口** | 新建 `mag-service` crate、`Command`/`Event` 等从 `mag-protocol` 迁入、删 `mag-protocol`；定义 `MagService` trait（近全集，object-safe）+ `ServiceEvent`；`Engine impl MagService`；mag-core 依赖 mag-service | 单元（trait 方法调用 + `subscribe` 事件流断言；既有 C1 测试改经 trait 后仍绿） |
| C2 | 会话生命周期 | per-session driver actor、`create_session`/`list_sessions`/`cancel`（`MagService` 方法）、多会话隔离、cancel 旁路 | 单元（并发会话、run 中途 cancel，离线） |
| C3 | 工具 + 交互审批 | `mag-tools` registry + 最小集、`IpcApproval` 异步暂停、`InteractionRequested`/`respond_interaction` 往返、`ToolStarted`/`ToolFinished` | 单元（脚本化工具 + 审批 channel，验证 machine 真停到 resolve） |
| C4 | 持久化 | SQLite、committed 点 snapshot、`resume_session` 重装配、凭据 store（不进 snapshot） | 单元（snapshot round-trip、跨"重启"恢复历史，离线临时库） |
| C5 | 收官验收 | 端到端离线主干（建会话→多轮带工具+审批→cancel→持久化→恢复）、`MagService` 契约冻结 review | 集成（离线全链路，经 `MagService` trait）+ review |

每个里程碑末尾隐含一次自检：全部离线测试绿、无网络/凭据/CLI 依赖、契约变更向后兼容（只加 enum 变体/
`#[non_exhaustive]` 加字段）。**C5 通过（`MagService` 契约冻结）后方可推进 `docs/DESIGN.md` §10 的 I1（ACP
interface，第一个），再到 I2/I3/I4。**

## 关键设计约束（落地时必须守）

- **service interface 无关**：`mag-core` 不依赖 tauri / axum / ACP crate；只依赖 mag-service、agent-lib、
  mag-tools、mag-sources。任何"发事件"都写内存事件总线，由 `subscribe` 暴露、（未来的）interface 订阅。
- **接口与实现分离**：`mag-service` 是纯抽象（`MagService` trait + 中立 serde 类型，含 Command/Event），
  **不依赖 agent-lib、不含实现**；`mag-core::Engine` 是唯一实现。需要的底层类型（`InteractionResponse`
  等）在 mag-service 侧薄封装/重定义为 wire 类型，不把 agent-lib 内部类型泄漏进接口。
- **`MagService` 一次按近全集成型**：CS 里就带上 GUI 视角需要的方法（多会话管理、委派、source 探测），
  即使 I1（ACP）只用子集——避免 ACP 视角把签名带窄，后续 interface 免改接口（`docs/DESIGN.md` §11 风险 6）。
- **审批是异步暂停点，不是通知**：`IpcApproval::fulfill` 必须真正 await；决不能像 facade 那样"先 emit
  再同步决策"。测试必须断言 machine 在 resolve 前不前进。
- **run 与控制命令解耦**：actor 里 run 占用 agent `&mut`；`cancel`/`respond_interaction` 只碰 cancel
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

- **R-A【已消解】流式 tap**：早期担心库 `LlmClientHandler` 内部聚合、需自建 `StreamingTapHandler`。
  agent-lib M7 之后走 facade `Agent::stream`（已产 `TextDelta`）+ `WireRunEvent`，无需自建。C1 早期实现的
  自建 tap 随 C1-3 切换移除。
- **R-B【已消解】restore 审批注入口**：M7-1 透出 `AgentBuilder::interaction_handler(..)`（主路径），
  **M7-F1（`29c4e2a`）已补齐对应的 `AgentRestoreBuilder::interaction_handler(Arc<dyn InteractionHandler>)`**，
  签名 / 相对 `.approval(..)` 优先级 / 同步 + 流式两路生效均与 `AgentBuilder` 对齐。恢复出的会话重注入
  `IpcApproval` 后即可跨进程审批，**C4 恢复对需审批会话已完全可用**，原「只对纯对话/只读工具会话可用」的限制
  作废。snapshot 仍 data-only 不带该句柄，故恢复须重注入；未重注入才回落同步 `FacadeApproval`。C4-1 不再被
  此缺口阻塞。
- **R-C 本地 agent 来源推迟**：mag-core 主干先只做 LLM API + 本地工具。source registry 与委派结构必须
  为本地 agent 预留位置（enum 变体 / trait 对象槽），但不实现 live 接入。I2（`docs/DESIGN.md` §10）用 M7-4 的
  `default_external_session_handler` 直接接入，无需自己 wire。避免主干被 external feature 与 CLI 环境
  依赖拖慢。
- **R-D 持久化 schema 演进**：SQLite schema 首版可能变。取向：snapshot 存 JSON blob（agent-lib
  `AgentSnapshot` 的 serde），schema 只存 id/config/时间戳等稳定列，降低迁移成本；预留 schema version 列。
- **R-E id source**：facade 路径下会话 id 由内建 `FacadeIds` 管理，恢复经 `Agent::restore()` 的
  `FacadeIds::continuing_after` 续号，mag 一般不需自管。C0-3 已实现的 `MagIds` 在 facade 路径下降级为
  可选/仅测试；C1-3 切换时评估是否保留。
