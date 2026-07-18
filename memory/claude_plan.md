# 执行计划

## 当前约束
- 以 `TODO.md` 为唯一任务顺序和完成状态来源。
- 本次只完成第一个标题未带 `[DONE]` 的任务，然后停止。
- 任务完成后必须更新 `TODO.md` 的任务标题和完成记录，并提交 Git commit。
- 只有阶段级计划变化才更新 `PLAN.md`。
- 在运行完整测试前，先执行 `cargo fmt`，再执行 `cargo clippy --all-targets -- -D warnings`。
- 不公开不可见的内部推理；本文件记录可审计的执行计划、决策和进度。

## 初始步骤
1. 读取 `TODO.md`，定位第一个标题未带 `[DONE]` 的任务。
2. 查看最近一次提交，确认是否有明确提到且直接影响当前任务的未完成问题。
3. 根据当前任务读取相关源码、测试和文档，确认实现边界。
4. 若发现当前任务被具体缺失前置条件阻塞，则把最小前置任务插入 `TODO.md` 的正确位置，保持当前任务未完成，提交后停止。

## 实施步骤
1. 按当前任务要求进行最小但完整的实现。
2. 为新增或改变的行为补充有针对性的测试。
3. 运行格式化、lint 和相关测试；必要时修复发现的问题。
4. 若观察到未被后续任务明确安排的失败测试，优先修复或把必要前置任务写入 `TODO.md`。
5. 更新 `TODO.md`：给完成任务标题添加 `[DONE]` 前缀，并填写完成记录。
6. 只在阶段计划确实变化时更新 `PLAN.md`。
7. 检查 Git diff，提交本次所有相关改动，然后停止。

## 进度记录
- 已创建本计划文件，下一步读取 `TODO.md` 并定位第一个未完成任务。
- 已读取 `TODO.md`；第一个未完成任务是 `C1-2 自组 scope 驱动一次对话 turn`。
- 下一步检查最近提交是否明确提到直接影响 `C1-2` 的未完成问题，然后阅读相关源码与 agent-lib 样板。
- 最近提交为 `[C1-1] Add streaming LLM tap handler`，未在提交标题中声明直接阻塞 `C1-2` 的未完成事项。
- 已阅读当前 `mag-core` engine/llm/event_bus/ids、`mag-protocol` 输出结构，以及 agent-lib `agent_chat.rs`、`HandlerScope`、`drain`、`AgentSpec`/`AgentState`/`Conversation` 相关 API。
- 已新增 `crates/mag-core/src/driver.rs`：创建 `SessionDriver`、`MagScope`，用 `DefaultAgentMachine` + `drain` 推进单轮对话，并从 `Conversation` 提取最终文本和 usage。
- 已修改 `Engine`：新增 `with_llm_client` 注入点，`CreateSession` 建立会话 driver，`SendMessage` 路由到单轮 driver。
- 已添加 `engine::chat` 测试覆盖有序事件流和同一会话历史累积；下一步运行格式化和聚焦测试。
- 已运行 `cargo fmt --all` 与 `cargo test -p mag-core engine::chat`，聚焦测试 2 个用例全绿。
- 下一步执行 `cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`、`cargo doc --no-deps --workspace`。
- 已完成默认验证序列：格式检查、聚焦测试、clippy、workspace 测试、文档构建均通过。
- 已将 `TODO.md` 中 `C1-2 自组 scope 驱动一次对话 turn` 标记为 `[DONE]` 并补充完成记录。
- 收尾前补跑格式检查和文档构建；随后检查 diff 并提交。
- 已补跑 `cargo fmt --all -- --check` 与 `cargo doc --no-deps --workspace`，均通过。
- 已运行 `git diff --check`，无空白错误；下一步提交本次所有相关改动。

## C1-3 切换到 facade `Agent` 注入路径（当前任务）

### 目标
用 facade `Agent`（`Agent::builder().client(..).model(..).build()` + `agent.stream(input)`）
替换 C1-1/C1-2 的自组 `DefaultAgentMachine`+`MagScope`+`drain` 装配，逐 `RunEvent`→`to_wire()`→mag `Event`。

