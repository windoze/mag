# TODO：mag-cli 落地任务单（最小 CLI 验证原型 + 运行时配置系统）

> 依据 [`PLAN.md`](PLAN.md) 与**唯一设计输入** [`docs/CLI.md`](docs/CLI.md)（决策 D1–D6、§5 service 主干
> 前置改动清单、§5A agent-lib 前置需求——A/B 类已在 agent-lib 侧全部完成）。
> **范围：`docs/CLI.md` §5 的 service 主干前置改动（M1–M5）+ `mag-cli` crate 本身（M6）。**
> 既有计划归档：[`docs/archive/2026-07-19-mag-service/`](docs/archive/2026-07-19-mag-service/)、
> [`docs/archive/2026-07-20-mag-acp/`](docs/archive/2026-07-20-mag-acp/)。

## 通用执行规则

- **一次一个任务**：每次只执行「首个标题带 `[TODO]` 的任务」。完成后把 `[TODO]` 改为 `[DONE]`，在任务末尾
  补「完成记录」，提交并推送，然后继续下一个任务（本计划按用户指令连续推进，不停顿等待）。
- **完成的定义**：只有标题带 `[DONE]` 才算完成。仅填完成记录而标题仍 `[TODO]`，按未完成处理。review 任务
  （`M<n>-R`、`F-R`）是真实任务，不得跳过。
- **编号**：任务按实现顺序编号 `M<里程碑>-<序号>`（如 `M1-1` = milestone 1 第一个任务）；每个里程碑末尾有
  独立 review 任务 `M<n>-R`；全部里程碑完成后有一次全计划 review `F-R`。
- **依赖边界（硬约束）**：
  - `mag-config`（新 crate）：纯数据 crate，**只依赖** serde/toml/thiserror 等通用 crate；**不得**依赖
    agent-lib / mag-core / mag-service。DTO↔DO 双向转换在本 crate 内完成。
  - `mag-service`：不依赖 agent-lib / mag-core（现状保持）；对契约只做**向后兼容新增**（方法/变体/字段），
    不改既有方法签名与事件语义；新增事件变体/字段必须 `#[serde(default)]` 或新增变体（`#[non_exhaustive]`
    已就位）。
  - `mag-core`：可依赖 mag-config / mag-service / mag-sources / mag-tools / agent-lib；M4 起 agent-lib
    依赖开 `external-acp` feature。
  - `mag-cli`（新 crate）：**只依赖** `mag-service`（+ rustyline/tokio/futures/serde/serde_json）；
    **不得**依赖 `mag-core` / `agent-lib` / `mag-config`，面对 `Arc<dyn MagService>`。装配
    `mag-core::Engine` 注入的是上层 bin（`crates/mag`）。
- **不改已冻结语义**：若发现前置缺口（缺事件/缺字段/缺方法），在本文件正确依赖位置插最小前置任务
  （按向后兼容方式加），让被阻塞任务显式依赖它，然后提交并继续。
- **离线测试纪律**：所有测试必须离线——LLM 用 fake `LlmClient`；interface 级注入 scripted
  `Arc<dyn MagService>`；CLI e2e 用管道驱动 stdin/stdout；external ACP 用本地 fake ACP 进程脚本
  （`docs/CLI.md` §6）。不依赖网络 / 真实凭据 / 真实 LLM / 真实 ACP agent。每个测试须 1 分钟内完成，卡住
  即为 bug，须立刻修。真实联调一律 `#[ignore]`，缺环境干净跳过（绿），不输出 secret。
- **配置 secret 纪律**：配置文件只存引用（`{env=...}` / `{keyring=...}`），任何测试/日志不输出解析后的
  secret 值（`docs/CLI.md` §4.1）。
- **默认完整验证序列**（任务另有放宽以任务为准）：
  1. `cargo fmt --all -- --check`
  2. 聚焦测试（任务给出精确过滤名）
  3. `cargo clippy --all-targets -- -D warnings`
  4. `cargo test --workspace`
  5. `cargo doc --no-deps --workspace`
- **公开 API 必须带 rustdoc**（crate 开 `#![warn(missing_docs)]`）。
- 环境：cargo 不在默认 PATH，每个 shell 先 `export PATH="$HOME/.cargo/bin:$PATH"`。

### 复用锚点（各任务通用，避免反复翻库）

- **`MagService` trait**（`crates/mag-service/src/service.rs`，object-safe，`Arc<dyn MagService>`）：
  `create_session(SessionConfig)->SessionId`、`list_sessions()->Vec<SessionInfo>`、`resume_session(SessionId)`、
  `delete_session(SessionId)`、`send_message(SessionId,UserInput)->RunId`、`cancel(SessionId)`、
  `respond_interaction(SessionId,RequestId,InteractionResponseWire)`、
  `subscribe(Option<SessionId>)->BoxStream<'static,ServiceEvent>`、`list_sources()`、`probe_local_agents()`；
  async 方法返回 `Result<_,ServiceError>`。`ServiceError`（`#[non_exhaustive]`）现有变体：
  `SessionNotFound/InteractionNotFound/InvalidInput/Unsupported/Backend`。
- **`ServiceEvent`**（同文件，`#[serde(tag="type",rename_all="snake_case")]`，`#[non_exhaustive]`）：
  `SessionCreated{id,config}`、`RunStarted{id,run_id}`、`RunFinished{id,output:RunOutput}`、
  `RunError{id,message,kind:RunErrorKind}`、`TextDelta{id,text}`、`ToolStarted{id,trace:ToolTrace}`、
  `ToolFinished{id,trace}`、`InteractionRequested{id,request_id:RequestId,kind:InteractionKindWire}`、
  `DelegationStarted/Finished/Failed{id,trace:DelegationTrace}`、`DelegationMessage{id,message}`、
  `LocalAgentsProbed{available}`；`ServiceEvent::session_id()->Option<SessionId>`。
- **wire 类型**（`crates/mag-service/src/lib.rs`）：`SessionId/RunId/RequestId`（`transparent` 包 `Uuid`）；
  `UserInput{text,attachments}`；`SessionConfig{provider,model,tool_profile,cwd,routing:RoutingMode,
  budget:Option<SessionBudget>}`；`RunErrorKind::{Other,Cancelled,LoopLimitExceeded,BudgetExhausted}`；
  `ToolTrace{run_id,call_id,name,input,output,status:ToolStatusWire,message}`；
  `InteractionKindWire{Approval{call_id,requirement},Question{prompt},Choice{prompt,options},
  Permission{action_id,actor,category,risk,summary,subject,reason}}`；
  `InteractionResponseWire{Approval{step_id,call_id,decision,message},Answer{text},Choice{index},
  Permission{action_id,decision}}`；`ApprovalDecisionWire::{Approve,Deny,Timeout,Cancel}`。
- **mag-core driver**（`crates/mag-core/src/driver.rs`）：`run_turn` 用
  `agent.stream_with_cancel(text, cancel)` 消费事件流；cancel 已实现（CancelHandle 模式，pivot 旁路同构）；
  `Engine` 现有构造器 `new/with_llm_client/with_llm_client_and_tools/with_persistence`
  （`crates/mag-core/src/engine.rs`）。
- **agent-lib 已就绪能力**（mag 直接可用，无需再改 agent-lib）：
  `AgentRunStream::interject()`（pivot，仅 step 边界窗口接受，InvalidState 失败无副作用、盲重试安全）；
  子 agent 交互带 `InteractionOrigin{delegate,depth}` 路由到父级注入 handler；
  `Agent::reconfigure(ReconfigRequest)`（Idle 时准入，skill 变体报 Config 错）；`Agent::worker()`；
  `ManagedExternalAgent::acp(binary,args)` + `default_external_session_handler`
  （behind `external-acp` feature，默认关）；cancel 可抢占阻塞批（2s 宽限后 detach）。
- **配置文件设计**（`docs/CLI.md` §4，决策 D4）：TOML 于 `~/.config/mag/config.toml`（示例见 §4.2）；
  secret 只存引用；生效时机决策 D2（会话创建钉住 `Arc<ConfigSnapshot>` + `apply_config` 到 turn 边界，
  审批策略下一 run 生效）；turn-complete 通用通知/回调机制见 §4.5。

---

## Milestone M1 — pivot 能力（`docs/CLI.md` §3.2，决策 D1）

目标：`MagService` 暴露两层 pivot 语义的第一层（`pivot_message`，无 in-progress turn 则 `NotPivotable`），
mag-core driver 落地 pivot 队列旁路（agent-lib `interject()`）。第二层「不能 pivot 自动转 send_message」
在 CLI 侧（M6）实现。

### M1-1 [DONE] mag-service：pivot 契约新增

- **上下文**：`docs/CLI.md` §3.2（决策 D1 第一层）。只加不改。
- **实现要求**：
  - `MagService` 新增方法 `async fn pivot_message(&self, id: SessionId, input: UserInput)
    -> Result<(), ServiceError>`（带默认实现返回 `Err(ServiceError::Unsupported(..))`，保持下游实现者
    向后兼容；或按既有惯例不加默认实现——以 crate 内现有方法风格为准，二选一并在完成记录注明）。
  - `ServiceError` 新增变体 `NotPivotable { id: SessionId, reason: String }`（`#[non_exhaustive]` 下直接加）。
  - `ServiceEvent` 新增变体 `PivotQueued{id}`、`PivotApplied{id}`、`PivotDropped{id,reason}`；
    `ServiceEvent::session_id()` 覆盖新变体。
  - rustdoc 引用 `docs/CLI.md` §3.2 说明两层语义：本方法只负责第一层（turn 中途注入 pivot；无
    in-progress turn 返回 `NotPivotable`），第二层（调用方回落 `send_message`）由调用方实现。
- **验证条件**：`cargo test -p mag-service`；序列化/反序列化 roundtrip 单测覆盖三个新事件变体与新错误
  变体；默认验证序列全过。

  **完成记录**（2026-07-20）：
  - 实现要点：`MagService` 新增 `pivot_message(SessionId, UserInput) -> Result<(), ServiceError>`
    （置于 `cancel` 之后，「Conversation / runs」段）；`ServiceError` 新增
    `NotPivotable{id, reason}`（含 `Display`）；`ServiceEvent` 新增 `PivotQueued{id}` /
    `PivotApplied{id}` / `PivotDropped{id, reason}`（追加在 `LocalAgentsProbed` 之后），
    `session_id()` 三个新变体均返回 `Some(id)`。全部只加不改，`Command`/`Event` wire 协议未动。
  - 关键决策：trait 新方法**不带默认实现**——crate 内现有方法（`create_session`…
    `probe_local_agents`）全部为必需方法、无默认实现，且未支持能力由实现者返回
    `ServiceError::Unsupported`（见 `Engine::list_sources`/`probe_local_agents`），故按既有惯例
    二选一中的「无默认实现」，并同步更新全部实现者返回 `Unsupported{operation:"pivot_message"}`
    stub：mag-core `Engine`（真正实现属 M1-2）、mag-service 测试 `DummyService`、mag-acp 五个测试
    fake（`FakeService`/`ScriptedService`/`BridgeService`/`RoundService`/`CancelService`/
    `TwoRunService`）。
  - rustdoc：`pivot_message`、`NotPivotable`、`Pivot*` 变体均引用 `docs/CLI.md` §3.2（决策 D1），
    注明本方法只负责第一层（仅对 in-progress turn 注入；无则 `NotPivotable`），第二层
    （回落 `send_message`）由调用方实现。
  - 测试：三新事件变体纳入 roundtrip+稳定 tag 用例（`pivot_queued`/`pivot_applied`/
    `pivot_dropped`）；`session_id()` 覆盖三新变体；新增
    `not_pivotable_error_round_trips_and_displays`（tag `not_pivotable` + Display）与
    `pivot_message_is_callable_behind_arc_dyn`（object-safe 下默认 stub 返回 Unsupported）。
  - 门禁结果：1) `cargo fmt --all -- --check` ✅ 2) `cargo test -p mag-service` ✅（14 passed）
    3) `cargo clippy --all-targets -- -D warnings` ✅ 4) `cargo test --workspace` ✅（全绿，
    无失败）5) `cargo doc --no-deps --workspace` ✅。

### M1-2 [DONE] mag-core：driver pivot 队列旁路

- **上下文**：`docs/CLI.md` §3.2；agent-lib `AgentRunStream::interject()` 锚点（仅 step 边界窗口接受，
  InvalidState 失败无副作用、盲重试安全）。CancelHandle 同构模式见 driver.rs 现有 cancel 实现。
- **实现要求**：
  - Engine 实现 `pivot_message`：会话无 in-progress run → `Err(NotPivotable)`；有则入 pivot 队列并发
    `PivotQueued`。
  - driver `run_turn` 事件循环每次 poll 后 drain pivot 队列尝试 `stream.interject()`；`InvalidState`
    留队下次重试；接受则发 `PivotApplied`。
  - run 结束（含 cancel/出错）时队列仍有未落地 pivot → 逐条发 `PivotDropped{id,reason}`（reason 区分
    run 正常结束 / cancelled / error）。
  - pivot 文本作为 user message 注入对话历史（agent-lib interject 语义），与 `send_message` 的持久化路径
    对齐（消息须进 session 存储，重启可见）。
- **验证条件**：聚焦测试：fake LLM 下 (a) run 进行中 pivot 被接受且 `PivotQueued`→`PivotApplied` 顺序正确、
  pivot 文本进入后续 LLM 请求上下文；(b) 无 run 时 pivot 返回 `NotPivotable`；(c) run 恰好结束时 pivot 未
  落地发 `PivotDropped`；(d) pivot 后 session 持久化包含 pivot 消息。默认验证序列全过。

  **完成记录**（2026-07-21）：
  - 实现要点：`Engine::pivot_message` 落地——未知会话 `SessionNotFound`，否则经 `SessionManager` 路由到
    会话 actor；actor 以 `pivots: Option<PivotQueue>`（仅 run 在飞时为 `Some`）作为 in-progress 标记，
    无 run → `NotPivotable{id, reason:"no in-progress run"}`，有 run → 入队、发 `PivotQueued`（先于回复
    `Ok`，保证订阅者在 `pivot_message` 返回前必见 `PivotQueued`）。driver `run_turn` 每次
    `stream.next().await` 返回后 drain `PivotQueue` 尝试 `stream.interject()`：`Ok` → `PivotApplied`；
    `InvalidState`（窗口未开/已被占，无副作用、盲重试安全）→ 退回队首下次重试；其它错误 → 出队发
    `PivotDropped{reason:"pivot rejected: .."}`。run 结束（含 `stream_with_cancel` 立即失败路径）对残留
    队列逐条发 `PivotDropped`，reason 区分 finished/failed/cancelled，且先于 terminal 事件发出。
  - 前置缺口（按通用规则记录）：M1-1 只在 `ServiceEvent` 加了三个 `Pivot*` 变体，mag-core `EventBus`
    承载的 wire 枚举 `Event`（mag-service `lib.rs`）没有对应变体无法发事件——本任务按向后兼容方式补上
    `Event::{PivotQueued,PivotApplied,PivotDropped}`（追加变体 + `From<Event> for ServiceEvent` 投影 +
    roundtrip/稳定 tag 用例），`Command` 协议未动。
  - 关键设计：① 队列与 CancelHandle 共存方式——`PivotQueue`（`Arc<Mutex<VecDeque<String>>>`，锁不跨
    `.await`、poison 恢复）与 `CancelHandle` 同构：actor 持有克隆做入队，run 任务持有克隆在 poll 后
    drain，互不触碰 agent `&mut`；② 竞态兜底——run 任务在 `run_turn` 末尾 drain 一次后，actor 回收
    driver 时（`run_done` 通道载荷扩为 `(Box<SessionDriver>, TurnOutcome)`）用同一
    `outcome.pivot_drop_reason()` 再 drain 一次，吃掉「run 刚 drain 完 pivot 才入队」的缝隙，pivot
    绝不静默丢失；③ interject 重试策略——靠 agent-lib 窗口机制（tool step 后 sink 未 drain 完之前窗口
    保持开）在每次 poll 后盲试，`InvalidState` 留队；④ 持久化接入点——不加新路径：被接受的 pivot 经
    agent-lib pivot 语义成为 user message 进入对话历史，run 提交时由既有
    `persist_committed_snapshot` 快照落盘（与 `send_message` 完全同路径），未落地 pivot 不进历史。
  - 测试（全部离线，`engine::pivot` 模块，5 个）：(a)
    `pivot_mid_run_is_applied_at_the_step_boundary_and_enters_llm_context`——gate 卡住的 stub 工具使
    run 确定性地在飞，断言 `PivotQueued`→`PivotApplied` 顺序、无 `PivotDropped`、第二个 LLM 请求含
    pivot user message；(b) `pivot_without_an_in_progress_run_reports_not_pivotable`——idle 会话 /
    未知会话（`SessionNotFound`）/ 无 client 引擎三态；(c) 两个 drop 场景：
    `queued_pivot_is_dropped_when_the_run_is_cancelled`（stalling 流 + cancel，reason 含 cancelled）
    与 `queued_pivot_is_dropped_when_the_run_finishes_without_a_boundary`（新增 `StreamGate` 门控脚本
    使单 text step 无边界可注入，run 正常结束，reason 含 finished），均断言 `PivotDropped` 先于
    terminal 事件；(d) `applied_pivot_is_persisted_with_the_committed_snapshot`——`with_persistence`
    引擎落 pivot 后快照 JSON 含 pivot 与原始消息文本。test_support 新增 `StreamGate` 与
    `StreamScript::Gated`/`gated_text_stream`（与既有 `Stall` 同族的确定性门控 fixture）。聚焦测试连跑
    5 次无 flake，单测试 < 1s。
  - 门禁结果：1) `cargo fmt --all -- --check` ✅ 2) `cargo test -p mag-core pivot` ✅（5 passed）
    3) `cargo clippy --all-targets -- -D warnings` ✅ 4) `cargo test --workspace` ✅（全绿，无失败）
    5) `cargo doc --no-deps --workspace` ✅。

