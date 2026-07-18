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