### 决策
- 移除自建 `StreamingTapHandler`（C1-1）：facade `Agent::stream` 内部已产 `TextDelta`。
- `FakeLlmClient` 夹具保留（离线测试），经 `AgentBuilder::client(..)` 注入。
- 事件映射：消费 `RunEvent` → `RunEvent::to_wire()` → `WireRunEvent`，
  - `TextDelta(text)` → `Event::TextDelta`
  - `Done(WireRunOutput)` → 收集为 mag `RunOutput`（text=reply.text()，usage=usage.total()），循环后 emit `RunFinished`
  - 其余变体（Tool/Approval/Delegation/Raw）C1 纯对话不产出，忽略，留待 C3+。
- run id 信封：facade 不外露 run id，mag 用 per-session 计数器铸造 `RunId` 供 `RunStarted`。
- `MagIds`（C0-3）决定：删除 `ids.rs` 及 `pub use ids::MagIds`——facade `FacadeIds` 接管全部身份铸造，
  `MagIds` 的 `RequirementIds`/`ToolExecutionIds` 实现已成自组 scope 残留死代码。
- driver 延迟构建 facade `Agent`（首次 `SendMessage` 时按 client 构建并缓存），会话内跨轮复用。

### 步骤
1. 重写 `driver.rs`：`SessionDriver` 持 facade `Agent` + run 计数器；`send_message` 走 `agent.stream`。
2. 删除 `StreamingTapHandler`；把 `FakeLlmClient` 夹具迁到 `test_support.rs`（`#[cfg(test)]`）。
3. 删除 `ids.rs` 与 `MagIds` 导出；`lib.rs` 更新模块与导出。
4. `engine.rs`：`SessionManager` 延迟构建 driver；测试 import 改 `crate::test_support`。
5. 验证：fmt → clippy(-D warnings) → `cargo test -p mag-core engine::chat` → workspace 测试 → doc。
6. 标记 `TODO.md` C1-3 `[DONE]`，提交。

### 进度（C1-3 完成）
- 已重写 driver.rs（facade Agent + stream + to_wire 映射）、迁移 test_support.rs、删除 llm.rs 与 ids.rs。
- 已改 engine.rs（延迟构建 driver、session_entry、测试 import）、更新 lib.rs 导出。
- 新增 driver::tests 覆盖 to_wire 映射 + WireRunEvent round-trip；test_support 自测 tool-use。
- 全部验证通过：fmt --check、clippy -D warnings、cargo test -p mag-core engine::chat/driver、
  cargo test --workspace（mag-core 9 绿）、cargo doc --no-deps --workspace。
- 已将 TODO.md C1-3 标 [DONE] 并补完成记录。下一步：git diff 复核并提交。

## C1-R Review：纯对话流式贯通（facade 路径）（当前任务）

### 目标
Review 任务：核对 mag-core 已切到 facade `Agent`+`Agent::stream`（C1-3），未绕过
`Conversation`/machine，`TextDelta` 语义与 `docs/DESIGN.md` §3.4 一致，`WireRunEvent`
映射无丢事件，自组 scope 残留代码已清理。汇总缺口。跑完整验证序列 1–5。

### Review 结论（代码核对）
- driver.rs：`SessionDriver` 持 facade `Agent`（`Agent::builder().client().model()...build()`），
  `send_message` 消费 `agent.stream(text)`，逐 `RunEvent::to_wire() -> WireRunEvent` 映射。✓ 走 facade。
- 未自拼 message Vec / 不重写状态机；历史由 facade `Conversation` 跨轮累积。✓
- `TextDelta` → `Event::TextDelta`；`Done` 折叠为 `RunOutput` 后 emit `RunFinished`，
  与 §3.4「逐 RunEvent → to_wire → mag Event」一致。✓