### M1-R [DONE] M1 review

- **实现要求**：通读 M1 全部 diff，对照 `docs/CLI.md` §3.2 检查：契约只加不改、事件顺序正确、pivot 与
  cancel 竞态（cancel 抢占后 pivot 必须 `PivotDropped` 而非丢失）、持久化对齐、rustdoc 完整。发现问题直接
  修复并补测试。
- **验证条件**：默认验证序列全过；完成记录列出 review 结论（发现/修复项）。

  **完成记录**（2026-07-21）：
  - review 范围：`git show e657fb0 9ee7141` 全 diff + 当前源码（`mag-service` service.rs/lib.rs、
    `mag-core` driver.rs/session.rs/engine.rs/test_support.rs），对照 `docs/CLI.md` §3.2（决策 D1）。
  - 检查点与结论：
    1. **契约只加不改** ✅——`MagService` 仅新增 `pivot_message`；`ServiceError`/`ServiceEvent`/wire
       `Event` 均只追加变体（`#[non_exhaustive]` 就位），既有方法签名、事件语义、既有 serde tag 未动；
       `Command` 协议未动。旧事件流可反序列化（tag 不变，roundtrip+稳定 tag 用例覆盖三新变体与
       `not_pivotable` 错误 tag）。
    2. **投影完整性** ✅——`From<Event> for ServiceEvent` 覆盖 `PivotQueued/Applied/Dropped` 三变体；
       `ServiceEvent::session_id()` 三变体均返回 `Some(id)`，故 `subscribe(Some(id))` 的按会话过滤
       对新事件天然正确。
    3. **事件顺序** ✅——actor 在回复 `Ok` 前先发 `PivotQueued`（订阅者必先见 Queued）；`drain_pivots`
       在每次 poll 后尝试 `interject()`，`InvalidState` 留队盲重试、接受发 `PivotApplied`；run 终态
       `drop_pivots` 在 terminal 事件**之前**发 `PivotDropped`（finished/failed/cancelled 三种 reason
       区分）。测试逐条断言 `PivotQueued→PivotApplied`、`PivotDropped < terminal` 顺序。已知的良性
       竞态：pivot 在 run 已发 terminal 之后才入队时，由 actor 回收 driver 时补发 `PivotDropped`
       （同一 terminal reason）——此时 `PivotDropped` 落在 terminal 事件之后属不可避免，关键不变量
       「每个 `PivotQueued` 必有恰好一个 `PivotApplied`/`PivotDropped` 收尾、绝不静默丢失」成立。
    4. **pivot 与 cancel 竞态** ✅——cancel 抢占 → facade 流以取消错误收场 →
       `TurnOutcome::Cancelled` → `drop_pivots`（reason 含 cancelled）先于 `RunError{Cancelled}`；
       actor 侧二次 drain 兜底同一 reason。`queued_pivot_is_dropped_when_the_run_is_cancelled`
       覆盖。
    5. **持久化对齐** ✅——接受的 pivot 经 agent-lib `interject()` 语义成为 user message 进对话历史，
       run 提交时走既有 `persist_committed_snapshot` 路径落盘（与 `send_message` 完全同路径）；
       未落地 pivot 不进历史。`applied_pivot_is_persisted_with_the_committed_snapshot` 断言快照 JSON
       同时含 pivot 与原始消息文本。
    6. **rustdoc** ✅（修复 1 项后）——`pivot_message`/`NotPivotable`/`Pivot*`/`PivotQueue`/
       `drain_pivots`/`drop_pivots`/`TurnOutcome::pivot_drop_reason` 均引用 `docs/CLI.md` §3.2 并准确
       描述两层语义与旁路机制。
  - 与 §3.2 设计稿的一处有意偏差（非问题）：§3.2 草图中 `Pivot*` 事件携带 `text: String`，M1-1 按
    TODO 任务规范实现为不携带 text（事件保持小、调用方自知所发文本）——以 TODO 任务定义为准，
    维持现状。
  - 发现与修复项（共 1 项，已修复）：wire `Event::PivotQueued` 的 rustdoc 中
    `[`ServiceEvent::PivotQueued`](crate::ServiceEvent::PivotQueued)` 显式链接目标冗余，
    `cargo doc` 报 `redundant explicit link target` 警告——已改为裸 intra-doc link，workspace
    文档构建警告归零。属纯文档修复，无需补测试。
  - 门禁结果（修复后全量重跑）：1) `cargo fmt --all -- --check` ✅ 2) `cargo test -p mag-core pivot`
    ✅（5 passed，连跑 3 次无 flake）+ `cargo test -p mag-service` ✅（14 passed）
    3) `cargo clippy --all-targets -- -D warnings` ✅ 4) `cargo test --workspace` ✅（18 个测试目标
    全 ok，无失败）5) `cargo doc --no-deps --workspace` ✅（0 warning）。

---

## Milestone M2 — 交互归因（`docs/CLI.md` §3.3，决策 D5）

目标：子 agent（delegate）产生的 `InteractionRequested` 统一 pop 到 root 会话事件流，并标注来源；
GUI/web/CLI 无需感知多个会话通道。

### M2-1 [DONE] mag-service：`InteractionOrigin` wire 类型 + 事件字段

- **上下文**：`docs/CLI.md` §3.3；agent-lib 已提供 `InteractionOrigin{delegate:Option<String>,
  depth:usize}`（子 agent 交互路由到父级注入 handler 时携带）。
- **实现要求**：
  - 新增 wire 类型 `InteractionOrigin{delegate:Option<String>, depth:u32}`（serde 完整，rustdoc 注明
    `None` = root 会话自身产生）。
  - `ServiceEvent::InteractionRequested` 新增字段 `#[serde(default)] origin: InteractionOrigin`
    （default = root，向后兼容旧事件流与消费者）。
- **验证条件**：序列化兼容单测（无 origin 字段的旧 JSON 可反序列化、新 JSON 含 origin）；默认验证序列全过。

  **完成记录**（2026-07-20）：
  - 实现要点：`crates/mag-service/src/lib.rs` 新增 wire 类型
    `InteractionOrigin{delegate:Option<String>, depth:u32}`（derive `Default`，root =
    `delegate:None, depth:0`；`delegate` 按 crate 惯例 `#[serde(default,
    skip_serializing_if = "Option::is_none")]`，`depth` 带 `#[serde(default)]` 容错），附
    `is_root()` 访问器；`ServiceEvent::InteractionRequested` 与 wire `Event::InteractionRequested`
    **两个枚举同步**新增 `#[serde(default)] origin: InteractionOrigin`（M1-2 教训：双枚举都要动），
    `From<Event> for ServiceEvent` 投影同步携带 origin。全部只加不改，`Command` 协议未动。
  - 关键决策：① origin 字段**不是** `Option<InteractionOrigin>`——`docs/CLI.md` §3.3 草稿写的是
    `Option<..>`（`None` = root），TODO 任务规范定为裸类型 + `#[serde(default)]`（default =
    root），以 TODO 为准：少一层嵌套、serde 语义等价（旧事件无字段 → default root）；② 草稿里的
    `agent: AgentIdWire` 字段不取——Permission 类交互本就有 `actor` 承载权限语义主体，origin 只做
    渲染归属（§3.3 原文「origin 与 actor 互补」）；③ depth 用 `u32`（agent-lib 侧是 `usize`，
    wire 收窄为定宽类型，M2-2 映射时转换）；④ `IpcApproval` 发事件处（mag-core
    `engine/approval.rs`）暂填 `InteractionOrigin::default()` 并注明 M2-2 接线真实 origin——本任务
    只做契约，子 agent origin 映射属 M2-2。
  - rustdoc：`InteractionOrigin`、两个 `InteractionRequested` 变体的 `origin` 字段、
    `ServiceEvent::InteractionRequested` 变体级文档均引用 `docs/CLI.md` §3.3（决策 D5），注明
    `delegate:None` = root 会话自身产生、与 Permission `actor` 的互补关系。
  - 下游编译适配（纯机械，不改语义）：mag-core `IpcApproval` emit 处补 `origin` 字段；mag-core
    `engine.rs` 两处与 `e2e_offline.rs` 一处穷尽模式补 `..`；mag-acp 三个测试文件
    （`e2e_acp.rs`/`permission_bridge.rs`/`cancel.rs`）共四处 `InteractionRequested` 字面构造补
    `origin: InteractionOrigin::default()`。
  - 测试（全部离线）：mag-service 新增 3 个——`interaction_origin_defaults_to_root_and_round_trips`
    （default = root、`is_root()`、delegated/default 双 roundtrip、裸 `{depth:0}` 解码回 root）；
    `interaction_requested_without_origin_deserializes_as_root`（lib.rs 与 service.rs 各一：无
    `origin` 键的旧 JSON → default root）。既有 roundtrip 用例中 `ServiceEvent::InteractionRequested`
    改带 delegated origin（`codex@depth1`）覆盖新字段 roundtrip；投影用例新增
    `Event::InteractionRequested → ServiceEvent::InteractionRequested`（delegated origin）一条，
    验证投影完整。`cargo test -p mag-service` 17 passed。
  - 门禁结果：1) `cargo fmt --all -- --check` ✅ 2) `cargo test -p mag-service` ✅（17 passed）
    3) `cargo clippy --all-targets -- -D warnings` ✅ 4) `cargo test --workspace` ✅（18 个测试
    目标全 ok，无失败）5) `cargo doc --no-deps --workspace` ✅。

### M2-2 [DONE] mag-core：子 agent 交互 origin 映射

- **上下文**：`docs/CLI.md` §3.3；mag-core 把 agent-lib 路由来的子 agent 交互（approval / question /
  choice / permission）统一经 root 会话的 `ServiceEvent::InteractionRequested` 发出。
- **实现要求**：
  - driver/approval 路径把 agent-lib `InteractionOrigin` 映射到 wire `InteractionOrigin`；
    root 会话自身交互 origin 为 default。
  - `respond_interaction` 按 `RequestId` 回灌时正确路由回发起交互的 delegate（RequestId 唯一性保证在
    mag-core 侧，不因多 delegate 并发交互而串号）。
  - 事件渲染不丢信息：origin 有 delegate 时 trace/日志含 delegate 名与 depth。
- **验证条件**：聚焦测试：两级 delegate 场景（fake LLM 驱动 subagent 触发审批），断言 root 订阅者收到
  一条 `InteractionRequested` 且 `origin.delegate == Some(..)`、`depth == 1`；`respond_interaction` 后
  正确的 delegate 恢复执行；无 delegate 时 origin 为 default。默认验证序列全过。

  **完成记录**（2026-07-21）：
  - 实现要点：`IpcApproval::emit_and_await` 把 M2-1 的 `InteractionOrigin::default()` 占位替换为
    `interaction_origin_to_wire(request.origin())`——新增映射函数把 agent-lib 路由层标注的
    `Option<&agent_lib::agent::InteractionOrigin>` 投影到 wire `InteractionOrigin`：`None`（root 会话
    自身交互）→ wire default（`delegate:None, depth:0`）；`Some(o)` → `delegate:Some(o.delegate),
    depth:o.depth`。全部改动集中在 `crates/mag-core/src/engine/approval.rs`；driver/session/engine 路径
    无需变动——注入点本就是「每会话一个共享 `IpcApproval`」，agent-lib 的 `ChildInteractionRouter`
    自动把任意深度 delegate 的暂停交互转发到这同一实例。
  - 关键设计：① **origin 传递链**——agent-lib `facade/delegate.rs` 的 `ChildInteractionRouter::fulfill`
    在转发前 `request.clone().with_origin(InteractionOrigin::new(delegate, ctx.depth()))` 标注
    （external 路径同构），父级注入 handler（即 `IpcApproval`）从 `Interaction::origin()` 读取；
    实测 agent-lib `InteractionOrigin{delegate: String, depth: u32}` 与 `RunContext::depth() -> u32`
    **均为 u32**（任务书按旧文档写的 usize→u32 收窄不需要，直接拷贝），并顺手修正 mag-service
    `InteractionOrigin` rustdoc 中「agent-lib uses usize」的不准确表述（纯文档）；② **RequestId 路由
    保证**——会话内全部交互（root + 任意深度的全部 delegate）都汇入同一个 `IpcApproval` 实例，id 由
    其实例级 `RequestIdSource` 单调铸造，天然唯一；`respond` 按 id 查 pending map，命中项存储的
    oneshot 精确唤醒发起该请求的那次 parked `fulfill`，应答经 `ChildInteractionRouter` 回到正确的
    delegate——多 delegate 并发暂停不可能串号。已在 `IpcApproval` 类型级与 `respond` rustdoc 中
    记录该保证；③ **渲染不丢信息**——wire `Event::InteractionRequested` 即 mag-core 的渲染/trace
    载体（crate 内无独立日志管道），origin 字段携带 delegate 名与 depth，消费者（CLI M6-2 的
    `[from <delegate>@depth<n>]` 前缀、未来 GUI）直接取用。
  - 测试（全部离线，`engine::approval` 模块，新增 4 个 + 1 处既有断言增强）：(a)
    `delegate_interaction_pops_to_root_with_origin_and_resumes_on_response`——两级 delegate 集成场景：
    fake LLM 顺序脚本驱动 supervisor `ask_reviewer` 委派 → reviewer 调 gated `shell` 暂停 → 断言 root
    订阅者收到 `InteractionRequested` 且 `origin.delegate == Some("reviewer")`、`depth == 1`；
    `respond`(Deny) 后 delegate 恢复执行（4 条脚本全部消费、子级同步 fallback decider 探针未被
    调用——证明由 root handler 应答）；delegate 装配走 agent-lib 公共 API
    （`Agent::worker()` + `AgentBuilder::subagent`），成本低于预期，未走降级路径；(b)
    `delegated_interaction_origin_surfaces_on_the_wire_event`——直接 fulfill 带 origin 标注的
    Question 交互，断言事件 origin 与应答回灌；(c)
    `concurrent_delegate_interactions_route_each_response_to_its_own_waiter`——两个 delegate
    （depth 1/2）并发暂停，乱序应答（先 b 后 a），断言各自 waiter 收到各自 decision 且
    step_id/call_id 归位、id 互不相同；(d) `interaction_origin_maps_to_wire`——映射函数单测
    （None→default、Some→delegate/depth）；(e) root 场景断言增强——既有
    `pause_then_approve_runs_the_tool` 补 `origin == InteractionOrigin::default()` / `is_root()`
    断言（验证条件 c）；`drive_until_interaction` 帮助函数返回 `(RequestId, InteractionOrigin)`，
    三个既有调用点同步解构。聚焦测试连跑 3 次无 flake，单测试 < 1s。
  - 门禁结果：1) `cargo fmt --all -- --check` ✅ 2) `cargo test -p mag-core approval` ✅（17 passed，
    连跑 3 次无 flake）3) `cargo clippy --all-targets -- -D warnings` ✅ 4) `cargo test --workspace`
    ✅（全绿，无失败）5) `cargo doc --no-deps --workspace` ✅（0 warning）。

### M2-R [DONE] M2 review

- **实现要求**：对照 `docs/CLI.md` §3.3 检查：向后兼容（旧消费者不感知 origin 仍可用）、多 delegate
  并发交互不串号、rustdoc 完整。发现问题直接修复并补测试。
- **验证条件**：默认验证序列全过；完成记录列出 review 结论。
- 完成记录：
  - **review 结论：通过，无问题，无修复项。** 通读 b092573（M2-1）与 01da54f（M2-2）全部 diff 并
    对照 `docs/CLI.md` §3.3 决策 D5 逐项核查。
  - 检查点 ① **向后兼容**：`origin` 在 wire `Event::InteractionRequested` 与 `ServiceEvent::
    InteractionRequested` 双枚举上均为裸 `InteractionOrigin` + `#[serde(default)]`（default =
    root：`delegate: None, depth: 0`）；旧 JSON（无 origin 键）反序列化为 root——两个枚举各有
    专项测试（`interaction_requested_without_origin_deserializes_as_root`）；旧消费者读新 JSON
    时 serde 默认忽略未知字段，不感知 origin 仍可用；`delegate` 带 `skip_serializing_if`，root
    序列化为 `{"depth":0}` 且可解回 default（round-trip 测试覆盖）。§3.3 原文写
    `Option<InteractionOrigin>`「之类」，实现取裸类型 + Default，语义等价（`None` ≈
    `delegate: None`），rustdoc 已记录该取舍，不算偏差。
  - 检查点 ② **双枚举一致性**：两枚举同步加字段、`From<Event> for ServiceEvent` 透传 origin，
    service.rs 转换测试逐项断言。
  - 检查点 ③ **多 delegate 并发不串号（RequestId 路由）**：生产路径 `driver.rs` `new`/`restore`
    均把会话级唯一 `Arc<IpcApproval>` 注入 agent 的 `interaction_handler`，委派链全部交互汇入同一
    实例；id 由实例级 `RequestIdSource` 单调铸造，pending map 按 id 存 oneshot 精确唤醒。测试
    `concurrent_delegate_interactions_route_each_response_to_its_own_waiter` 双 delegate
    （depth 1/2）并发暂停、乱序应答，断言各自 waiter 收到各自 decision 且 step_id/call_id 归位。
  - 检查点 ④ **origin 映射正确性**：`interaction_origin_to_wire`——`None`→default（root）、
    `Some`→delegate+depth；两侧 depth 均为 `u32`（M2-2 已顺手修正 M2-1 rustdoc 中「agent-lib
    uses usize」的不准确表述）；单测 + 两级 delegate 集成测试（`Agent::worker()` +
    `AgentBuilder::subagent` 走 agent-lib 公共 API）断言 root 订阅者收到
    `origin.delegate == Some("reviewer")`、`depth == 1`，root 场景既有测试补强
    `origin == default` / `is_root()` 断言。
  - 检查点 ⑤ **rustdoc 完整**：模块级 D5 说明、`IpcApproval` 类型级路由保证、`respond` 路由
    保证、wire 类型与字段级文档齐备；`cargo doc` 0 warning。
  - 备注（非缺陷）：`InteractionOrigin::is_root()` 仅判 `delegate.is_none()`，理论上
    `delegate: None, depth > 0` 会误判 root，但映射函数不可能产生该组合，接受。
  - 门禁结果：1) `cargo fmt --all -- --check` ✅ 2) `cargo test -p mag-service` ✅（17 passed）
    + `cargo test -p mag-core approval` ✅（17 passed）3) `cargo clippy --all-targets --
    -D warnings` ✅（touch 后强制重检，非缓存）4) `cargo test --workspace` ✅（全绿，1 ignored
    为既有 `#[ignore]` 联调测试）5) `cargo doc --no-deps --workspace` ✅（touch 强制重建 M2 涉及
    两 crate，0 warning）。

