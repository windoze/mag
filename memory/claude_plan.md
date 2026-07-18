# 执行计划

## 约束说明

- 本文件用于记录可审计的执行计划、关键判断依据和进度更新。
- 不记录逐字内部思考过程；改为记录足以复核的推理摘要、决策依据和操作步骤。
- 输出与进度记录使用中文。

## 当前状态

- 已读取 `TODO.md`，第一个未完成任务是 `C1-1 fake LlmClient 测试夹具 + StreamingTapHandler`。
- 已检查最新提交：`8dc99ca [C0-R] Review skeleton and protocol consistency`，未明确提到与 C1-1 直接相关的未完成问题。
- 当前工作区未提交变更仅包含本计划文件。
- 本次只完成 C1-1；完成后更新 `TODO.md`、执行验证、提交 Git，然后停止。

## 初始执行步骤

1. 已根据 C1-1 读取 `agent-lib` 中 `LlmClient`、`LlmHandler`、`Accumulator`、`StreamEvent`、`Delta`、facade `StreamingTapHandler` 以及相关 fake client 测试样板。
2. 已读取 `mag-core` 当前模块结构、事件总线和协议事件类型，确定将实现放在 `mag-core::llm`，并公开导出 `StreamingTapHandler`。
3. 已在 `mag-core` 实现 `StreamingTapHandler`：包装 `Arc<dyn LlmClient>`，在流式 fold 中逐个文本 delta emit `Event::TextDelta`，并用 agent-lib accumulator 折叠成 `Response`。
4. 已实现测试用 `FakeLlmClient`，支持脚本化纯文本流与带 tool-use 的响应。
5. 已添加 C1-1 单元测试：断言 `TextDelta` 序列与脚本一致，折叠出的 `Response` 文本完整且可交回 agent-lib。
6. 已运行 `cargo fmt --all` 与 `cargo fmt --all -- --check`，格式检查通过。
7. 已运行聚焦测试 `cargo test -p mag-core llm::stream`，3 个 C1-1 测试通过。
8. 已按要求运行 `cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`（1800 秒超时包装）、
   `cargo doc --no-deps --workspace`，均通过。
9. 已在 `TODO.md` 将 C1-1 标为 `[DONE]` 并补完成记录。
10. 下一步检查工作区变更，提交所有本次任务相关文件，然后停止。