- `map_wire_event` 对纯对话产出的 `TextDelta`/`Done` 全覆盖，无丢事件；Tool/Approval/
  Delegation/Raw 变体 C1 不产出，明确注释延后到 C3+（已在 C3 任务安排）。✓
- 残留清理：无 `StreamingTapHandler`/`MagScope`/`drain`/`ids.rs`/`MagIds`/`llm.rs`；
  仅 driver.rs 文档注释提及已下沉到 facade。✓
- 无需插入新的前置/阻塞任务；前向缺口（工具/审批/委派映射）已由 C3+ 覆盖。

### 步骤
1. 跑验证序列：fmt --check → clippy -D warnings → cargo test --workspace → cargo doc。
2. 标记 TODO.md C1-R `[DONE]` 并补完成记录（含 review 对照与缺口汇总）。
3. 提交。

### 进度（C1-R 完成）
- 完整验证序列 1–5 全绿：fmt --check、clippy -D warnings（干净）、cargo test --workspace（mag-core 9 + mag-protocol 5 + mag-sources 1 + mag-tools 1）、cargo doc。
- Review 未发现新阻塞缺口；前向变体映射已由 C3+ 覆盖。
- 已将 TODO.md C1-R 标 [DONE] 并补完成记录。下一步：git 提交。

## CS-1 新建 `mag-service` crate + 迁移 Command/Event（并入 mag-protocol）（当前任务）

### 目标
把 `mag-protocol` 整体迁入新的 `crates/mag-service`（crate 名改为 `mag-service`），
删除 `mag-protocol`，更新 workspace 成员、依赖与所有 `use` 路径。CS-1 只迁移协议中立类型，
`MagService` trait 由 CS-2 引入。`mag-service` 仍不依赖 agent-lib（仅 serde/serde_json/uuid）。

### 决策
- 用 `git mv crates/mag-protocol crates/mag-service` 保留历史；文件内容原样迁入（serde 契约/tag 不变）。
- `mag-service/Cargo.toml`：`name = "mag-service"`。更新 crate 级 rustdoc 说明其为 service protocol 载体。
- workspace `Cargo.toml` 成员：`crates/mag-protocol` → `crates/mag-service`。
- `mag-core/Cargo.toml`：`mag-protocol = {path=..}` → `mag-service = {path="../mag-service"}`。
- 源码 use：`mag_protocol::` → `mag_service::`（engine.rs×3、driver.rs×2、event_bus.rs×1）。
- README.md crate 列表：`mag-protocol` → `mag-service`。
- TODO.md/PLAN.md/docs 中已 `[DONE]` 任务或历史记录里的 mag-protocol 字样保持不变（不改完成记录）。

### 步骤
1. git mv 目录；改 Cargo.toml name；更新 crate rustdoc。
2. 更新 workspace 成员、mag-core 依赖、源码 use 路径、README。
3. 验证序列：fmt --check → clippy -D warnings → cargo test --workspace → cargo doc；
   另跑 `cargo tree -p mag-service`（无 agent-lib）、确认无 mag-protocol 代码残留。
4. 标记 TODO.md CS-1 `[DONE]` 并补完成记录，提交。

### 进度（CS-1 完成）
- git mv 迁移 + 改名 mag-service；更新 workspace 成员、mag-core 依赖、6 处 use 路径、README、crate rustdoc。
- 验证全绿：fmt --check、clippy -D warnings、cargo test --workspace（9+5+1+1）、cargo doc；
  cargo tree -p mag-service 无 agent-lib；Cargo.lock/代码无 mag-protocol 残留。
- 已标 TODO.md CS-1 [DONE] 并补完成记录。下一步：git 提交后停止。

---

## 任务 CS-2：定义 `MagService` trait（近全集）+ `ServiceEvent`

### 目标与边界
- 只在 `mag-service` 定义抽象：`MagService` trait + `ServiceError`/`ServiceEvent`/`UserInput`/`SessionInfo` 配套类型。
- 不做 `Engine impl`（属 CS-3）。`SourceInfo` 已存在，直接复用。
- trait 必须 object-safe（供 `Arc<dyn MagService>`），用 `#[async_trait]`。
- `mag-service` 仍不得依赖 agent-lib（`cargo tree -p mag-service` 无 agent-lib）。