---

## Milestone M3 — 运行时配置系统（`docs/CLI.md` §4，决策 D2/D4，重点）

目标：配置不是一次读入的只读快照——DTO（配置文件/任意来源）↔ DO（程序实际使用的 `Arc` 对象树）双向
转换；运行时可更新（`update_config` / 文件 watch `reload`），在合适时机生效（会话创建钉住 +
`apply_config` 到 turn 边界）；同一套系统后续直接服务 GUI/web。

### M3-1 [DONE] 新 crate `mag-config`：DTO + TOML 读写 + 校验

- **上下文**：`docs/CLI.md` §4.1/§4.2（TOML 示例在 §4.2 代码块）。DTO 对应配置文件或任何可能的配置
  来源（决策 D4：不与「配置文件」绑死）。
- **实现要求**：
  - 新建 `crates/mag-config`，加入 workspace members；`#![warn(missing_docs)]`。
  - DTO serde 类型覆盖 §4.2 全部配置面：`[llm.<name>]`（provider/model/api_key 引用/endpoint/参数表）、
    `[tools]`（profile、逐工具开关/策略）、`[[agent]]`（name、role、llm 引用、tools 引用、system prompt、
    budget）、`[[external_agent]]`（name、kind=acp、command/args/env、capabilities）、`[session]`
    （默认 budget、persist 路径）、`[approval]`（默认策略、超时）等——以 §4.2 示例为最小完备集，
    字段命名与示例一致；全部字段 `Option`/`Default` 友好（部分配置合法）。
  - secret 字段只接受引用形态（`{env=VAR}` / `{keyring=NAME}`，字符串 mini-DSL 按 §4.2），提供
    `SecretRef` 解析类型（解析出值是后续 DO/运行时的事，DTO 层只保引用）。
  - TOML 读写：`parse_str`/`to_string_pretty`/`load(path)`/`save_atomic(path)`（写临时文件 + rename，
    保证 watch 端不读到半截文件）；行级校验错误（带 line/col 与字段路径，`ConfigError` 枚举）。
  - 轮次 trip 单测：§4.2 示例 TOML → DTO → TOML 语义等价。
- **验证条件**：聚焦测试 `cargo test -p mag-config`；默认验证序列全过。

  **完成记录**（2026-07-22）：
  - 实现要点：新建 `crates/mag-config`（已加入 workspace members，`#![warn(missing_docs)]`），
    四个模块：`dto.rs`（DTO serde 类型）、`secret.rs`（`SecretRef`）、`error.rs`（`ConfigError`）、
    `io.rs`（TOML 读写 + `validate`）。根类型 `ConfigDto{providers, agents, external_agents, tools,
    session, approval}`——全部 name-keyed map 用 `BTreeMap`（序列化确定性），全部字段
    `Option` + `skip_serializing_if`（部分配置合法、round-trip 不长幽灵节）。`ProviderDto{wire,
    base_url, api_key: SecretRef, params}`、`AgentDto{provider, model, tools, system_prompt, role,
    budget}`、`ExternalAgentDto{kind, command, env, capabilities}`、`ToolDto{approval, enabled}`、
    `SessionDefaultsDto{routing, persist_path, budget}`、`ApprovalSectionDto{default_policy,
    timeout_secs}`、`BudgetDto{max_steps, max_tokens, max_cost_micros, max_wall_time_secs}`。
    TOML IO：`parse_str`/`to_string_pretty`/`load`（读+解析+validate）/`save_atomic`（同目录
    `.<name>.tmp-<pid>` 临时文件 + write_all + sync_all + rename，失败清理临时文件，父目录按需
    创建）。`ConfigError`（thiserror，`#[non_exhaustive]`）：`Io{path,source}` /
    `Parse{path,line,col,message}`（byte span → 1-based line/col）/ `Serialize{message}` /
    `Validation{path,message}`（字段路径如 `agents.default.tools[2]`）。
  - 关键决策：① **字段命名以 §4.2 实际示例为准**——TODO 任务书列举的 `[llm.<name>]`/
    `[[agent]]`/`[[external_agent]]`/`[approval]` 与 docs/CLI.md §4.2 示例（`[providers.<name>]`/
    `[agents.<name>]`/`[external_agents.<name>]`/`[tools.<name>]`/`[session]`）不一致，按任务书
    自身「字段命名与示例一致、§4.2 示例为最小完备集」的要求取后者（§4.2 的 DTO↔DO 结构图同）；
    任务书提到但示例没有的字段（agent role/system_prompt/budget、external env/capabilities、
    session persist_path、provider 参数表 params、`[approval]` 段）作为 Option 字段补齐，即
    「示例最小完备集 ∪ 任务书列举面」。② 根类型命名 `ConfigDto`（对齐 M3-4 契约方法签名
    `get_config() -> ConfigDto`），rustdoc 注明对应 §4.2 图中的 `ConfigFileDto`。③ **enum 类字符串
    保持 String**（`wire`/`kind`/`approval`/`routing`/`default_policy`）——§4.2 明确把「approval
    枚举合法」归入 DTO→DO resolve 校验，DTO 层保持宽松以支持部分配置；M3-2 resolve 时校验并报
    带路径的错。④ `SecretRef` 双形态：反序列化接受 §4.2 内联表 `{env=...}`/`{keyring=...}`
    （canonical）与字符串 mini-DSL `"env:VAR"`/`"keyring:NAME"`（服务决策 D4 的非 TOML 来源）；
    序列化恒为 canonical 表形态；裸 secret 值（无前缀字符串、双 key、未知 key、空名）一律拒绝，
    杜绝 secret 内联落盘；`Display` 输出 §4.3 `ConfigView` 脱敏字样 `{env = "VAR"}`。⑤
    `BudgetDto` 四字段逐一对齐 mag-service `SessionBudget`（max_steps/max_tokens/max_cost_micros/
    max_wall_time_secs），M3-2 转换为纯字段投影。⑥ `validate()` 只做结构校验（空节名/空字符串/
    重复 tool 名/空 argv/零 budget/零 timeout），交叉引用与枚举校验留给 resolve；`load` 自动跑
    validate，`parse_str` 保持纯 serde（GUI patch 等来源可构造中间态）。⑦ struct 字段顺序
    标量在前、表在后（`api_key`/`params`/`budget` 置尾），规避 TOML ValueAfterTable 序列化限制。
  - 测试（全部离线，tempdir；4 个单元 + 23 个集成，共 27 个）：§4.2 示例**逐字符** TOML →
    DTO 字段断言（含两个 secret 引用形态、external command、tools.shell.approval、session
    budget）→ `to_string_pretty` → 重解析 `PartialEq` 语义等价；序列化输出含全部节头且 secret
    保持引用形态；JSON roundtrip（D4 GUI 来源）；空文档 = default；部分配置合法且 roundtrip
    无幽灵节；SecretRef 表/字符串双形态接受 + 5 类畸形拒绝 + 裸值拒绝 + 序列化恒 canonical；
    tempdir 下 save_atomic/load roundtrip、覆盖写、无临时文件残留、缺文件 Io 错带路径、parse
    错带 line/col（line=3/2 逐例断言）、load 自动 validate；validate 各失败路径字段路径断言
    （`agents.default.tools[2]` 等）。
  - 依赖边界：`cargo tree -p mag-config -e normal --depth 1` 仅 serde/thiserror/toml——无
    agent-lib/mag-core/mag-service，硬约束满足。
  - 门禁结果：1) `cargo fmt --all -- --check` ✅ 2) `cargo test -p mag-config` ✅（27 passed）
    3) `cargo clippy --all-targets -- -D warnings` ✅ 4) `cargo test --workspace` ✅（全绿，
    1 ignored 为既有 `#[ignore]` 联调测试）5) `cargo doc --no-deps --workspace` ✅（0 warning）。

### M3-2 [DONE] mag-config：DTO ↔ DO 双向转换

- **上下文**：`docs/CLI.md` §4.2（决策 D4）。DO 对应程序实际使用的配置对象；由于动态生效需求，DO 不是
  简单嵌套 struct，而是**引用计数的关联对象树/森林**——会话钉住 `Arc<ConfigSnapshot>` 后，后续
  `update_config` 产生新快照，旧快照对存活会话保持不变。
- **实现要求**：
  - DO 类型：`ConfigSnapshot`（`Arc` 内部不可变对象树：`LlmConfig`/`ToolConfig`/`AgentConfig`/
    `ExternalAgentConfig`/`SessionDefaults`/`ApprovalConfig` 等节点均 `Arc` 共享，revision: u64）；
    节点提供访问器；`ConfigSnapshot` 整树 `Clone`（廉价，`Arc` 拷贝）。
  - 双向转换：`ConfigDto -> ConfigSnapshot`（校验+归一化：引用解析为结构、默认填充、交叉引用检查——
    agent 引用的 llm/tools 名必须存在，报带路径的 `ConfigError`）；`ConfigSnapshot -> ConfigDto`
    （无损回写，secret 引用形态保持引用不物化）。
  - 转换测试：DTO→DO→DTO roundtrip 等价；非法交叉引用报错路径正确。
- **验证条件**：聚焦测试 `cargo test -p mag-config`；默认验证序列全过。

  **完成记录**（2026-07-22）：
  - 实现要点：新增 `crates/mag-config/src/snapshot.rs`（DO 层），lib.rs 补 re-export 与 crate 文档
    「Layering」「Entry points」更新。`ConfigSnapshot{revision: u64, providers/agents/external_agents/
    tools: BTreeMap<String, Arc<..>>, session_defaults/approval: Option<Arc<..>>}`——整树 `Clone`
    只拷 `Arc` 句柄，节点构建后不可变；`resolve(&ConfigDto, revision)` 为 DTO→DO（先跑
    `ConfigDto::validate()` 结构校验 → 语义校验 → 按拓扑序实例化 Arc 节点：providers/tools 先于引用
    它们的 agents → 打 revision 戳；任一步失败整体失败，不产出半个图）；`project()` 为 DO→DTO 无损
    回写（revision 属 DO 元数据不回投）。节点访问器齐备（`revision()`/`provider(name)`/`agent(name)`/
    `tools()`/`session_defaults()`/`approval()` 等）。
  - 关键决策：① **DO 命名以 §4.2 结构图为准**——图里明确写的是 `Arc<ResolvedProvider>`/
    `Arc<ResolvedAgent>`（`ResolvedConfig / ConfigSnapshot` 根），一致扩展为 `ResolvedExternalAgent`/
    `ResolvedTool`；任务书列举的 `LlmConfig`/`AgentConfig`/`ExternalAgentConfig`/`ToolConfig` 对应
    关系在 snapshot.rs 模块级 rustdoc 用对照表注明（`SessionDefaults`/`ApprovalConfig` 两侧同名）。
    ② **raw + effective 双层访问器**实现「默认填充且无损回写」：节点保留 DTO 原始 `Option` 字段
    （未设置的键投影后仍未设置，无幽灵节），默认值经 `is_enabled()`（默认 true）/
    `effective_routing()`（默认 `ModelRouted`，对齐 mag-service `RoutingMode::default()`）/
    `effective_default_policy()`（默认 `Allow`，对齐 agent-lib `ApprovalPolicy::default()` 的
    auto_allow 层）/`effective_kind()`（默认 `Acp`）访问器填充。③ **工具引用宽松解析（有意偏离
    任务书「tools 名必须存在」字面）**：`agents.<name>.tools` 引用的工具名不要求有 `[tools.<name>]`
    条目——§4.2 示例的 `read_file`/`list_dir`/`grep`/`ask_user` 均无对应条目，且 §4.2 resolve 一节
    只把「悬空 provider 引用」列为报错项；无条目的名字解析为按名共享的合成默认 `ResolvedTool` 节点
    （implicit，多 agent 引用同名工具共享同一 Arc），不进入快照 `tools` map，投影时不回现（roundtrip
    无损）；工具名对真实 tool registry 的存在性检查属装配层（M3-6 `Engine::from_config`）职责。
    ④ `session`/`approval` 节在快照内为 `Option<Arc<..>>` 以保留「节是否存在」信息（显式空表与
    缺失在投影时可区分）；节缺失时访问器返回节点上的 `const EMPTY`。⑤ 枚举类型 DO 侧落地为
    `ProviderWire{Anthropic,OpenAi}`（协议集对齐 agent-lib adapter 实现的 Anthropic Messages /
    OpenAI Responses）、`ApprovalPolicyKind{Ask,Allow,Deny}`、`RoutingModeKind{ModelRouted,
    Dispatcher}`（字符串形态对齐 mag-service serde 名）、`ExternalAgentKind{Acp}`，均带
    `as_str`/`Display`/`FromStr`（错误信息列合法值）。⑥ `provider.wire` 在 resolve 时必填——无协议
    的 provider 无法装配，DTO 层 rustdoc 本就声明协议集在 resolve 校验。⑦ `Budget` 为 `BudgetDto`
    的逐字段投影（双向 `From`）。
  - resolve 校验规则清单（全部报 `ConfigError::Validation`，带点分字段路径）：结构校验
    （`validate()` 先行，M3-1 已有规则原样生效）；`providers.<name>.wire` 缺失/未知值；
    `agents.<name>.provider` 悬空引用（报 `unknown provider "<name>"`）；`tools.<name>.approval`、
    `approval.default_policy` 非法策略枚举；`session.routing` 非法路由枚举；
    `external_agents.<name>.kind` 非法 kind。
  - 测试（全部离线；src 单元 3 个 + tests/snapshot.rs 集成 9 个，crate 总计 39 个）：§4.2 示例
    逐字符 TOML resolve 成共享 Arc 图（`Arc::ptr_eq` 断言 agent→provider、agent→显式 tool override、
    implicit tool 跨 agent 同名共享三处共享关系；implicit 节点不进 tools map）；DTO→DO→DTO
    `PartialEq` 无损 roundtrip + 投影 DTO 序列化后 secret 仍为引用形态（`env = "ANTHROPIC_API_KEY"`/
    `keyring = "mag/local_proxy"` 字样断言）且重解析等价；悬空 provider 报 `agents.reviewer.provider`；
    四类非法枚举 + wire 缺失/未知逐例断言路径与消息；结构校验先于语义校验（空字符串 model 报
    `agents.default.model`）；空 DTO → 空快照 + effective 访问器默认值 + 投影回 `ConfigDto::default()`；
    默认填充不产生幽灵投影（raw 字段保持未设置）；快照隔离（rev1 钉住句柄在 rev2 改 base_url/model/
    approval 后原值不变、revision 各自正确、整树 Clone 后节点 Arc 同一）。
  - 依赖边界：`cargo tree -p mag-config -e normal --depth 1` 仍仅 serde/thiserror/toml——硬约束满足。
  - 门禁结果：1) `cargo fmt --all -- --check` ✅ 2) `cargo test -p mag-config` ✅（39 passed）
    3) `cargo clippy --all-targets -- -D warnings` ✅ 4) `cargo test --workspace` ✅（24 个测试
    目标全 ok，1 ignored 为既有 `#[ignore]` 联调测试）5) `cargo doc --no-deps --workspace` ✅
    （touch 强制重建 mag-config，0 warning）。

### M3-3 [DONE] mag-core：`ConfigService`

- **上下文**：`docs/CLI.md` §4.3。
- **实现要求**：
  - mag-core 新增 `config` 模块：`ConfigService` 持有 `RwLock<Arc<ConfigSnapshot>>` + 配置文件路径 +
    revision 计数；`current() -> Arc<ConfigSnapshot>`（廉价克隆）。
  - `update(dto) -> Result<Arc<ConfigSnapshot>, ConfigError>`：DTO→DO 转换 → 原子写配置文件
    （write-through，save_atomic）→ 替换 RwLock 内快照 + revision+1 → 发 `ConfigChanged`（经 M3-4 的
    事件通道）。写盘失败则不换快照（内存与文件一致优先，报错）。
  - `reload() -> Result<Arc<ConfigSnapshot>, ConfigError>`：重读文件 → 校验 → 换快照 → 发事件；
    文件损坏时保留当前快照并报错（不崩）。
  - 文件 watch（`notify` crate；若离线环境引入失败，降级为仅显式 `reload`，在完成记录注明）：去抖
    （~300ms）+ 自身 write-through 写入去重（记录自写 revision/mtime 指纹，避免自写触发 reload 回环）。
  - watch 触发的自动 reload 失败只记 warn 日志，不发错误事件、不换快照。
