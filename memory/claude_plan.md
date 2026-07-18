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