### 决策
- 新增 `crates/mag-service/src/service.rs` 模块，lib.rs `mod service; pub use service::*;`，保持已迁移文件历史与 protocol 类型不动（modular）。
- `Cargo.toml` 增加 `async-trait`、`futures`（workspace 版本；均不引入 agent-lib）。
- `ServiceEvent` 为中立事件枚举，变体与 `Event` 对齐（近全集），`#[serde(tag="type", rename_all="snake_case")]` + `#[non_exhaustive]`；附 `session_id()` 便于 subscribe 过滤（CS-3 用）。
- `ServiceError`：`#[non_exhaustive]` 枚举 + serde + `Display`/`Error`。
- `UserInput { text, attachments }`；`SessionInfo { id, config }`（中立，serde）。
- `subscribe(Option<SessionId>) -> BoxStream<'static, ServiceEvent>`（`futures::stream::BoxStream`）。

### 验证
1. `cargo fmt --all -- --check`
2. 聚焦 `cargo test -p mag-service`
3. `cargo clippy --all-targets -- -D warnings`
4. `cargo test --workspace`
5. `cargo doc --no-deps --workspace`
- 额外：`cargo tree -p mag-service` 无 agent-lib。
- object-safe 编译期断言 + `Arc<dyn MagService>` 构造测试；`ServiceEvent` serde round-trip 测试。

### 进度
- 已定位首个未完成任务 CS-2，读毕 DESIGN §3.0、mag-service/lib.rs、mag-core engine/driver/event_bus。下一步：改 Cargo.toml、写 service.rs、接线 lib.rs。
- 已实现 CS-2：新增 service.rs（MagService trait + ServiceEvent/ServiceError/UserInput/SessionInfo），lib.rs 接线导出，Cargo.toml 加 async-trait/futures。
- 完整验证序列全绿；cargo tree -p mag-service 无 agent-lib。已将 TODO.md CS-2 标记 [DONE] 并补完成记录。下一步提交并停止。

---

## 任务 CS-3：`Engine impl MagService` + 既有路径改经 trait（当前任务）

### 目标与边界
- 在 `mag-core` 为 `Engine` 实现 `mag_service::MagService` trait；把 C0/C1 的 `handle_command` 分发下沉为 trait 各方法。
- `subscribe` 包装现有 broadcast `EventBus`，将 `Event` 映射为 `ServiceEvent` 并按 `Option<SessionId>` 过滤。
- 既有 C1/C0 单元测试改经 `MagService` trait 调用 + 消费 `subscribe` 流断言事件序列。
- Command/Event 降为 wire 编码：本任务不做 Command↔trait adapter（DESIGN §3.0/§4）。
- 未实现方法（resume/delete/cancel/respond_interaction/list_sources/probe_local_agents）返回 `ServiceError::Unsupported`，由 C2/C3/C4 落地。

### 决策
- mag-service 新增 `impl From<Event> for ServiceEvent`（两类型同构，字段 1:1），配 round-trip 测试。
- 删除 mag-core 的 `handle_command`/`CommandOutput`/`EngineError`/`emit_unimplemented`（interim 脚手架）。
- 删除 mag-core 自有 `SessionInfo`，改用 `mag_service::SessionInfo`（同形）。
- 移除 `Engine` 的 inherent `subscribe()`，改由 trait `subscribe(Option<SessionId>) -> BoxStream<'static, ServiceEvent>`。
- `send_message(id, input)`：会话不存在→`SessionNotFound`；无 client / driver 构建失败→`Backend`；run 启动后返回 `RunId`，run 内错误经事件流 `RunError` 暴露（driver 内 emit）。
- driver.send_message 改为 mint run_id→emit RunStarted→drive→emit RunFinished/RunError，返回 `RunId`。
- `MagIds`（C0-3）此前已在 C1-3 删除；本任务复核无残留（完成记录说明）。