- **验证条件**：聚焦测试（用 tempdir 配置文件）：update 后 current() 返回新快照且文件落盘；reload 拾取
  外部手改；损坏文件 reload 报错且快照不变；自写不触发回环（若 watch 启用）。默认验证序列全过。

  **完成记录**（2026-07-22）：
  - 实现要点：新增 `crates/mag-core/src/config.rs`（lib.rs `mod config;` + re-export
    `ConfigService`/`ConfigChange`；模块 rustdoc 引用 `docs/CLI.md` §4.3、决策 D2/D4）。`ConfigService`
    持有 `RwLock<Arc<ConfigSnapshot>>`（std RwLock、poison 恢复、锁不跨 `.await`，与 `PivotQueue`
    同纪律）+ write-through 文件路径 + `broadcast::Sender<ConfigChange>`；revision 计数直接住在快照
    内（`ConfigSnapshot::revision`），不与 DO 图分离，初始快照为 0、每次成功应用 +1。mag-core 新增
    path 依赖 `mag-config`（合法方向：core → config 纯数据 crate）。API 全同步（唯一阻塞操作是小
    文件 I/O，无 `.await` 持锁问题）：`load_or_default(path)` / `current()`（廉价 Arc 克隆即会话
    钉住）/ `revision()` / `path()` / `subscribe()` / `update(dto)` / `reload()`。
  - `update(dto)` 管线：resolve（含 validate）→ `save_atomic` write-through → 换快照 + revision+1 →
    广播 `ConfigChange`。写盘失败不换快照（内存与文件一致优先，报错）；resolve 失败文件与快照都不动。
  - `reload()` 管线：`ConfigDto::load` → resolve → 换快照 + revision+1 → 广播；文件缺失/损坏/语义非法
    均报错且保留当前快照（不崩），文件修复后 reload 自然恢复。
  - 关键决策：① **信号机制选型 = `tokio::sync::broadcast`**——载荷 `ConfigChange{revision,
    snapshot: Arc<ConfigSnapshot>}`（容量 16，Lagged 订阅者可重读 `current()`，最新状态恒权威）；
    多订阅者、lag-tolerant、与既有 `EventBus` fan-out 同构，M3-4 据此桥接
    `ServiceEvent::ConfigChanged{revision}`，比回调注册表更适合 service 层转发；无订阅者时发送为
    no-op（镜像 `EventBus::emit`）。② **watch 降级为仅显式 reload**——`cargo add notify` 清单层成功
    但 `cargo fetch --offline` 失败（crate 文件不在本地 registry 缓存，拉取需网络），按任务书降级；
    自写回环防护（去抖 ~300ms + 自写指纹去重）随之不需要——仅显式 reload 时回环不可能发生。模块
    rustdoc 记录了降级原因与 watcher 落地时的回环防护要求。③ **写者串行化**——`update`/`reload`
    全程持写锁（resolve→落盘→换根），revision 赋值、文件内容、内存快照三者天然一致，无并发交错。
    ④ `load_or_default`：文件不存在 → 内置默认空快照（`ConfigDto::default()` resolve，revision 0，
    不报错、不建文件，验证原型友好，bin 层 M3-6 用）；文件存在但损坏 → **报错**（§4.2「装配失败
    给诊断，不静默降级」）。
  - 测试（全部离线，`config::tests`，12 个，tempdir 配置目录复用 `TempDb` 同款
    pid+nanos+counter+Drop 模式）：缺文件 → 默认空快照且不建文件；已有文件 → 加载且
    agent→provider Arc 共享；启动即损坏 → `Parse` 错；update 换快照 + 文件落盘逐字节等价 + 旧钉住
    快照不变；写盘失败（父路径被普通文件占用）→ `Io` 错、快照不换、无信号；非法 DTO（悬空 provider
    引用）→ `Validation` 错带 `agents.default.provider` 路径、文件与快照不动；reload 拾取外部手改
    （model 变更 + 新增 `[tools.shell]`）；损坏文件 reload → `Parse` 错、快照不换、修文件后恢复；
    语义非法文件 reload → `Validation` 错、快照不换；订阅者按序收到 update/reload 两条
    `ConfigChange`（revision 1/2 递增、载荷快照与 `current()` 同一 Arc、失败操作不发信号）；
    并发读者只见到旧/新完整快照。聚焦测试连跑 3 次无 flake，单测试 < 1s。
  - 门禁结果：1) `cargo fmt --all -- --check` ✅ 2) `cargo test -p mag-core config` ✅（12 passed，
    连跑 3 次无 flake）3) `cargo clippy --all-targets -- -D warnings` ✅ 4) `cargo test --workspace`
    ✅（全绿，1 ignored 为既有 `#[ignore]` 联调测试）5) `cargo doc --no-deps --workspace` ✅
    （0 warning）。

### M3-4 [DONE] mag-service：配置方法 + `ConfigChanged` 事件

- **上下文**：`docs/CLI.md` §4.3/§5。wire 形态：配置内容本身用 DTO 的 JSON 形态（serde 已就位），
  不在 mag-service 重复定义一套配置 wire 类型——`mag-service` 依赖 `mag-config` 取 DTO 类型（纯数据
  crate，依赖方向合法：service 层定义契约，config DTO 是契约数据）。
- **实现要求**：
  - `MagService` 新增四方法（风格与 M1-1 决策一致）：
    `get_config() -> ConfigDto`、`update_config(ConfigDto) -> Result<(), ServiceError>`、
    `reload_config() -> Result<(), ServiceError>`、`apply_config() -> Result<(), ServiceError>`。
    rustdoc 注明生效时机语义（决策 D2）：update/reload 立即换快照但**不影响已钉住会话**；
    `apply_config` 请求把当前快照在**各会话下一个 turn 边界**应用（经 turn-complete 机制，M3-5）。
  - `ServiceEvent` 新增变体 `ConfigChanged{revision: u64}`（全局事件，`session_id() -> None`）。
  - `ServiceError` 如需新增 `Config{message}` 变体承载配置错误。
- **验证条件**：`cargo test -p mag-service`；事件序列化 roundtrip；默认验证序列全过。

  **完成记录**（2026-07-22）：
  - 实现要点：mag-service 新增 path 依赖 `mag-config`（合法方向：service → config 纯数据 crate，
    `cargo tree` 确认无 agent-lib/mag-core 反向依赖），lib.rs re-export `ConfigDto` 使其成为契约
    类型（mag-cli 只允许依赖 mag-service，`/config show` 渲染经 re-export 取型）。`MagService` 新增
    「Runtime configuration」段四方法（置于 `probe_local_agents` 之后）：`get_config() ->
    Result<ConfigDto, ServiceError>`（当前快照投影回 DTO，secret 保持引用形态）、
    `update_config(ConfigDto)`、`reload_config()`、`apply_config()`。`ServiceEvent` 与 wire `Event`
    **双枚举同步**新增 `ConfigChanged{revision: u64}`（M1-2 教训），`From<Event> for ServiceEvent`
    投影同步；`session_id()` 返回 `None`（全局事件，按会话订阅者也能收到）。`ServiceError` 新增
    `Config{message}`（含 `Display`："config error: .."）。全部只加不改，`Command` 协议未动。
  - 关键决策：① **四方法均无默认实现**（M1-1 惯例：crate 内方法全部为必需方法，未支持由实现者
    返回 `ServiceError::Unsupported`），同步更新全部 8 个实现者——mag-core `Engine`（四方法
    `Unsupported{operation}` stub + 注释注明 M3-5/M3-6 接线点；**未**顺手接 get/update/reload：
    Engine 尚无 `ConfigService` 字段，构造注入与 broadcast→`ConfigChanged` 桥接属 M3-5/M3-6，
    本任务接不了三方法中的任何一个而不越界）、mag-service `DummyService`、mag-acp 六个测试
    fake（`FakeService`/`ScriptedService`/`RoundService`/`BridgeService`/`CancelService`/
    `TwoRunService`）。② **`get_config` 返回 `Result`**——TODO 任务书原文写 `-> ConfigDto`，但
    trait 全部 async 方法统一 `Result<_, ServiceError>`（复用锚点明确），且无配置后端的实现者需要
    `Unsupported` 出口，取 `Result<ConfigDto, ServiceError>`。③ **事件载荷只有 `revision`**——
    docs/CLI.md §4.3 草稿写 `ConfigChanged{revision, summary}`，以 TODO 任务书
    `ConfigChanged{revision: u64}` 为准（同 M1-R 对 `Pivot*` 不携带 text 的取舍：事件保持小）。
    ④ `Config{message}` 只带 message 字符串（serde 友好；`ConfigError` 的行/列/路径信息展平进
    message，契约不绑定 mag-config 错误类型——secret 纪律由 message 不含物化 secret 值保证，
    rustdoc 注明）。
  - rustdoc：四方法注明决策 D2 生效时机——update/reload 立即换快照、revision+1、发
    `ConfigChanged`，但**不影响已钉住会话**；reload 遇损坏文件保留当前快照不崩；`apply_config`
    经 turn-complete 机制（§4.5，M3-5）在**各会话下一个 turn 边界**应用、Idle 会话立即应用、
    在飞 run 绝不 mid-turn 变更。`ConfigChanged`（双枚举）注明全局事件语义与新会话立即生效/
    旧会话钉住的 D2 语义。
  - 测试（全部离线；mag-service 19 passed）：`config_changed` tag 纳入双枚举 roundtrip+稳定 tag
    用例（revision 7）；`event_projects_into_matching_service_event` 新增 `Event::ConfigChanged`
    投影用例；`session_id()` 新增 `ConfigChanged -> None` 断言；
    `config_error_round_trips_and_displays`（tag `config` + Display）；
    `config_methods_are_callable_behind_arc_dyn`（object-safe 下四方法默认 stub 返回对应
    `Unsupported`）。mag-core `unimplemented_methods_return_unsupported` 扩为覆盖四方法。
  - 依赖边界：`cargo tree -p mag-service -e normal --depth 1` 为 async-trait/futures/mag-config/
    serde/serde_json/uuid——无 agent-lib/mag-core，硬约束满足。
  - 门禁结果：1) `cargo fmt --all -- --check` ✅（初次 3 处 fmt diff，`cargo fmt --all` 修复后复检
    通过）2) `cargo test -p mag-service` ✅（19 passed）3) `cargo clippy --all-targets --
    -D warnings` ✅ 4) `cargo test --workspace` ✅（全部测试目标 ok，1 ignored 为既有
    `#[ignore]` 联调测试）5) `cargo doc --no-deps --workspace` ✅（0 warning）。

### M3-5 [DONE] mag-core：turn-complete 通知/回调机制 + `apply_config`

- **上下文**：`docs/CLI.md` §4.5（决策 D2 附带）：通用「turn complete」通知/回调机制——不光能
  apply config，还能做其他功能（如弹桌面通知）。
- **实现要求**：
  - mag-core 定义内部 trait（如 `TurnCompleteListener: Send + Sync`），`on_turn_complete(session_id,
    TurnSummary)`；Engine 持有 `Vec<Arc<dyn TurnCompleteListener>>`，driver 在每次 run 终态
    （finished/error/cancelled）后逐一调用（listener  panic/错误隔离，不影响主流程）。
  - 第一消费者：`ConfigApplier`——`apply_config()` 置 pending 标记；turn complete 时若 pending，对
    该会话执行 reconfigure（agent-lib `Agent::reconfigure(ReconfigRequest)`：llm 参数/工具集/审批策略/
    budget 等可变项；skill 变体等不可变项按 agent-lib 语义报 Config 错→记 warn 不换）；审批策略类变更
    下一 run 自然生效。无 in-progress run 的会话立即应用（Idle 准入）。
  - listener 机制留出注册口（Engine builder / `add_turn_complete_listener`），供后续桌面通知等使用。
- **验证条件**：聚焦测试：(a) `apply_config` 后进行中会话在 turn 结束边界被 reconfigure（fake agent
  断言 reconfigure 调用时机在 run 终态之后）；(b) Idle 会话立即应用；(c) listener 异常不影响后续
  listener 与主流程；(d) 不可变项变更记 warn 且不中断。默认验证序列全过。

  **完成记录**（2026-07-23）：
  - 实现要点：新增 `crates/mag-core/src/turn_complete.rs`（lib.rs `mod turn_complete;` + 公开
    re-export `TurnCompleteListener`/`TurnCompletion`/`TurnSummary`；模块 rustdoc 引用
    `docs/CLI.md` §4.5）。`TurnCompleteListener: Send + Sync` 为**同步**回调
    `on_turn_complete(&self, &TurnSummary)`（`TurnSummary{session_id, completion:
    TurnCompletion::{Committed,Failed,Cancelled}}`）；crate 内部 `TurnCompleteHub`
    （`Arc<RwLock<Vec<Arc<dyn TurnCompleteListener>>>>`，Clone 共享注册表）持有 listener 列表，
    `notify()` 逐一对每个 listener `catch_unwind` 调用——panic 记 `tracing::warn!` 后继续后续
    listener，绝不波及 driver（投递前快照 listener 列表，毒锁按仓库统一策略恢复）。同步而非
    async 的取舍：触发点恰在 run 的可变 stream 借用释放、facade agent 归位之后，观察提交态
    无需 `.await`；异步消费者（桌面通知等）在回调内自行 spawn，rustdoc 注明。
  - 注入点（driver 层）：`SessionDriver::run_turn` 两条终态路径（stream 建立失败的早退 +
    主循环终态）在终态事件发出后各发一次 `TurnSummary`——§4.5 的 committed 一致点语义
    （成功 = 快照落库后；cancel/失败 = 归位后）。Engine 持有 hub（`EngineInner.turn_complete`），
    经 `SessionManager`→`session_thread`→`SessionDriver` 注入；公开注册口
    `Engine::add_turn_complete_listener`（hub 是 Arc 共享，会话存活后注册依然生效，供 bin
    装配处挂桌面通知等后续消费者）。
  - 第一消费者（配置 apply）：`Engine::apply_config` 从 Unsupported stub 变为真实实现。Engine
    新增构造器 `with_config_service(client, tools, Arc<ConfigService>)`（配置注入点；M3-6 的
    `Engine::from_config` 将走此路径），`EngineInner` 持有 `ConfigApplyState{service,
    generation: Arc<AtomicU64>}`（**pending 标记**=共享世代计数）并克隆给 `SessionManager`→
    各 session actor。`apply_config()`：`bump()` 世代+1 → 向每个存活 actor 发
    `SessionCommand::ApplyConfig{generation}` → `Ok(())`；无 config 后端的引擎（其余构造器）
    四方法维持 `Unsupported`。actor 侧：`ApplyConfig` 命令到达时 Idle 则立即应用
    （**Idle 立即应用**）；Running 则不动，run 终态回收 driver 处（`run_done_rx` 分支，先于
    deferred 命令重放）按 pending 世代补应用（**turn 边界应用**，§4.4「driver actor 在 run
    之间检查」）。actor 记录 `applied_generation`，重复命令/重复世代幂等。
    **竞态修复**：actor 线程启动晚于 bump 时「spawn 时读 pending 作基线」会丢应用——改为
    基线在 `spawn_session` 同步读取（注册 handle 之前）+ `ApplyConfig` 命令携带世代号，
    两种交错全覆盖。
  - reconfigure 字段映射（`SessionDriver::apply_config`，从快照 `agents.default` 条目映射；
    会话↔agent 名绑定在 M3-6 `from_config` 才显式化，`DEFAULT_AGENT_NAME` 常量注明）：
    `model` 变更→`SetModel`（`max_tokens`/`temperature` 沿用现值——配置 schema 尚无采样参数）；
    `tools`（过滤 `enabled=false`）→`ReplaceToolSet`（声明从 driver 持有的 `Arc<ToolRegistry>`
    投影，与现名集比对去抖，新 `ToolSetId` 由计数器铸造）；配置中的工具名不在注册表→warn 跳过
    （避免整组被 facade 准入拒绝）；agent 条目无 tools 列表=不约束，不动现有面。**逐项隔离**：
    每个 `ReconfigRequest` 独立 `agent.reconfigure`，失败（skill 变体等不可变项的
    `FacadeError::Config`、准入失败）记 `tracing::warn!` 换下一项，agent 与主流程不受影响。
    **出范围（记录备 M3-R/M3-6）**：审批策略与 budget 烤在 agent build 时、agent-lib reconfigure
    无对应变体——`tools.*.approval`/`approval.*` 变更在下次会话（重）建生效；`session` 缺省按
    D2 本就只影响新会话。
  - `get_config/update_config/reload_config` 一并接（M3-4 完成记录预留的 M3-5 接线点）：
    Engine 已持 `ConfigService`，`get_config` 投影当前快照回 DTO（secret 保持引用形态）；
    `update_config`/`reload_config` 代理 service，成功后经 EventBus 发
    `ServiceEvent::ConfigChanged{revision}`；`ConfigError` 展平为 `ServiceError::Config{message}`
    （message 不含物化 secret）。注：`ConfigChanged` 目前由引擎内发起的 update/reload 直接发射；
    未来 watch 自动 reload（M3-3 降级项）需另接 broadcast→事件桥。
  - 测试（全部离线；mag-core 81 passed，新增 11 个）：`turn_complete` 单测（panicking listener
    不饿死后续 listener）；driver 层 4 个——逐项隔离（(d)：`ActivateSkill` 报 Config 错跳过、
    后续 `SetModel` 仍生效且 turn 正常完成）、model+工具子集投影、disabled/未知工具过滤、无
    `agents.default` 条目 no-op（agent-lib 语义：reconfigure 排队、下一 turn 起点生效，断言经
    fake client 记录的请求）；engine 层 6 个——(a) gated stream 在飞 run 中 `apply_config`：
    首请求旧 model、run 正常完成后第二个 run 新 model（边界语义），(b) Idle 会话 apply 后首个
    run 即新 model+收缩工具面，(c) panicking listener 在后置 recording listener 之前注册：
    run 事件流正常且 recording 收到 `Committed`，cancel 终态→`Cancelled` 通知，
    四方法代理（get/update/reload/无效 update 报 `Config` 且快照不动 + `ConfigChanged` 事件），
    无后端 `apply_config` 维持 `Unsupported`。聚焦测试连跑 3 次无 flake。
  - 依赖边界：mag-core 新增 `tracing = "0.1"`（warn 日志；lockfile 已有 0.1.44，离线可用），
    未新增其他依赖；依赖方向不变。
  - 门禁结果：1) `cargo fmt --all -- --check` ✅（初次 6 处 fmt diff，`cargo fmt --all` 修复后
    复检通过）2) 聚焦测试 ✅（`cargo test -p mag-core apply_config` 6 个 + turn_complete/driver
    单测，连跑 3 次无 flake）3) `cargo clippy --all-targets -- -D warnings` ✅ 4)
    `cargo test --workspace` ✅（24 个测试目标全 ok，1 ignored 为既有 `#[ignore]` 联调测试）
    5) `cargo doc --no-deps --workspace` ✅（0 warning）。

