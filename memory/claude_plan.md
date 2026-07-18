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