### 步骤
1. mag-service：加 `From<Event> for ServiceEvent` + 测试。
2. driver.rs：改 send_message 返回 RunId，内部 emit RunFinished/RunError。
3. engine.rs：impl MagService；删 handle_command/CommandOutput/EngineError；用 mag_service::SessionInfo；subscribe 映射+过滤。
4. lib.rs：更新导出。
5. 迁移 engine.rs 内 skeleton/chat 测试到 trait；加经 `Arc<dyn MagService>` 的 create+send+subscribe 断言 RunStarted→TextDelta*→RunFinished。
6. 验证：fmt --check → clippy -D warnings → cargo test -p mag-core → cargo test --workspace → cargo doc；cargo tree -p mag-service 无 agent-lib。
7. 标 TODO.md CS-3 `[DONE]` + 完成记录，提交。

### 阻塞与解决（Send 正确性）
- 阻塞：facade `Agent::stream(&mut self)` 返回的 `AgentRunStream` 非 `Send`，无法在 `#[async_trait]`（默认 Send）
  的 `impl MagService::send_message` future 内联驱动 → 编译报 "future cannot be sent between threads safely"。
- 排查：`AgentRunStream` 内含 `Pin<Box<dyn Future + 'a>>`（无 Send），本质非 Send；但 `Agent` 本身 IS Send（已验证）。
- 解决（非 workaround，DESIGN §3.1 最小实现）：新增 `crates/mag-core/src/run_loop.rs`——`Engine` 拥有一条专用 OS
  线程，内跑 current_thread runtime + block_on 循环，独占 `HashMap<SessionId, SessionDriver>`；`send_message` 只经
  `mpsc`/`oneshot` 交付 `RunLoopCommand::Run`。`RunLoop: Drop` 关 channel + join 线程。仅在 `with_llm_client` 时起线程。
- C2-1 将其重构为 per-session actor + cancel 旁路 + 并发隔离（已在 TODO.md C2-1 上下文补起点说明）。

### 完成
- 全部验证绿：fmt --check、clippy -D warnings、cargo test -p mag-core -p mag-service（12+10）、
  cargo test --workspace --all-targets、cargo doc --no-deps --workspace（无警告）、cargo tree -p mag-service 无 agent-lib。
- TODO.md：CS-3 标 [DONE] + 完成记录；C2-1 上下文补 CS-3 run-executor 起点。提交后停止。

---

## 当前任务：C2-1 per-session driver actor（TODO.md 589 行）

### 目标（TODO.md + DESIGN.md §3.1）
把 CS-3 的单条全局 run-executor（`run_loop.rs`）重构为 **per-session driver actor**：
- 每会话隔离：各会话独占其 facade `Agent`；跨会话 run 并发独立；事件按 `session_id` 分流。
- cancel 旁路：`CancelRun` 走 cancel token，不碰 agent `&mut`，永不被 run 饿死。
- `RespondInteraction` 留 C3 钩子。

### 设计
- **每会话一条专用 OS 线程**（current_thread runtime + `LocalSet`）。命令 `mpsc` sender 在
  `create` 中同步登记，无竞态；actor 线程独立启动并消费无界通道。跨会话真并行。
- actor 持 `SessionDriver`。`SendMessage`：mint run_id → emit `RunStarted` → 提前回 run_id →
  `spawn_local` run task（拥有 driver，驱动 `agent.stream`，受 `CancelToken` 控制）；结束把
  driver 经通道交还 actor。
- `CancelRun`：actor 触发 run 的 `CancelToken`（不碰 agent `&mut`）。run task 的驱动循环用
  `select!` 在 stream 与 `cancel.cancelled()` 间竞争；取消时丢弃 stream（agent-lib
  `AgentRunStream::Drop` 会 abandon 在途 turn，committed 历史不变），emit
  `RunError { message: "run cancelled" }`。