### M3-6 [DONE] `Engine::from_config` + bin 读配置

- **上下文**：`docs/CLI.md` §4.3/§4.6；Engine 现有构造器在 `crates/mag-core/src/engine.rs`；
  bin 在 `crates/mag/src/main.rs`（现有 `--acp` 装配路径）。
- **实现要求**：
  - `Engine::from_config(config_service: Arc<ConfigService>, ..) -> Result<Engine, EngineError>` 装配
    构造器：按当前快照装配 LLM client（含 secret 引用解析——env/keyring 读取在此层做，失败报带引用名
    的错误、不输出值）、tool registry、sources registry（含 external agent slot 注册，供 M4 使用）、
    persistence、审批策略。
  - bin：新增 `--config <path>`（默认 `~/.config/mag/config.toml`）；启动读配置 → 建 ConfigService →
    `Engine::from_config`；`mag --acp` 同走配置装配（mag-acp 行为不变）。
  - 配置文件不存在：用内置默认配置（空 sources、无 external agent、默认审批策略）启动并记 info，
    不报错退出（验证原型友好）。
- **验证条件**：聚焦测试：`Engine::from_config` 用样例配置装配成功；缺 secret env var 报带引用名的错；
  bin 级 smoke（`--config` 指向 tempdir 样例，`--help`/启动路径不崩，可用 `#[ignore]` 或假 LLM 注入点）。
  默认验证序列全过。

  **完成记录**（2026-07-23）：
  - 实现要点：新增 `crates/mag-core/src/assembly.rs`——`EngineError`（thiserror；变体
    `Secret{provider,reference,reason}`/`Provider(String)`/`Persistence(#[from])`/`Io(#[from])`，
    `#[non_exhaustive]`，错误只带引用名/provider 名，绝不带值）；`Engine::from_config(
    Arc<ConfigService>) -> Result<Self, EngineError>` 装配构造器（LLM client 自 `agents.default`
    条目的 provider 装配；persistence：`persist_path` 是目录，db 文件
    `SESSION_DB_FILENAME = "mag-sessions.db"` 放其下）；`assemble_tool_registry`（builtins 减
    `enabled=false`，未知工具名 warn 跳过）；`assemble_source_registry`（providers→`LlmSource`、
    external_agents→`LocalAgentSlot::Acp` 占位供 M4）；`SessionBinding`（会话↔agent 绑定）+
    `ApprovalOverrides::from_snapshot`；`DEFAULT_AGENT_NAME` 自 driver.rs 移入（pub(crate)）。
    `engine.rs`：`EngineInner` 新增 `sources: SourceRegistry` 字段 + 公开访问器
    `Engine::sources()`；`assemble`/`in_memory_store` 改 `pub(crate)` 供 assembly 复用；
    `with_config_service` rustdoc 改现在时。bin 重写 `crates/mag/src/main.rs`：手写参数解析
    （`--acp`、`--config <path>`/`--config=<path>`、`--help`/`-h`；未知参数→usage+exit 2），
    默认路径 `$XDG_CONFIG_HOME/mag/config.toml` 否则 `~/.config/mag/config.toml`；文件缺失记
    info 用内置默认配置不报错；`--acp` 走 `ConfigService::load_or_default` → `Engine::from_config`
    → `mag_acp::serve`。
  - 关键设计/取舍：
    - **secret 解析**：惰性——只解析默认 agent provider 的 api_key；env 变体在此层
      `std::env::var` 读取；keyring 报 `EngineError::Secret` 明确 Unsupported（mag-sources
      `os-keyring` feature 未开，离线约束下按任务口径只实现 env 变体）；错误带 `{env = "VAR"}`
      引用名 + provider 名，不输出值。
    - **会话↔agent 绑定**：`SessionConfig.provider` 命名 `agents.<name>` 条目；空/"default"/
      未知名→回落 `default`（未知名 warn，保住 ACP 占位 provider "openai" 与遗留 "fake" 标签）；
      spawn 时（`session.rs::spawn_session`）从当前快照解析 `SessionBinding` +
      `ApprovalOverrides`——**配置更新后新建的会话自动拾取新审批策略/绑定**（agent-lib policy
      烤在 build 时，这是可行的最强语义）。创建时绑定条目的 model/tools/system_prompt 覆盖
      wire 值（§4.4「新会话立即用新 DO 图」），wire 值兜底；budget 优先级：显式 wire >
      `agents.<name>.budget` > `session.budget`；`apply_config` 从绑定条目 reconfigure（推广
      M3-5 写死的 default，`reconfig_requests` 改用 `self.agent_name`）。无配置后端的引擎行为
      完全不变。
    - **审批策略**：`driver::tool_surface(tools, binding, overrides)` 重写——base_policy 映射
      Allow→default / Deny→auto_deny / Ask→`Approval::ask(|_| Deny)` 兜底，binding.tools 过滤
      面，per-tool 覆盖最后应用；`approval.timeout_secs` 无 agent-lib 表面对应，未接线（留
      M3-R）；deny tier 在注入 IpcApproval 下仍暂停到界面（agent-lib 语义，rustdoc 已注明）。
    - **provider params / 多 provider 并行**：未消费（agent-lib 无对应表面/单共享 client
      架构），记录给 M3-R/M4。
  - 测试（全部离线）：assembly 7 个单测；driver/session 层新增 `mod session_binding` 5 个 e2e
    （创建时绑定 model+tools、命名条目绑定+apply 用绑定名、approval 默认 ask 暂停免权限工具、
    per-tool allow 覆盖 ask 默认、disabled 工具不出现在会话面）；重写
    `config_apply::apply_config_during_run_lands_at_the_turn_boundary`（创建时绑定使首个请求已是
    model-b，改为 mid-run `update_config` 到 model-c 验证边界语义）；bin 级新增
    `crates/mag/tests/cli.rs` 6 个 smoke（`env!("CARGO_BIN_EXE_mag")`：help、未知参数、无 --acp
    不加载配置、样例配置+注入假 env secret 的 ACP initialize 握手含 agentCapabilities、配置文件
    缺失握手、secret 缺失 exit≠0 且 stderr 带引用名/provider 名不带值）。
  - 依赖边界：mag-core 新增 `thiserror.workspace = true`（lockfile 已有，离线可用）；未新增其他
    依赖；依赖方向不变。
  - 门禁结果：1) `cargo fmt --all -- --check` ✅（初次有 diff，`cargo fmt --all` 修复后复检通过）
    2) 聚焦测试 ✅（`cargo test -p mag-core` 93 passed、`cargo test -p mag` 6 passed）
    3) `cargo clippy --all-targets -- -D warnings` ✅（曾报 3 个 doc_lazy_continuation 缩进错，
    已修）4) `cargo test --workspace` ✅（全部 ok，0 失败，1 ignored 为既有 zed 联调测试）
    5) `cargo doc --no-deps --workspace` ✅（0 warning）。

### M3-R [DONE] M3 review

- **实现要求**：对照 `docs/CLI.md` §4 全节检查：DTO↔DO 双向无损、快照隔离（update 不影响已钉住会话）、
  write-through 原子性、watch 回环防护、turn-complete 边界语义、secret 不物化不输出、GUI/web 可用性
  （四方法 + 事件足以驱动配置 UI）。发现问题直接修复并补测试。
- **验证条件**：默认验证序列全过；完成记录列出 review 结论。

  **完成记录**（2026-07-23）：
  - **review 方法**：通读 M3 全部六个提交的 diff 与当前源码（mag-config 五模块、mag-core
    `config.rs`/`turn_complete.rs`/`assembly.rs` 及 engine/driver/session 接线、mag-service 契约 diff、
    bin `main.rs`），逐条对照 `docs/CLI.md` §4.1–§4.6 与决策 D2/D4。
  - **检查点结论（逐项）**：
    1. **DTO↔DO 双向无损** ✅——`resolve`/`project` 对称，raw `Option` 字段保留（未设置的键投影后仍
       未设置，无幽灵节）；implicit tool 节点不回投；roundtrip 测试（`dto_do_dto_round_trip_is_lossless`
       等）覆盖；secret 全程保持 `SecretRef` 引用形态，DO→DTO→TOML 序列化仍为 `{env=...}`/
       `{keyring=...}` 引用字样。
    2. **快照隔离** ✅——会话钉住 `Arc<ConfigSnapshot>`（create/resume 时 `current()` 廉价克隆），
       节点构建后不可变，update 换根不影响旧图；`snapshot_isolation_keeps_pinned_snapshot_unchanged`、
       `update_swaps_the_snapshot_and_persists_the_file`（pinned 句柄原值断言）覆盖。
    3. **write-through 原子性** ✅——`save_atomic` 同目录临时文件 + write_all + sync_all + rename，
       失败清理临时文件；`ConfigService::update` 先落盘后换根，写盘失败不换快照、不发信号
       （`update_keeps_the_snapshot_and_emits_nothing_when_the_write_fails`）；写者全程持写锁串行化，
       revision/文件/内存快照三者一致。
    4. **watch 回环防护** ✅（已降级，记录完整）——notify 离线不可拉取，降级为仅显式 reload；降级原因
       与 watcher 落地时的回环防护要求（去抖 ~300ms + 自写指纹去重）写在 `config.rs` 模块 rustdoc 与
       M3-3 完成记录；仅显式 reload 时回环不可能发生，降级可接受。
    5. **turn-complete 边界语义** ✅——`run_turn` 两条终态路径（stream 建立失败早退 + 主循环终态）在
       终态事件发出、stream 借用释放后各发一次 `TurnSummary`（committed=快照落库后，cancel/失败=归位
       后）；listener 经 `catch_unwind` 隔离（panicking listener 不饿死后续 listener 的测试）；apply 在
       turn 边界（run_done 回收 driver 处先于 deferred 命令重放补应用）、Idle 立即（`ApplyConfig` 命令
       即服务即应用）、在飞 run 绝不 mid-turn 变更（engine e2e
       `apply_config_during_run_lands_at_the_turn_boundary`：首请求旧 model、次请求新 model）。
    6. **secret 不物化不输出** ✅——取值只在装配 `ProviderConfig` 最后一刻（`resolve_provider_secret`
       惰性解析默认 agent 的 provider）；`EngineError::Secret` 只带 provider 名 + 引用名字样；
       `get_config` 投影引用形态；测试断言错误消息不含 secret 物质（`!message.contains("sk-")`）。
    7. **GUI/web 可用性** ✅（一处确认偏差）——四方法 + `ConfigChanged{revision}` 足以驱动配置 UI
       （读 `get_config`、写 `update_config`、刷新靠事件）。确认偏差：§4.3 草稿的
       `ConfigView`（含 revision）与 `update_config/reload_config -> Result<u64>` 按 M3-4 任务书签名
       落地为 `ConfigDto` + `Result<(), ServiceError>`，revision 经 `ConfigChanged` 事件获得（M3-4
       决策②③已记录）；`ConfigPatch` 节级替换简化为整文档替换（GUI 读-改-写全文档，能力上是超集）。
       如需「启动即知当前 revision」，可在后续里程碑以向后兼容新增方法补上（本 review 不动冻结契约）。
    8. **冻结契约只加不改** ✅——M3-4 diff 纯新增（四方法、`ConfigChanged` 双枚举变体、
       `ServiceError::Config`），既有方法签名与事件语义未动；M3-5/M3-6 只动 mag-core 内部与 bin。
  - **发现与修复**（1 项，已修并补测试）：**显式 `tools = []` 与缺失 `tools` 键被混为一谈**。
    `SessionBinding::resolve` 用 `.filter(|names| !names.is_empty())` 把显式空列表折叠成「不约束」
    （会话反而暴露全部注册工具），`driver::reconfig_requests` 用 `if !wanted.is_empty()` 跳过，导致
    apply 无法把工具面收缩到零——两处都把 `None`（无 tools 键=不约束）与 `Some([])`（显式空=零工具）
    混同。修复：`ResolvedAgent` 新增 `tools_list() -> Option<&[Arc<ResolvedTool>]>` 访问器保留区分
    （`tools()` 维持折叠语义并在 rustdoc 指明）；binding 与 reconfig 改用 `tools_list()`——显式空列表
    在建会话时投射为空工具面（`tool_surface` 本就正确处理 `Some(&[])`）、在 apply 时投射为空
    declarations 的 `ReplaceToolSet`（agent-lib 准入对空集 vacuous 通过，已核
    `facade::Agent::reconfigure` 与 `ReconfigRequest::validate` 只查重名与注册表背书）。新增测试 4
    个：mag-config `explicit_empty_tool_list_is_distinct_from_no_tool_list`（含 roundtrip 保留
    `Some(vec![])`）；mag-core `session_binding_distinguishes_an_explicit_empty_tool_list`、
    `apply_config_clears_the_surface_on_an_explicit_empty_tool_list`、engine e2e
    `explicit_empty_tool_list_builds_a_tool_less_session`。
  - **已知限制确认清单**（逐条核对记录位置，确认完整、可接受）：
    1. 文件 watch 降级为仅显式 reload——M3-3 完成记录 + `config.rs` 模块 rustdoc（含回环防护落地
       要求）✅。
    2. keyring secret 变体报明确 Unsupported（只实现 env）——M3-6 完成记录 + `assembly.rs` 模块
       rustdoc/`EngineError::Secret` rustdoc/`resolve_provider_secret` 实现（错误带引用名不带值，
       有测试）✅。
    3. 审批策略/budget 烤在 agent build 时，reconfigure 只覆盖 SetModel/ReplaceToolSet；新会话
       spawn 时拾取新绑定——M3-5「出范围」段 + M3-6 审批策略段 + `driver.rs::apply_config` rustdoc
       「Out of scope」段 + `ApprovalOverrides::from_snapshot` rustdoc ✅。
    4. `approval.timeout_secs` 未接线（agent-lib 无对应表面）——`ApprovalOverrides::from_snapshot`
       rustdoc（注明留 M3-R）+ M3-6 完成记录 ✅，本 review 确认为 agent-lib 表面缺口，非 mag 侧缺陷。
    5. deny tier 在注入 IpcApproval 下仍暂停到界面（agent-lib 语义）——`tool_surface` rustdoc +
       M3-6 完成记录 ✅。
    6. provider params / 多 provider 并行未消费（agent-lib 无对应表面/单共享 client 架构）——M3-6
       完成记录（记给 M3-R/M4）✅；`ResolvedProvider::params` 在 DTO/DO/roundtrip 层完整保留，
       仅装配层不消费。
    7. （本 review 补充确认）`get_config` 不返回 revision、`update_config` 为整文档替换——M3-4
       决策记录 ✅，见检查点 7。
  - **依赖边界抽核**：`cargo tree -p mag-config -e normal --depth 1` 仅 serde/thiserror/toml；
    mag-service 无 agent-lib/mag-core——硬约束满足。
  - **门禁结果**：1) `cargo fmt --all -- --check` ✅（本次修复初检 1 处 diff，`cargo fmt --all`
    修复后复检通过）2) `cargo test -p mag-config` ✅（40 passed，含新增 1）与
    `cargo test -p mag-core` ✅（96 passed，含新增 3）3) `cargo clippy --all-targets -- -D warnings`
    ✅ 4) `cargo test --workspace` ✅（全部测试目标 ok，0 失败，1 ignored 为既有 zed 联调测试）
    5) `cargo doc --no-deps --workspace` ✅（0 warning）。
  - **review 结论**：M3 六个任务实现与 `docs/CLI.md` §4 全节一致，决策 D2（钉住 + 显式 apply）与
    D4（DTO/DO 分层）落地正确；发现 1 处语义缺陷（显式空工具列表）已修复并补 4 个测试；全部已知
    限制均有完整记录且属外部环境约束（agent-lib 表面/离线依赖），可接受。**M3 通过**。

---

## Milestone M4 — delegation 接线（`docs/CLI.md` §5 P7，决策 D3）

目标：external ACP agent 是本程序核心功能（决策 D3：尽早动手）。model-routed `ask_<name>` 委派两条来源
全部落地：local LLM subagent（agent-lib `Agent::worker()`）与 external ACP agent
（`ManagedExternalAgent::acp`）。

### M4-1 [DONE] mag-core：local LLM subagent 委派 + `Delegation*` 事件映射

