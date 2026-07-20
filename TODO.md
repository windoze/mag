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

### M3-2 [TODO] mag-config：DTO ↔ DO 双向转换

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

### M3-3 [TODO] mag-core：`ConfigService`

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

### M3-4 [TODO] mag-service：配置方法 + `ConfigChanged` 事件

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

### M3-5 [TODO] mag-core：turn-complete 通知/回调机制 + `apply_config`

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

### M3-6 [TODO] `Engine::from_config` + bin 读配置

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

### M3-R [TODO] M3 review

- **实现要求**：对照 `docs/CLI.md` §4 全节检查：DTO↔DO 双向无损、快照隔离（update 不影响已钉住会话）、
  write-through 原子性、watch 回环防护、turn-complete 边界语义、secret 不物化不输出、GUI/web 可用性
  （四方法 + 事件足以驱动配置 UI）。发现问题直接修复并补测试。
- **验证条件**：默认验证序列全过；完成记录列出 review 结论。

---

## Milestone M4 — delegation 接线（`docs/CLI.md` §5 P7，决策 D3）

目标：external ACP agent 是本程序核心功能（决策 D3：尽早动手）。model-routed `ask_<name>` 委派两条来源
全部落地：local LLM subagent（agent-lib `Agent::worker()`）与 external ACP agent
（`ManagedExternalAgent::acp`）。

### M4-1 [TODO] mag-core：local LLM subagent 委派 + `Delegation*` 事件映射

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

### M4-2 [TODO] mag-core：external ACP agent 委派（决策 D3，核心）

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

### M4-3 [TODO] 委派审批 + restore 重注册 delegate

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

### M4-R [TODO] M4 review

- **实现要求**：对照 `docs/CLI.md` §5 P7 与决策 D3 检查：两条来源行为一致（事件、审批、origin）、
  external 生命周期清扫、restore 完备性、feature gating 正确（不开 feature 时编译过、external 配置报
  明确错误）。发现问题直接修复并补测试。
- **验证条件**：默认验证序列全过；完成记录列出 review 结论。

---

## Milestone M5 — ask_user 工具（`docs/CLI.md` §5 P6，决策 D6）

目标：AskUserQuestion 式通用交互作为普通 plugin 先行（GUI 阶段再详细设计）。

### M5-1 [TODO] mag-tools：`ask_user` ToolPlugin

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

### M5-R [TODO] M5 review

- **实现要求**：对照 `docs/CLI.md` §5 P6 检查：交互桥复用一致、cancel 语义、tool profile 开关、rustdoc。
  发现问题直接修复并补测试。
- **验证条件**：默认验证序列全过；完成记录列出 review 结论。

---

## Milestone M6 — mag-cli crate（`docs/CLI.md` §1/§2）

目标：最小 CLI 验证原型——「符合 GUI/web 使用模式」风格的最小实现，验证端到端管线。易用性/美观不考虑。

### M6-1 [TODO] `mag-cli` 骨架：双任务 REPL + 基本对话渲染

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

### M6-2 [TODO] PromptCoordinator：交互提示（审批 + Question/Choice）

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

### M6-3 [TODO] pivot/cancel/会话命令

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

### M6-4 [TODO] `/config` 命令

- **上下文**：`docs/CLI.md` §2 + §4（决策 D2 生效时机）。
- **实现要求**：
  - `/config show`：打印 `get_config()` 的 TOML 形态（secret 引用原样显示，不物化）。
  - `/config reload`：`reload_config()`，打印结果（新 revision 或错误）。
  - `/config apply`：`apply_config()`，打印「将在各会话下一 turn 边界生效」语义提示；
    `ConfigChanged{revision}` 事件渲染一行提示。
- **验证条件**：e2e：三个子命令调用正确 service 方法并渲染预期输出；`ConfigChanged` 事件到达时打印。
  默认验证序列全过。

### M6-5 [TODO] bin 装配 + 端到端验证

- **上下文**：`docs/CLI.md` §1.1/§5/§6；bin 在 `crates/mag/src/main.rs`（现有 `--acp`）。
- **实现要求**：
  - bin 子命令：`mag`（默认 CLI）、`mag --resume <id>`、`mag --config <path>`（M3-6）、`mag --acp`
    （保留不变）；CLI 路径：读配置 → ConfigService → `Engine::from_config` → `Cli::run`。
  - 端到端 e2e：fake LLM 装配真实 Engine + mag-cli（管道 stdio），跑通：对话 → ask_user 交互 → 委派
    （local + fake external ACP）→ pivot → cancel → `/config reload` → `/resume` 恢复后继续对话。
    可分多个 e2e 测试，全部离线。
- **验证条件**：上述 e2e 全绿；默认验证序列全过。

### M6-R [TODO] M6 review

- **实现要求**：对照 `docs/CLI.md` §0 目标清单逐项核对验证覆盖：流式对话/工具权限/通用交互/多 agent
  编排（含 external ACP）/协作/持久化恢复/cancel/pivot/配置动态生效。检查依赖边界（`cargo tree -p
  mag-cli` 无 mag-core/agent-lib/mag-config）。发现问题直接修复并补测试。
- **验证条件**：默认验证序列全过；完成记录逐项列出 §0 验证清单结论。

---

## F-R [TODO] 全计划 review

- **实现要求**：全部里程碑完成后，对整轮改动做一次完整 review（可分子代理分块）：对照 `docs/CLI.md`
  全节（含决策 D1–D6）逐条核对；重点：冻结契约只加不改、配置系统 DTO↔DO 与快照隔离、pivot/cancel 竞态、
  delegate restore 完备性、external ACP 生命周期、依赖边界、离线测试纪律、rustdoc 完整性。发现的问题直接
  修复并补测试。
- **验证条件**：默认验证序列全过（含整个 workspace）；完成记录列出 review 发现与修复清单。