- `RespondInteraction`：路由到 actor 钩子，暂回 `Unsupported`（C3 填充）。
- run 进行中到达的 `SendMessage`：延后处理（会话内串行）。

### CancelToken
- 无新依赖的小 `CancelToken`（`Arc<AtomicBool>` + `tokio::sync::Notify`），可克隆，C3 `IpcApproval` 复用。

### 文件
- 新增 `session.rs`（替换 `run_loop.rs`）：`SessionCommand` / `CancelToken` / `SessionActor` /
  `SessionManager` / `SessionHandle`。
- `driver.rs`：暴露 `next_run_id`；`send_message` → cancel 感知的 `run_turn`。
- `engine.rs`：改用 `SessionManager`；实现 `cancel`（路由 `CancelRun`）与 `delete_session`
  （停 actor + 删元数据）；`respond_interaction` 路由到钩子。新增 `mod session` 测试。
- `test_support.rs`：加 stalling stream 脚本，供 cancel 测试构造"进行中"的 run。
- `lib.rs`：`mod run_loop` → `mod session`。

### 测试（聚焦 `cargo test -p mag-core engine::session`）
1. 两会话并行 SendMessage：TextDelta/RunFinished 按 session_id 分流、不串。
2. 长（stalling）run 中途 CancelRun → 干净终止、emit RunError（cancelled）、无 RunFinished、
   会话仍可用；另一会话不受影响。
- 同步更新 `unimplemented_methods_return_unsupported`（cancel/delete_session 现返回 Ok）。

### 进度
- [x] test_support stalling 脚本
- [x] driver run_turn + next_run_id
- [x] session.rs（actor + manager + CancelToken；`DriverState::Idle` 装箱避免 large_enum_variant）
- [x] engine 接线 + cancel/delete/respond
- [x] engine::session 测试（两测试稳定通过）
- [x] 更新 unimplemented 测试 + 新增 cancel/delete 已知/未知会话测试
- [x] cargo fmt --all --check ✓
- [x] cargo clippy --all-targets -- -D warnings ✓（0 警告）
- [x] cargo test --workspace ✓（mag-core 16 / mag-service 10 / sources 1 / tools 1）
- [x] cargo doc --no-deps --workspace ✓（无警告）
- [x] TODO.md 标 [DONE] + 完成记录
- [ ] 提交（待并发 review 复核后）

---

## 当前任务：C2-R Review 会话隔离与 cancel 正确性（TODO.md 649 行）

### 目标（纯 review 任务）
核对 per-session actor 模型：无死锁/饿死（cancel 不被 run 阻塞，§3.1）、多会话事件分流正确、
run 失败/取消后会话状态一致；汇总缺口；跑完整验证序列 1–5。

### Review 结论（代码核对，无源码改动）
- 无死锁/饿死：actor 循环 `select!` 收命令 + 归还 driver，run 在独立 `spawn_local` task 推进，
  `CancelRun` 只翻 `CancelToken`（不碰 agent `&mut`），永不饿死；`CancelToken::cancelled()` enable 后再查标志，
  无丢唤醒；single-thread + LocalSet 协作让点，stalling 流在 pending 处 park 让出执行器。
- 多会话分流：每会话独立线程/runtime/agent，命令 sender 同步登记无竞态；事件带 session_id 走广播 + 过滤订阅；
  `two_sessions_route_events_by_session_id` 证明不串话。
- 状态一致：`run_turn` 总 drop stream + emit 单一终止事件，再交还 driver → Idle；cancel/失败后会话续用；
  `cancel_mid_run_terminates_and_session_stays_usable` 证明；`delete_session` drop sender + join，在途 run abandon。
- 前向缺口（非阻塞）：RespondInteraction→C3；cancel 不移除 deferred SendMessage（符合规范）；
  next_run_id 跨会话可能重复（当前恒与 session_id 成对故安全）；snapshot→C4。无未排期失败测试，无需新前置任务。