- **上下文**：`docs/CLI.md` §5 P7；agent-lib `Agent::worker()`、`ask_<name>` model-routed 委派机制；
  `ServiceEvent::DelegationStarted/Finished/Failed/DelegationMessage` wire 变体已在契约中（本任务把它们
  真正接通）。
- **实现要求**：
  - mag-core 新增 delegate 装配：按配置 `[[agent]]` 条目为会话主 agent 注册 worker delegate
    （agent-lib worker 语义），tool surface 出现 `ask_<name>`。
  - driver 把 agent-lib 委派生命周期事件映射到 `Delegation*` wire 事件（trace 含 delegate 名、输入摘要、
    输出/失败原因）；`DelegationMessage` 映射中间消息。
  - 子 agent 交互经 M2 origin 归因 pop 到 root。
- **验证条件**：聚焦测试：fake LLM 下主 agent 调 `ask_researcher` → `DelegationStarted/Finished` 顺序正确、
  trace 内容完整；delegate 内触发审批 → root 收到带 origin 的 `InteractionRequested`。默认验证序列全过。

  **完成记录**（2026-07-20）：

  - **delegate 装配点**：`SessionBinding::resolve`（assembly.rs）新增 `delegates: Vec<DelegateBinding>`——
    快照中除会话绑定条目外的每个 `agents.<name>` 条目解析为一个 `DelegateBinding{name, description,
    model, system_prompt, tools}`；`description` 取条目 `role`，缺省回落 ``Local subagent `<name>` ``；
    `tools` 复用绑定条目的同一过滤语义（`tools_list()` 保留「无键=不约束 / 显式空=零工具」区分，
    disabled 条目滤除）。`SessionDriver::new`/`restore`（driver.rs）对每个 delegate 经 `delegate_worker`
    构建 agent-lib `Agent::worker()` 的 `LocalSubagent` 并 `.subagent(name, worker)` 注册（facade 默认
    `Delegation::model_routed` → tool surface 出现 `ask_<name>`，e2e 断言其在 supervisor 的
    `ChatRequest.tools` 中）。worker 的 LLM 参数/系统提示/工具声明来自 `ResolvedAgent`：`model` 有值则
    pin、无值则 inherit（agent-lib R4）；工具面为 **declaration-only**（agent-lib worker 数据优先语义，
    见下「已知限制 1」）；审批策略按 `[approval]` 默认 tier + 插件 `permission()` → `ask_tool` +
    `[tools.<name>].approval` 覆盖逐 delegate 派生（与主 agent `tool_surface` 同源，提取共享辅助
    `apply_per_tool_tiers`）。
  - **LlmClient 共享决策**：按 agent-lib 语义**共享**——`LocalSubagent` 数据优先不含 client，委派兑现时
    `FacadeSubagentSpawner` 克隆 supervisor 的 `Arc<dyn LlmClient>` 驱动 child；agent-lib 无 per-delegate
    client 表面，故 delegate 条目自己的 `provider` 在装配层不可消费（与 M3-6 记录的 provider params
    未消费同源，记给 M4-R）。共享 client 也让测试脚本顺序确定（supervisor `chat_stream` / child `chat`
    走同一 `FakeLlmClient` 脚本队列）。
  - **事件映射点**：driver `map_wire_event` 新增四个 arm——facade `DelegationStarted/Finished/Failed`
    → 同名 wire 事件（`delegation_trace_from_wire`），`DelegationMessage` → wire `DelegationMessage`
    （`delegation_message_from_wire`）；`DelegationProgress` 无 wire 对应变体、忽略；
    `ApprovalRequested` 仍按既有决策丢弃（canonical pause 是 `IpcApproval` 的 `InteractionRequested`）。
    契约零改动（`Delegation*` 变体早已在冻结契约中）。
  - **restore 路径**：`SessionDriver::restore` 经 `AgentRestoreBuilder::subagent` 用同一 `delegate_worker`
    重注册全部 delegate——快照只持久化 data-only recipe 且审批策略恒回落 default，重注册使恢复后审批
    策略与 tool surface 与新会话一致（M4-3 的「静默回落 auto_allow」陷阱在本任务已结构性消除；M4-3
    仍负责其回归测试与 `ask_<name>` 调用本身的审批接入）。
  - **验证**：新增测试 7 个（mag-core 96→103）：assembly
    `session_binding_resolves_delegates_from_the_other_agent_entries`（排除绑定条目/role→description/
    model pin/工具过滤/无配置后端无 delegate）；driver 映射单测 ×4（Started/Finished/Failed/Message
    映射 + facade wire roundtrip）；engine e2e ×2——
    `ask_researcher_emits_the_delegation_lifecycle_in_order`（Started<Finished<RunFinished、trace.delegate
    正确、无 `ToolStarted/Finished` 包裹 `ask_researcher`、supervisor tool surface 含 `ask_researcher`、
    child 用 pin 模型 + 配置 system prompt 经共享 client 驱动）与
    `delegate_tool_approval_pops_to_root_with_origin`（delegate 内 `shell` 门控暂停 → root 收到
    `origin{delegate:"researcher", depth:1}` 的 Approval 交互 → approve 后 DelegationFinished +
    RunFinished，端到端验证 M2 origin 归因）。
  - **已知限制**（逐条记录，供 M4-R 核对）：
    1. **delegate 工具 declaration-only**：agent-lib worker 语义下 child 工具只有声明；获批的 child
       工具调用以 facade 的 declaration-only `UnknownTool` 结果回填给 child 模型（e2e 已按此断言）。
       child 真正执行 mag 工具需 agent-lib 提供可执行 child registry 表面。
    2. **wire trace 的 `task`/`output`/`message` 恒为 `None`**：agent-lib facade `DelegationTrace` 仅
       携带 `{delegate, status, usage}`，委派输入摘要与输出/失败原因不在 facade 事件表面上；
       `delegation_trace_from_wire` rustdoc 已注明。契约字段为 optional，语义不受损。
    3. **streaming 路径发射时序**：agent-lib 在 delegation tool 的 fulfill 内同步驱动 child，
       `DelegationStarted/Finished` 在 child settle 后成对发射——故 delegate 内暂停的审批交互
       **先于** `DelegationStarted` 到达 root（e2e 注释与断言已按实际时序写）；`Started<Finished`
       顺序本身不受影响。若 UI 需要「先见 Started 再见子交互」，需 agent-lib 调整 tap 发射点（记给
       M4-R 评估是否提 agent-lib 需求）。
    4. `DelegationMessage`/`DelegationProgress` 目前 agent-lib 生产路径从不发射（placeholder 类型）；
       映射已接通并有单测，未来 agent-lib 开始发射即自动生效。
  - **门禁结果**：1) `cargo fmt --all -- --check` ✅（初检 2 处 diff，`cargo fmt --all` 修复后复检通过）
    2) 聚焦测试 `cargo test -p mag-core delegation`（6 过）与 `cargo test -p mag-core session_binding`
    （9 过）✅ 3) `cargo clippy --all-targets -- -D warnings` ✅ 4) `cargo test --workspace` ✅
    （全部目标 ok，0 失败；mag-core 103 过含新增 7；1 ignored 为既有 zed 联调测试）
    5) `cargo doc --no-deps --workspace` ✅（0 warning）。

### M4-2 [DONE] mag-core：external ACP agent 委派（决策 D3，核心）

- **上下文**：`docs/CLI.md` §5 P7 + 决策 D3；agent-lib `ManagedExternalAgent::acp(binary,args)` +
  `default_external_session_handler`（behind `external-acp` feature）；mag-sources 已有 ACP slot
  （`crates/mag-sources/src/registry.rs`）。
- **实现要求**：
  - mag-core 的 agent-lib 依赖开 `external-acp` feature。
  - 按配置 `[[external_agent]]`（kind=acp、command/args/env）经 mag-sources ACP slot 注册
    `ManagedExternalAgent::acp(..)` + `default_external_session_handler`；name 进入 tool surface
    （`ask_<name>`），`list_sources()` 如实反映（kind/available/capabilities）。
  - external agent 启动失败/探测失败：`SourceInfo.available=false` + 原因记日志，不阻塞 Engine 启动；
    委派调用时失败映射 `DelegationFailed`。
  - 会话生命周期对齐：会话结束/删除时 external session 清扫（agent-lib 未 committed 自动清扫已就位）。
- **验证条件**：聚焦测试用**本地 fake ACP 进程**（脚本实现 ACP initialize/session/prompt 最小协议，
  `docs/CLI.md` §6）：(a) `ask_<ext>` 委派全生命周期事件正确；(b) fake 进程崩溃映射 `DelegationFailed`；
  (c) `list_sources` 反映可用性；(d) 会话结束清扫（断言 fake 进程收到结束/退出）。真实 claude-code-acp
  等联调 `#[ignore]`。默认验证序列全过。

  **完成记录**（2026-07-20）：
  - 实现要点：mag-core 的 `agent-lib` 依赖开启 `external-acp` feature；`SessionBinding::resolve`
    新增 `external_delegates`，把 `external_agents.<name>`（kind=acp）解析为
    `ExternalDelegateBinding{name, command, env, capabilities}`。`SessionDriver::new` 与 `restore`
    均为每个 external binding 注册 `ManagedExternalAgent::acp(..)`，因此 supervisor tool surface
    出现 `ask_<name>`；restore 路径同样重注册 external delegate 与 session handler，为 M4-3 的
    审批/restore 回归打底。
  - ACP handler：每个 external delegate 注入 registry-backed `ExternalSessionHandler`，底层是
    `AcpAdapter + ExternalSessionRegistry`，与 agent-lib `default_external_session_handler` 的 ACP 分支
    同一组合；因默认 helper 没有 mag 配置里 `external_agents.<name>.env` 的注入面，本任务直接构造
    `AcpConfig` 以保留 env 覆盖。handler 外包一层 `TrackedExternalSessionHandler` 记录 agent-lib
    为 external child mint 的 `AgentId`，session actor 退出/删除前按这些 id 调
    `registry.cleanup_agent(..)`，确保已完成 external session 也显式收到 ACP `session/cancel` 并清扫。
    每个 handler 使用独立 `GitWorktreeManager` root，避免默认全局 temp root 在并行/重复测试中碰撞，
    仍保留 agent-lib 的 worktree 隔离与 cleanup 策略。
  - source/probe：`Engine::list_sources()` / `probe_local_agents()` 从当前 `ConfigSnapshot` 投影
    `SourceInfo`；LLM provider 标为 `LlmProvider` 且 available=true；external ACP source 标为
    `LocalAgent`，`path` 为 command[0]，capabilities 来自配置，并用绝对/相对路径或 `$PATH` 做轻量
    executable 检查。`probe_local_agents()` 返回 local/external sources 并发
    `LocalAgentsProbed{available}` 全局事件。无配置后端的 engine 返回空列表，不再报 Unsupported。
  - 测试（全部离线，`engine::delegation`）：新增本地 fake ACP shell 进程（响应 initialize / session/new /
    session/prompt，记录收到的 JSON-RPC 帧）；`ask_external_acp_delegate_emits_lifecycle_and_cleans_up_on_delete`
    断言 `ask_peer` 出现在 tool surface、`DelegationStarted -> DelegationFinished -> RunFinished`、fake
    进程收到 initialize/new/prompt，删除 session 后收到 `session/cancel`；
    `crashing_external_acp_delegate_maps_to_delegation_failed` 断言 fake 进程崩溃映射为
    `DelegationFailed` 且 supervisor 可继续完成；
    `list_sources_reports_external_acp_availability_and_capabilities` 断言可执行 fake source available=true、
    缺失 binary available=false、capabilities/path 正确，并验证 `LocalAgentsProbed` 事件。聚焦测试
    连跑 3 次无 flake，单测试 < 1s。
  - 门禁结果：1) `cargo fmt --all -- --check` ✅ 2) `cargo test -p mag-core delegation` ✅（9 passed）
    3) `cargo clippy --all-targets -- -D warnings` ✅ 4) `cargo test --workspace` ✅（全绿，1 ignored
    为既有 zed 联调测试）5) `cargo doc --no-deps --workspace` ✅（0 warning）。

### M4-3 [DONE] 委派审批 + restore 重注册 delegate

- **上下文**：`docs/CLI.md` §5 P7；`ApprovalPolicy::ask_tool("ask_<name>")` 走 IpcApproval
  （`crates/mag-core/src/approval.rs`）；**已知陷阱**：restore 必须重注册全部 delegate，否则审批策略静默
  回落 auto_allow。
- **实现要求**：
  - 委派调用默认走审批：`ask_<name>` 按审批策略（配置 `[approval]` 段）经 IpcApproval 向 root 会话发
    `InteractionRequested`（Approval kind，origin 归因 M2）；批准才执行。
  - `resume_session` 恢复路径重注册会话全部 delegate（local + external），恢复后审批策略与 tool surface
    与新会话一致；补回归测试防静默回落。
- **验证条件**：聚焦测试：(a) 委派触发审批，deny 时 `DelegationFailed`/工具拒绝路径正确；(b) resume 后
  `ask_<name>` 仍出现在 tool surface 且审批策略生效（断言不回落 auto_allow）；(c) approve 后委派正常
  执行。默认验证序列全过。

  **完成记录**（2026-07-20）：
  - 实现要点：`driver::tool_surface` 在 fresh 与 restore 共用路径中新增默认 delegate start 审批层：
    对 `SessionBinding` 解析出的全部 local delegate 与 external ACP delegate 统一生成 `ask_<name>`，并在
    应用 `[tools.<name>].approval` 前先 `ApprovalPolicy::ask_tool("ask_<name>")`。因此委派启动默认经
    root 会话注入的 `IpcApproval` 发 `InteractionRequested{kind:Approval}`，批准后才执行；显式
    `[tools.ask_<name>] approval = "allow" | "deny" | "ask"` 仍是最终覆盖。external ACP 的 start gate
    走 agent-lib drive-layer async parent handler，local delegate start 走普通 tool approval gate；两者共用
    root `IpcApproval`。
  - restore 路径：`SessionDriver::restore` 已在 M4-2 路径中按 `binding.delegates()` 与
    `binding.external_delegates()` 重注册 local + external delegate；本任务补默认 start 审批后，restore 与
    fresh build 自动共享同一 `tool_surface` 策略，防止 snapshot 中 data-only delegate 恢复为
    agent-lib 默认 `auto_allow`。新增回归测试证明 resume 后 `ask_researcher` 仍在 tool surface 且先弹审批，
    不会静默回落 auto_allow。
  - 测试（全部离线）：新增 4 个 delegation e2e：
    `delegate_start_denial_does_not_drive_the_local_delegate`（deny 后不驱动 child LLM，supervisor 收到拒绝
    工具结果后继续）、`delegate_start_approval_allows_the_local_delegate_to_run`（approve 后
    `DelegationStarted`→`DelegationFinished` 且 child 运行）、
    `resume_re_registers_local_delegate_and_start_approval_policy`（持久化重启后 tool surface 含
    `ask_researcher` 且审批仍生效）、`external_acp_delegate_start_approval_runs_only_after_approval`（fake ACP
    在批准前不收到 prompt，批准后正常 `DelegationStarted`→`DelegationFinished`）。旧 M4-1/M4-2 生命周期测试
    对 `ask_researcher`/`ask_peer` 显式配置 `allow`，保持原测试焦点。
  - 门禁结果：1) `cargo fmt --all` ✅ 2) `cargo test -p mag-core delegation` ✅（13 passed）
    3) `cargo fmt --all -- --check` ✅ 4) `cargo clippy --all-targets -- -D warnings` ✅
    5) `cargo test --workspace` ✅（全绿，1 ignored 为既有 zed 联调测试）
    6) `cargo doc --no-deps --workspace` ✅（0 warning）。

### M4-R [DONE] M4 review

- **实现要求**：对照 `docs/CLI.md` §5 P7 与决策 D3 检查：两条来源行为一致（事件、审批、origin）、
  external 生命周期清扫、restore 完备性、feature gating 正确（不开 feature 时编译过、external 配置报
  明确错误）。发现问题直接修复并补测试。
- **验证条件**：默认验证序列全过；完成记录列出 review 结论。

  **完成记录**（2026-07-20）：
  - review 范围：对照 `docs/CLI.md` §5 P7 与决策 D3，复核 M4-1/M4-2/M4-3 当前实现与测试覆盖，重点检查
    local LLM subagent 与 external ACP agent 两条来源的事件、审批、origin 归因、external 生命周期清扫、
    restore 重注册，以及 feature gating。
  - 检查点结论：
    1. **两条委派来源事件一致** ✅——local 与 external 均通过 model-routed `ask_<name>` 暴露；driver
       将 facade `DelegationStarted/Finished/Failed/Message` 映射到 service wire 事件；成功与失败路径均有
       聚焦测试覆盖。
    2. **审批与 origin** ✅——delegate start 默认经 `ApprovalPolicy::ask_tool("ask_<name>")` 进入 root
       `IpcApproval`；`[tools.ask_<name>].approval` 可覆盖；local child 工具审批与 external start 审批均携带
       M2 origin，root 订阅者可见 delegate 名与 depth。
    3. **external 生命周期清扫** ✅——external ACP handler 记录 agent-lib minted child `AgentId`，会话删除/actor
       退出时调用 registry cleanup；fake ACP e2e 断言删除后收到 `session/cancel` 或等价清扫标记。
    4. **restore 完备性** ✅——restore 路径与 fresh build 共用 delegate 注册与 `tool_surface` 审批策略，恢复后
       `ask_researcher` 仍在 tool surface 且不会回落 auto_allow；回归测试覆盖。
    5. **feature gating** ✅（发现 1 项问题并修复）——原实现把 `agent-lib/external-acp` 直接写在依赖上，且
       `driver.rs` 无条件导入 ACP runtime 类型，无法验证“不启 feature 仍编译”。已修复为 `mag-core` 自身默认
       开启 `external-acp` feature，并转发到 `agent-lib/external-acp`；external ACP runtime handler、delegate
       构造、cleanup 字段与 Unix fake ACP e2e 均按 feature 条件编译；关闭 feature 时，`Engine::from_config`
       遇到 `[external_agents.*] kind="acp"` 返回明确的 `EngineError::ExternalAgentUnsupported`，新增
       no-default feature 测试覆盖错误消息包含 agent 名、kind 与所需 feature。
  - 保持的已知限制（确认非本轮缺陷）：local delegate 工具仍是 agent-lib worker 的 declaration-only 语义；
    delegate 自身 provider、provider params、多 provider 并行仍受 agent-lib 当前共享 client 表面限制；
    `DelegationMessage/Progress` 生产路径仍取决于 agent-lib 是否发射。上述限制均已在 M4-1/M4-2 完成记录中
    明确记录，不阻塞 M4-R。
  - 门禁结果：1) `cargo fmt --all -- --check` ✅ 2) 聚焦测试
    `cargo test -p mag-core delegation` ✅（13 passed）与
    `cargo test -p mag-core --no-default-features from_config_rejects_external_acp_config_when_feature_is_disabled`
    ✅（1 passed）3) no-default 编译/ lint：
    `cargo check -p mag-core --no-default-features --all-targets` ✅，
    `cargo clippy -p mag-core --no-default-features --all-targets -- -D warnings` ✅ 4)
    `cargo clippy --all-targets -- -D warnings` ✅ 5) `cargo test --workspace` ✅（全绿，1 ignored 为既有
    zed 联调测试）6) `cargo doc --no-deps --workspace` ✅（0 warning）。
  - review 结论：M4 对 `docs/CLI.md` §5 P7 / 决策 D3 的核心要求已满足；本 review 发现并修复 feature
    gating 缺口，默认 external ACP 功能保持开启且行为不变，关闭 feature 时编译与诊断均明确。**M4 通过**。

---

## Milestone M5 — ask_user 工具（`docs/CLI.md` §5 P6，决策 D6）

目标：AskUserQuestion 式通用交互作为普通 plugin 先行（GUI 阶段再详细设计）。

### M5-1 [DONE] mag-tools：`ask_user` ToolPlugin

- **上下文**：`docs/CLI.md` §5 P6 + 决策 D6；`ToolPlugin` trait 在
  `crates/mag-tools/src/plugin.rs`（执行时细读签名）；交互桥复用 mag 侧审批/交互注入路径
  （`InteractionKindWire::Question/Choice`）。
- **实现要求**：
  - 新 `ask_user` 工具：输入 `{question: String, options: Option<Vec<String>>}`；有 options →
    `Choice{prompt,options}` 交互，响应 `Choice{index}`；无 → `Question{prompt}`，响应 `Answer{text}`。
  - 阻塞式 handler + 闭包捕获交互桥（与 approval 注入同路径）；`select!` `ToolContext::cancel`——cancel
    时立即返回取消（agent-lib 阻塞批抢占已就位），不悬挂。
  - 注册进 tool registry（tool profile 可控开关）；工具描述写清「向用户提问并等待回答」。
- **验证条件**：聚焦测试：fake LLM 触发 `ask_user` → 订阅者收到 `Question/Choice` 交互 →
  `respond_interaction` 回答成为工具输出进入后续上下文；cancel 中途打断工具立即返回 Cancelled。
  默认验证序列全过。

  **完成记录**（2026-07-20）：
  - 实现要点：`mag-tools` 新增普通 `ToolPlugin`：`AskUserTool`（工具名 `ask_user`，输入
    `{question:String, options:Option<Vec<String>>}`，描述为“向用户提问并等待回答”），并注册进
    `ToolRegistry::with_builtins()` / `builtin_tools()`（内置工具顺序变为 `read_file`、`list_dir`、`grep`、
    `shell`、`ask_user`）。`mag-tools` 同步新增 `ToolInvocation` 与 `UserInteractionBridge` 抽象：既有工具
    继续只实现旧 `invoke(ToolContext, Value)`；`ask_user` 覆盖 `invoke_with_context`，通过桥发起用户交互。
    `ToolRegistry::bind_with_user_interaction` 供直接 registry 路径注入桥，`bind` 无桥时 `ask_user` 返回
    model-visible error（避免静默挂起）。
  - mag-core 接线：`SessionDriver` 在把每个 `ToolPlugin` 投影成 facade `Tool` 时，为当前会话注入
    `IpcUserInteractionBridge`（私有桥，持有同一 `Arc<IpcApproval>`）。桥把 `ask_user` 请求映射为
    agent-lib `Interaction::question` / `Interaction::choice`，经既有 `IpcApproval` 发
    `ServiceEvent::InteractionRequested{kind:Question|Choice, origin:root}`，再由 `respond_interaction` 回灌。
    这复用同一 pending map / `RequestId` 铸造 / cancel 处理路径，未新增 service 契约。
  - cancel 语义：`AskUserTool` 使用 `tokio::select!` 竞争桥调用与 `ToolContext::cancel.cancelled()`；cancel
    触发时立即返回 `ToolResult::error("ask_user cancelled")`。桥侧构造的 `RunContext` 共享同一个 cancel token，
    因此底层 `IpcApproval` 也会丢弃 pending 交互，避免迟到响应唤醒已取消 run。
  - 输出形态：open `Question` 的 `Answer{text}` 作为纯文本工具输出；fixed `Choice{index}` 输出紧凑 JSON
    `{"index":n,"option":"..."}`，并校验 index 落在 options 范围内，越界返回 model-visible error。
  - 测试（全部离线）：`mag-tools` 新增 4 个集成测试覆盖 Question 桥调用、Choice 输出、无桥错误、预取消快速
    返回，并更新内置工具声明/`ToolSetRef`/permission 元数据断言；`mag-core` 新增 3 个 fake LLM 端到端测试：
    `ask_user_question_round_trips_through_interaction_and_enters_context`、
    `ask_user_choice_round_trips_through_interaction_and_enters_context`、
    `ask_user_cancel_unblocks_the_parked_tool`，断言订阅者收到 `Question/Choice`，`respond_interaction` 后工具输出
    出现在下一次 LLM 请求上下文，cancel 后 run 以 `RunErrorKind::Cancelled` 结束。单测试均 < 1s。
  - 门禁结果：1) `cargo fmt --all -- --check` ✅ 2) 聚焦测试 ✅：`cargo test -p mag-tools`（23 passed）与
    `cargo test -p mag-core ask_user`（3 passed）3) `cargo clippy --all-targets -- -D warnings` ✅
    4) `cargo test --workspace` ✅（全绿，1 ignored 为既有 zed 联调测试）5) `cargo doc --no-deps --workspace` ✅
    （0 warning）。

### M5-R [DONE] M5 review

- **实现要求**：对照 `docs/CLI.md` §5 P6 检查：交互桥复用一致、cancel 语义、tool profile 开关、rustdoc。
  发现问题直接修复并补测试。
- **验证条件**：默认验证序列全过；完成记录列出 review 结论。

  **完成记录**（2026-07-20）：
  - review 范围：对照 `docs/CLI.md` §5 P6 / 决策 D6，复核 `mag-tools` 的 `ask_user` 插件、
    `ToolInvocation` / `UserInteractionBridge` / registry 注入路径、mag-core `tool_surface` 与
    `IpcUserInteractionBridge` 接线、ask_user 端到端测试与 rustdoc。
  - 检查点结论：交互桥复用一致 ✅——fresh/restore 会话用同一个 `Arc<IpcApproval>` 注入 facade
    `interaction_handler`，并构造 `IpcUserInteractionBridge`；`Question` / `Choice` 通过同一 pending map、
    `RequestId` 铸造与 `respond_interaction` 唤醒路径回灌，root origin 行为由既有测试覆盖。
  - 检查点结论：tool profile / 工具面开关 ✅——`ask_user` 作为普通 `ToolPlugin` 注册在 builtins 中，
    `tool_surface` 按绑定 agent 的工具列表过滤全部插件；既有显式工具子集、disabled 工具、显式空工具面测试
    同样覆盖 `ask_user` 所属的普通插件路径，未发现特例绕过。
  - 检查点结论：rustdoc ✅——`AskUserTool`、`UserInteractionRequest` / `Response` / `Bridge`、
    `ToolRegistry::bind_with_user_interaction`、mag-core `IpcUserInteractionBridge` 与 `facade_tool` 文档均说明
    §5 P6 / D6 的普通 plugin + 主机桥设计；`cargo doc` 0 warning。
  - 发现与修复（1 项，已修并补测试）：`AskUserTool` 原先把桥调用包进 `tokio::spawn`，取消分支返回时只丢弃
    `JoinHandle`，不会取消桥 future；对未主动观察 cancel 的桥实现会遗留后台任务。修复为在
    `tokio::select!` 中直接等待 `bridge.ask_user(ctx, request)`，取消时直接 drop 桥 future。该修复暴露并一并修复
    `IpcApproval::emit_and_await` 的 pending 生命周期缺口：当交互等待 future 被 drop（例如 ask_user cancel 抢占）时，
    pending map 中的 request id 可能残留，迟到响应会命中已失效 sender 并报 Backend。新增 `PendingCleanup` guard，
    让任意被 drop 的交互等待自动移除 pending 项；正常响应和 cancel 分支下重复移除为 no-op。
  - 新增/增强测试：`mag-tools` 新增
    `ask_user_in_flight_cancellation_drops_the_bridge_future`，验证运行中取消会释放桥 future 且快速返回 cancelled；
    `mag-core` 增强 `ask_user_cancel_unblocks_the_parked_tool`，保存 `request_id` 并断言 cancel 后迟到
    `respond_interaction` 返回 `InteractionNotFound`，证明 pending 已清理。
  - 门禁结果：1) `cargo fmt --all -- --check` ✅ 2) 聚焦测试 ✅：`cargo test -p mag-tools ask_user`
    （5 passed）与 `cargo test -p mag-core ask_user`（3 passed）3) `cargo clippy --all-targets -- -D warnings` ✅
    4) `cargo test --workspace` ✅（全绿，1 ignored 为既有 zed 联调测试）5)
    `cargo doc --no-deps --workspace` ✅（0 warning）。
  - review 结论：M5 的 `ask_user` 普通 ToolPlugin 方案符合 `docs/CLI.md` §5 P6 / 决策 D6；本 review 发现并修复
    1 处 cancel/pending 生命周期缺陷，交互桥复用、取消语义、工具面开关与文档均满足要求。**M5 通过**。

---

## Milestone M6 — mag-cli crate（`docs/CLI.md` §1/§2）

目标：最小 CLI 验证原型——「符合 GUI/web 使用模式」风格的最小实现，验证端到端管线。易用性/美观不考虑。

### M6-1 [DONE] `mag-cli` 骨架：双任务 REPL + 基本对话渲染

- **上下文**：`docs/CLI.md` §1.1（crate 与依赖边界）/§1.2（双任务结构）。**硬约束**：只依赖
  `mag-service`（+ rustyline/tokio/futures/serde/serde_json）。
- **实现要求**：
  - 新建 `crates/mag-cli`，加入 workspace members；`#![warn(missing_docs)]`。
  - `Cli::run(service: Arc<dyn MagService>, opts)` 入口：input 任务（rustyline 读行）+ render 任务
    （`subscribe` 事件流渲染到 stdout）；两任务经 channel 协调； rustyline 与流式输出共享 stdout 的
    最小互斥（渲染时暂停 prompt 回显即可，不做高级 TUI）。
  - 基本对话：输入行 → `send_message`；`TextDelta` 流式原样写 stdout；`RunFinished` 换行 +
    可选 usage 摘要；`RunError` 打印 kind + message。
  - 会话生命周期：启动 `create_session`（`/new` 重建）；`/quit` 退出（delete 与否按 §2 命令面）。
- **验证条件**：聚焦测试：scripted `Arc<dyn MagService>` + 管道 stdin/stdout e2e——输入两行消息断言
  stdout 含流式文本与 finish 摘要。默认验证序列全过。

  **完成记录**（2026-07-20）：
  - 实现要点：新增 `crates/mag-cli`（workspace member，`#![warn(missing_docs)]`），直接依赖边界为
    `mag-service` + `futures`/`tokio`/`rustyline`（dev 侧 `async-trait` 仅用于 scripted service 测试），不依赖
    `mag-core` / `agent-lib`。公开入口 `Cli::run(Arc<dyn MagService>, CliOptions)`：TTY stdin 使用
    `rustyline::DefaultEditor` 读行并保留历史；非 TTY 自动走同一 pipe-friendly 读行器，供 headless e2e 驱动。
    另提供 `Cli::run_with_io` 测试/嵌入入口。
  - 双任务结构：启动先 `create_session`（默认 `openai` / `gpt-5-codex`，`RoutingMode::default()`），随后启动
    input task 与 render task，经 `tokio::mpsc` 向 coordinator 汇报输入行、EOF、I/O 错误与 run 终态。stdout 经
    `Arc<tokio::sync::Mutex<_>>` 串行写入；TTY 与 pipe prompt 都走同一锁，满足最小互斥。render 订阅
    `service.subscribe(None)`，这样 `/new` 切换会话后无需重建渲染流，也能保留全局事件可见性；多会话精细过滤与
    命令面扩展留给 M6-3。
  - 基本对话/会话生命周期：普通非空输入调用 `send_message(current_session, UserInput::text(line))`；
    `TextDelta` 原样流式写 stdout；`RunFinished` 在必要时补打印 final text（无 delta 的服务也可见输出），再打印
    `[finished ...]` 摘要（含 usage 时输出 input/output/total tokens）；`RunError` 打印 kind + message。
    slash 命令已实现 M6-1 范围内的 `/new`、`/help`、`/quit`，未知 slash 命令打印错误但不中断 REPL。`/quit`/EOF
    会等待已启动 run 的 terminal 事件写出后再退出，避免 pipe e2e 丢尾部输出。
  - 测试：新增 `crates/mag-cli/tests/e2e.rs`，使用 scripted `Arc<dyn MagService>` + `tokio::io::duplex` 管道，
    全离线、无真实 LLM/凭据/网络。覆盖两条消息输入 → 记录两次 `send_message`、stdout 含两个流式文本与
    `[finished usage ...]` 摘要；覆盖 `/new` 创建并切换到新 session，后续消息落到新 `SessionId`。
  - 门禁结果：1) `cargo fmt --all -- --check` ✅ 2) `cargo test -p mag-cli` ✅（2 passed）3)
    `cargo clippy --all-targets -- -D warnings` ✅ 4) `cargo test --workspace` ✅（全绿，1 ignored 为既有 zed
    联调测试）5) `cargo doc --no-deps --workspace` ✅（0 warning）。

### M6-2 [DONE] PromptCoordinator：交互提示（审批 + Question/Choice）

- **上下文**：`docs/CLI.md` §1.2/§3.3；单一 pending 交互队列；origin 标注渲染（决策 D5）。
- **实现要求**：
  - PromptCoordinator：`InteractionRequested` 入队（同会话同一时刻只提示一条，其余排队）；render 任务在
    合适时机（当前无流式输出进行中）弹出提示。
  - 审批提示：显示 tool 名/输入摘要/origin（`[from <delegate>@depth<n>]` 前缀），读 y/n（+ always/
    never 若 wire 支持映射）→ `respond_interaction(Approval{decision})`。
  - Question → 读一行文本 → `Answer{text}`；Choice → 编号菜单读数字 → `Choice{index}`。
  - 超时不做（验证原型）；Ctrl-C 在 pending 交互中等价 cancel 决策（`ApprovalDecisionWire::Cancel` /
    对应变体）。
- **验证条件**：e2e：scripted service 发审批/问题/选择交互（含 delegate origin），断言提示文本含 origin
  标注、回答正确回灌、多条交互按序处理。默认验证序列全过。

  **完成记录**（2026-07-20）：
  - 实现要点：`mag-cli` 新增 `PromptCoordinator`（单一 pending 队列 + active prompt），render task 遇到
    `ServiceEvent::InteractionRequested` 不直接输出，而是通过 `RenderNotice::Interaction` 交给 coordinator；当前
    会话的交互逐条提示并经 `MagService::respond_interaction` 回灌，其余会话只打印待处理提示。输入行在 active
    prompt 存在时优先解释为交互回答，否则仍走既有 slash/普通消息分派。Ctrl-C（rustyline
    `Interrupted`）在 pending 交互中发送保守取消响应；EOF 会取消已排队交互，避免关闭 stdin 后 run 永久挂起。
  - 提示与响应：Approval 渲染 origin 前缀、`tool_call`、`ApprovalRequirementWire::RequireApproval.reason`
    摘要并接受 `y/n/cancel`；由于冻结 `InteractionKindWire::Approval` 只携带 `call_id` + requirement，CLI 不伪造
    不存在的 tool name/input 字段（与 mag-acp 同一 wire 约束），响应用占位 `step_id`，由 mag-core 按
    `request_id` 重建真实 step/call id。Question 读取整行文本为 `Answer{text}`；Choice 渲染 1-based 编号菜单并
    回传 0-based `Choice{index}`；Permission 也按同一队列做保守 y/n/cancel 支持，覆盖完整
    `InteractionRequested` family。
  - 测试：`crates/mag-cli/tests/e2e.rs` 的 scripted service 增加三交互脚本与 response 记录；新增
    `prompt_coordinator_answers_queued_interactions_in_order`，通过管道按提示同步输入，断言 delegate origin
    `[from researcher@depth1]` 出现在审批提示中、requirement reason 可见、Question/Choice 提示正确、三条
    `respond_interaction` 按 `REQ_APPROVAL -> REQ_QUESTION -> REQ_CHOICE` 顺序回灌，Approval 为 Approve、Question
    文本原样、Choice index 为 1。既有两条 M6-1 e2e 继续覆盖普通对话与 `/new`。
  - 依赖边界：`cargo tree -p mag-cli -e normal --depth 1` 为 futures / mag-service / rustyline / tokio，未直接依赖
    mag-core / agent-lib / mag-config。
  - 门禁结果：1) `cargo fmt --all -- --check` ✅ 2) `cargo clippy --all-targets -- -D warnings` ✅（初次发现
    `large_enum_variant` 与 `collapsible_str_replace`，已修）3) `cargo test -p mag-cli` ✅（3 passed）4)
    `cargo test --workspace` ✅（全绿，1 ignored 为既有 zed 联调测试）5) `cargo doc --no-deps --workspace` ✅。