### 验证（完整序列 1–5 全绿）
- fmt --check ✓；`cargo test -p mag-core engine::session`（2 用例，3 次稳定）✓；
  clippy -D warnings（0 警告）✓；`cargo test --workspace`（16/10/1/1）✓；`cargo doc --no-deps --workspace` ✓。

### 进度
- 已核对 session.rs / driver.rs / engine.rs / event_bus.rs / test_support.rs 与 DESIGN §3.1；结论如上。
- 已将 TODO.md C2-R 标 [DONE] 并补完成记录（含 review 对照与缺口汇总）。下一步：git 提交后停止。

---

## 当前任务：C3-1 mag-tools：ToolPlugin registry + 最小工具集（TODO.md 699 行）

### 目标
在 `mag-tools` crate 落地插件式工具体系（DESIGN §7）：
- `ToolPlugin` trait（declaration→agent-lib `Tool`；`invoke(ctx,args)->ToolResult`；`permission()->Option<PermissionSpec>`）。
- mag 侧 `ToolRegistry` 收集 plugin；产出 `declarations()` / `ToolSetRef`；`bind(ToolContextParts)` 得一个实现
  agent-lib `agent::ToolRegistry`（declarations/execute 按 name dispatch）的 `PluginToolRegistry`。
- 四内置工具：read_file / list_dir / grep（只读，permission=None/auto，路径受 worktree 约束）+
  shell（permission=Shell，cwd=worktree，cancel 可中断，risk 按命令）。

### 设计要点
- `ToolContext`（facade）带 worktree/cancel/tool_call_id；registry 持 `ToolContextParts` 逐调用建 ctx。
- `invoke` 返回 facade `ToolResult`（text/error/status）；execute 用公开 getter 转 `ToolResponse`
  （`into_response` 私有）。未知工具 → `ToolRuntimeError::UnknownTool`。
- 路径安全：`safe_join` 词法归一，禁止 `..` 逃逸 worktree / 绝对路径。
- grep：字面子串搜索（不引 regex），`spawn_blocking` 递归遍历（不跟 symlink），限量。
- shell：`sh -c`，piped stdout/stderr 并发 drain，`timeout(20ms)` 轮询 cancel（token 为 poll-based）→
  `start_kill` 中断。
- `PermissionSpec { category: ToolCategory, risk: ToolRisk }`，全 serde，risk 有序（Low<Medium<High）。
  静态 `permission()` 给基线；新增默认 `permission_for(args)` 供 shell 按命令细化 risk（honor “risk 按命令”）。

### 模块拆分
- `plugin.rs`：ToolPlugin / PermissionSpec / ToolCategory / ToolRisk。
- `registry.rs`：ToolRegistry（收集器）+ PluginToolRegistry（impl agent::ToolRegistry）。
- `path.rs`：safe_join + 错误。
- `tools/{read_file,list_dir,grep,shell}.rs`：四内置。
- `lib.rs`：re-export + crate 文档。

### 验证
1. `cargo fmt --all -- --check`
2. `cargo test -p mag-tools`（read/list/grep 结果、shell stdout、shell cancel 中断、declarations 四工具、
   execute 未知→UnknownTool、safe_join 逃逸拒绝、risk 分级）
3. `cargo clippy --all-targets -- -D warnings`
4. `cargo test --workspace`
5. `cargo doc --no-deps --workspace`

### 进度
- [x] Cargo.toml 依赖（tokio process/fs/io-util/time/rt/sync；dev: tempfile/uuid/tokio）
- [x] plugin.rs / path.rs / registry.rs / tools/{read_file,list_dir,grep,shell,mod}
- [x] lib.rs 接线（re-export ToolRegistry/PluginToolRegistry/ToolPlugin/PermissionSpec/工具/safe_join）
- [x] 单测（6 单测 + 13 集成测）全绿
- [x] 完整验证 1–5 全绿（fmt/clippy 0 警告、workspace 46 测、doc 无警告）
- [x] TODO.md 标 [DONE] + 完成记录
- [ ] git 提交后停止