### M6-3 [DONE] pivot/cancel/会话命令

- **上下文**：`docs/CLI.md` §2/§3.2（决策 D1 第二层在 CLI）。
- **实现要求**：
  - run 进行中输入普通文本 → 先 `pivot_message`，`NotPivotable` 自动回落 `send_message`（两层语义，用户
    无感）；`PivotQueued/Applied/Dropped` 渲染为一行状态提示。
  - Ctrl-C：run 进行中 → `cancel`；pending 交互 → cancel 决策（M6-2）；Idle → 忽略（不退出，退出用
    `/quit`）。
  - slash 命令：`/new`、`/sessions`（列表，标当前）、`/resume <id>`、`/delete <id>`、`/cancel`、
    `/sources`、`/help`、`/quit`——行为按 §2 命令面。
- **验证条件**：e2e：run 中输入触发 pivot（断言先 pivot 后无回落）；scripted `NotPivotable` 时断言回落
  `send_message`；Ctrl-C 路径（用信号或注入点）；各 slash 命令调用正确 service 方法。默认验证序列全过。

  **完成记录**（2026-07-20）：
  - 实现要点：`mag-cli` 主循环从全局 in-flight 计数改为按 `SessionId` 跟踪活动 run；普通文本在当前会话
    run 进行中时先调 `pivot_message`，仅 `ServiceError::NotPivotable` 自动回落 `send_message`，保持
    `docs/CLI.md` §3.2 决策 D1 的两层语义。`PivotQueued` / `PivotApplied` / `PivotDropped` 事件新增一行状态
    渲染。Ctrl-C：pending 交互仍走 M6-2 的 cancel 决策；当前会话 run 进行中时调 `cancel(session_id)`；Idle
    时忽略。非 TTY e2e 增加单独 ASCII ETX 行（`\u{3}`）作为 Ctrl-C 注入点。
  - slash 命令：补齐 `/sessions`（列表并用 `*` 标当前）、`/resume <id>`、`/delete <id>`、`/cancel`、
    `/sources`（依次调用 `list_sources` 与 `probe_local_agents` 并打印两组结果）和更新后的 `/help`；保留
    `/new`、`/quit`。命令错误以 `[error]` 打印并保持 REPL 存活。
  - 竞态修复：`NotPivotable` 回落路径可能在同一 session 上立即启动新 run，而旧 run 的 terminal 事件稍后才
    到达；若直接按 session 清活动标记，`/quit` 可提前退出并丢新 run 输出。实现上在 NotPivotable 回落成功启动
    新 run 时记录一次“跳过下一条旧 terminal”的计数，保证旧 terminal 不会清掉回落后新 run 的活动状态。
  - 测试（全部离线，scripted `Arc<dyn MagService>` + 管道 stdin/stdout）：`mag-cli` e2e 从 3 个扩到 7 个，新增
    `text_during_an_in_flight_run_uses_pivot_without_falling_back`、`not_pivotable_falls_back_to_send_message`、
    `ctrl_c_during_an_in_flight_run_cancels_the_current_session`、
    `slash_commands_call_the_matching_service_methods`；scripted service 记录 pivot/cancel/session/source 调用并
    模拟 pivot 成功、NotPivotable 竞态回落和取消终态。既有 M6-1/M6-2 对话、`/new`、交互队列测试继续通过。
  - 依赖边界：`cargo tree -p mag-cli -e normal --depth 1` 仅为 futures / mag-service / rustyline / tokio，未直接依赖
    mag-core / agent-lib / mag-config。
  - 门禁结果：1) `cargo fmt --all -- --check` ✅ 2) `cargo test -p mag-cli` ✅（7 passed）3)
    `cargo clippy --all-targets -- -D warnings` ✅ 4) `cargo test --workspace` ✅（全绿，1 ignored 为既有 zed 联调测试）
    5) `cargo doc --no-deps --workspace` ✅（0 warning）。

### M6-4 [DONE] `/config` 命令

- **上下文**：`docs/CLI.md` §2 + §4（决策 D2 生效时机）。
- **实现要求**：
  - `/config show`：打印 `get_config()` 的 TOML 形态（secret 引用原样显示，不物化）。
  - `/config reload`：`reload_config()`，打印结果（新 revision 或错误）。
  - `/config apply`：`apply_config()`，打印「将在各会话下一 turn 边界生效」语义提示；
    `ConfigChanged{revision}` 事件渲染一行提示。
- **验证条件**：e2e：三个子命令调用正确 service 方法并渲染预期输出；`ConfigChanged` 事件到达时打印。
  默认验证序列全过。

  **完成记录**（2026-07-20）：
  - 实现要点：`mag-cli` 补齐 `/config <show|reload|apply>` 子命令并纳入 `/help`。`/config show` 调
    `MagService::get_config()`，直接使用 `mag-service::ConfigDto` re-export 的 `to_string_pretty()` 输出 TOML
    形态，secret 保持 DTO 引用形态、不会物化；空配置输出 TOML 注释 `# empty config`，避免无可见反馈。
    `/config reload` 调 `reload_config()` 并打印 `[config reloaded]`；因 service 契约返回 `()` 不携带 revision，
    新 revision 通过事件流的 `ConfigChanged{revision}` 一行提示体现。`/config apply` 调 `apply_config()` 并打印
    “changes will apply at each session's next turn boundary” 的 D2 生效时机提示。
  - 事件渲染：`ServiceEvent::ConfigChanged{revision}` 新增渲染为
    `[config changed revision=<n>]`，全局订阅流到达时即可提示配置快照 revision 更新。
  - 测试：`crates/mag-cli/tests/e2e.rs` 的 scripted `Arc<dyn MagService>` 新增配置方法成功路径、调用计数与
    `ConfigChanged{revision:42}` 事件；新增
    `config_commands_call_service_methods_and_render_changes`，逐步管道驱动 `/config show`、`/config reload`、
    `/config apply`，断言 TOML 输出包含 provider/agent/tool 配置、三个 service 方法各调用一次、reload 成功提示
    与 `ConfigChanged` revision 提示均出现、apply 提示包含下一 turn 边界语义。
  - 依赖边界：`crates/mag-cli/Cargo.toml` 未变；`mag-cli` 仍只直接依赖 `mag-service` +
    `futures`/`tokio`/`rustyline`，未直接依赖 `mag-core` / `agent-lib` / `mag-config`。
  - 门禁结果：1) `cargo fmt --all -- --check` 初次发现 rustfmt diff，`cargo fmt --all` 修复后复检 ✅
    2) `cargo test -p mag-cli` ✅（8 passed）3) `cargo clippy --all-targets -- -D warnings` ✅
    4) `cargo test --workspace` ✅（全绿，1 ignored 为既有 zed 联调测试）5)
    `cargo doc --no-deps --workspace` ✅（0 warning）。

### M6-5 [DONE] bin 装配 + 端到端验证

- **上下文**：`docs/CLI.md` §1.1/§5/§6；bin 在 `crates/mag/src/main.rs`（现有 `--acp`）。
- **实现要求**：
  - bin 子命令：`mag`（默认 CLI）、`mag --resume <id>`、`mag --config <path>`（M3-6）、`mag --acp`
    （保留不变）；CLI 路径：读配置 → ConfigService → `Engine::from_config` → `Cli::run`。
  - 端到端 e2e：fake LLM 装配真实 Engine + mag-cli（管道 stdio），跑通：对话 → ask_user 交互 → 委派
    （local + fake external ACP）→ pivot → cancel → `/config reload` → `/resume` 恢复后继续对话。
    可分多个 e2e 测试，全部离线。
- **验证条件**：上述 e2e 全绿；默认验证序列全过。

  **完成记录**（2026-07-20）：
  - 实现要点：`crates/mag/src/main.rs` 从“必须 `--acp`”改为默认启动终端 CLI：`mag` / `mag --config
    <path>` 读配置 → `ConfigService` → `Engine::from_config` → `mag_cli::Cli::run`；`mag --acp` 保持原 ACP
    stdio 路径不变并共用同一配置装配；新增 `--resume <id>` / `--resume=<id>`，启动时传给 CLI 恢复已有
    session，且显式拒绝 `--acp --resume`。usage 同步说明默认 CLI、`--config`、`--resume`、`--acp`。
  - `mag-cli` 接线：`CliOptions` 新增 `resume: Option<SessionId>`；REPL 启动时若给定 resume id，则先调用
    `resume_session(id)` 并打印 `[session <id> resumed]`，否则沿用原 `create_session` 路径。`/new`、`/resume`
    slash 命令和既有 pipe/TTY 双任务结构不变；`mag-cli` crate 的 normal 依赖仍仅为 `mag-service` +
    futures/tokio/rustyline。
  - bin smoke：更新 `crates/mag/tests/cli.rs` 到 M6-5 语义——默认 `mag` 可用内置默认配置启动 CLI 并 `/quit`；
    corrupt config 在默认 CLI 路径会被加载并失败；`--resume` 用 provider-backed、env-secret-only（不发送消息、
    不触网）的持久化配置验证跨进程恢复；既有 `--acp` initialize handshake、missing config 和 secret 诊断测试
    继续覆盖 ACP 路径不回归。
  - 真实 Engine + CLI 离线 e2e：新增 `crates/mag/tests/engine_cli.rs`，通过本地 scripted `LlmClient` +
    `Cli::run_with_io` 管道驱动真实 `mag_core::Engine`，覆盖：普通流式对话、`ask_user` Question 回灌、local
    `ask_researcher` 委派、`/config reload` 与 `ConfigChanged` 渲染、门控工具场景下 pivot queued/applied、stalling
    stream 下 Ctrl-C cancel、fake external ACP 进程 `ask_peer` 委派，以及 `/resume <id>` 后继续对话。fake ACP 为本地
    shell 脚本，测试只读写 tempdir，不依赖网络/真实凭据/真实 LLM。
  - 依赖边界：`mag` 作为顶层装配 crate 新增 normal 依赖 `mag-cli`；新增测试 dev-deps（agent-lib、mag-tools、
    futures、serde_json、async-trait、tokio io/time）仅用于离线 fake LLM/工具 e2e；`mag-cli` normal 依赖未增加
    mag-core/agent-lib/mag-config。
  - 门禁结果：1) `cargo fmt --all -- --check` ✅ 2) 聚焦测试 `cargo test -p mag-cli` ✅（8 passed）与
    `cargo test -p mag` ✅（bin smoke 8 passed + Engine/CLI e2e 3 passed）3)
    `cargo clippy --all-targets -- -D warnings` ✅ 4) `cargo test --workspace` ✅（全绿，1 ignored 为既有 zed 联调
    测试）5) `cargo doc --no-deps --workspace` ✅（0 warning）。

### M6-R [DONE] M6 review

- **实现要求**：对照 `docs/CLI.md` §0 目标清单逐项核对验证覆盖：流式对话/工具权限/通用交互/多 agent
  编排（含 external ACP）/协作/持久化恢复/cancel/pivot/配置动态生效。检查依赖边界（`cargo tree -p
  mag-cli` 无 mag-core/agent-lib/mag-config）。发现问题直接修复并补测试。
- **验证条件**：默认验证序列全过；完成记录逐项列出 §0 验证清单结论。

  **完成记录**（2026-07-20）：
  - review 范围：对照 `docs/CLI.md` §0 目标清单与 §1/§2/§3.3 关键 CLI 语义，复核 `crates/mag-cli`
    实现与 scripted e2e、`crates/mag` bin smoke、真实 Engine+CLI e2e、M6-1..M6-5 当前 diff；并检查
    `mag-cli` 依赖边界。
  - 发现与修复（4 项，均已修并补测试）：
    1. **后台会话 interaction 被丢弃**：`PromptCoordinator::enqueue` 原先对非当前会话只打印提示后返回，
       `/resume` 回去后无法应答，可能让后台 run 永久 pending。修复为队列保留所有会话 interaction，
       `prompt_next(current_session)` 只弹当前会话项；切换会话后立即恢复该会话 pending prompt；`/quit` 时取消
       已排队交互，避免退出等待被 pending run 卡住。新增
       `background_session_interaction_is_answered_after_resume`。
    2. **delegation 事件未渲染**：`render_event` 原先吞掉 `DelegationStarted/Finished/Failed/Message`，与
       §0「Delegation* 事件渲染」不符。新增一行摘要渲染（delegate/task/output/message），并顺手补
       `ToolStarted/ToolFinished` 摘要，方便 CLI 观察工具执行面。新增 `delegation_events_are_rendered`。
    3. **Interaction 四种 kind 覆盖不全**：scripted e2e 原先只覆盖 Approval/Question/Choice。扩展
       `prompt_coordinator_answers_queued_interactions_in_order`，加入 Permission（含 delegate origin、category、risk、
       subject、reason）并断言 `PermissionDecisionWire::Approve` 正确回灌。
    4. **真实 Engine+CLI e2e 覆盖偏弱**：补强 `engine_cli.rs`：`/config reload` 后执行 `/config apply` 并断言
       下一 turn 使用新 model；新增跨 Engine 持久化恢复测试，第二个 Engine `resume` 后继续对话且 LLM request
       上下文包含重启前 user/assistant 历史；pivot 测试新增断言 pivot 文本进入后续 LLM request；local/external
       delegation e2e 均断言 CLI 输出包含 delegation started/finished 与 delegate 名。
  - §0 验证清单结论：
    1. **基本 agent 对话 + 流式输出** ✅——scripted `mag-cli` e2e 覆盖 `send_message`、`TextDelta` 分块打印、
       `RunFinished` usage 摘要；真实 Engine+CLI 对话路径继续覆盖。
    2. **用户交互** ✅——Approval/Question/Choice/Permission 四类均经 CLI prompt 渲染并通过
       `respond_interaction` 回灌；delegate origin 前缀与后台会话切换后应答均有 e2e。
    3. **多 agent 编排** ✅——真实 Engine+CLI 覆盖 local `ask_researcher` 与 external ACP `ask_peer`；CLI 现在渲染
       DelegationStarted/Finished/Failed/Message，其中 production 当前会发 started/finished，Message/Failed 渲染由
       scripted service 覆盖事件族。
    4. **agent 间协作** ✅——M4/M5 已验证 delegate 交互穿透到 root；M6 scripted e2e 验证 CLI 对带 origin 的
       delegate interaction 逐项提示/应答，真实 Engine+CLI 覆盖 local/external 委派路径可见。
    5. **会话持久化与恢复** ✅——bin smoke 覆盖 `--resume`；新增真实 Engine+CLI 跨 Engine restart 恢复，断言
       恢复后继续对话时上下文包含重启前 committed 历史；`/sessions`/`/resume`/`/delete` scripted 命令面继续覆盖。
    6. **cancel + pivot** ✅——scripted CLI 覆盖 run 中 pivot、`NotPivotable` 自动回落、Ctrl-C cancel；真实 Engine+CLI
       覆盖 pivot queued/applied、pivot 文本进入下一次 LLM request、stalling run Ctrl-C 后 `RunError{cancelled}`。
    7. **运行时配置系统** ✅——bin 路径覆盖启动读配置和缺失/损坏/secret 诊断；CLI `/config show|reload|apply`
       scripted e2e 覆盖 service 方法与 `ConfigChanged` 渲染；真实 Engine+CLI 覆盖 reload 后显式 apply 到 live idle
       session，下一 turn 使用新配置。
  - 依赖边界：`cargo tree -p mag-cli -e normal --depth 1` 显示 direct normal 依赖仅为 `futures`、`mag-service`、
    `rustyline`、`tokio`；源码/Cargo.toml 无直接 `mag-core`/`agent-lib`/`mag-config` 引用。完整 tree 中
    `mag-config` 仅通过 `mag-service` 的契约 DTO re-export 传递出现（M3-4 决策），无 `mag-core`/`agent-lib`。
  - 门禁结果：1) `cargo fmt --all -- --check` ✅ 2) 聚焦测试 ✅：`cargo test -p mag-cli`（10 passed）与
    `cargo test -p mag --test engine_cli`（4 passed）3) `cargo clippy --all-targets -- -D warnings` ✅
    4) `cargo test --workspace` ✅（全绿，1 ignored 为既有 zed 联调测试）5) `cargo doc --no-deps --workspace` ✅
    （0 warning）。

---

## F-R [TODO] 全计划 review

- **实现要求**：全部里程碑完成后，对整轮改动做一次完整 review（可分子代理分块）：对照 `docs/CLI.md`
  全节（含决策 D1–D6）逐条核对；重点：冻结契约只加不改、配置系统 DTO↔DO 与快照隔离、pivot/cancel 竞态、
  delegate restore 完备性、external ACP 生命周期、依赖边界、离线测试纪律、rustdoc 完整性。发现的问题直接
  修复并补测试。
- **验证条件**：默认验证序列全过（含整个 workspace）；完成记录列出 review 发现与修复清单。
