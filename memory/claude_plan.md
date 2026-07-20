# 执行计划

本文件记录本次调用的可公开执行计划与进度。不会记录私有推理过程。

## 初始计划

1. 读取 `TODO.md`，按文档顺序识别第一个标题未以 `[DONE]` 开头的任务。
2. 检查该任务的要求、依赖、验证方式和完成记录；必要时查看最新提交是否明确提到与该任务直接相关的未完成问题。
3. 在不做开放式历史问题清扫的前提下，收集完成当前任务所需的最小代码上下文。
4. 实现当前任务；若遇到阻塞当前任务的规格不匹配或缺失前置条件，则将最小必要前置任务插入 `TODO.md`，提交后停止。
5. 按要求运行格式化、lint 和相关测试；若观察到未被排期的失败测试，修复或在 `TODO.md` 中排期到当前任务完成之前。
6. 任务完成后，在 `TODO.md` 中将任务标题加上 `[DONE]` 并更新完成记录；仅当阶段计划实际变化时才更新 `PLAN.md`。
7. 检查 git 状态和差异，提交本次任务相关全部变更，然后停止，不继续下一个任务。

## 进度

- 已创建初始执行计划，下一步读取 `TODO.md` 识别当前任务。
- 已读取 `TODO.md`，本次执行目标确定为首个未完成任务：`W1-3 [TODO] SessionInfo 增强（P4）`。
- 下一步只检查与 `SessionInfo`、`list_sessions`、运行/交互状态直接相关的实现与最新提交信息，避免开放式历史问题清扫。
- 已确认最新提交 `[W1-2] Add session history retrieval` 未直接声明 W1-3 的未完成问题。
- 实施方案：在 `mag-service` 为 `SessionInfo` 向后兼容新增 `title: Option<String>`、`last_active_at: Option<u64>`、`status: SessionStatusWire`；在 `mag-core` 的 `list_sessions` 中以持久化列表为基底，叠加会话历史标题/活跃时间与 live session actor 状态快照。
- 测试方案：补 `mag-service` 旧 JSON 兼容与新字段 roundtrip；补 `mag-core` running、awaiting interaction、历史标题/时间的聚焦断言，再运行任务要求和默认验证序列。
- 已完成代码实现与新增聚焦测试，并已运行 `cargo fmt --all`。
- 下一步运行 `cargo test -p mag-service` 与 mag-core 聚焦测试；若通过，再运行 clippy、workspace test、doc。
- 验证已通过：`cargo test -p mag-service`、三个 mag-core 聚焦测试、`cargo clippy --all-targets -- -D warnings`、`cargo fmt --all -- --check`、`cargo test --workspace`、`cargo doc --no-deps --workspace`。
- 已将 `TODO.md` 中 `W1-3` 标记为 `[DONE]` 并补完成记录；下一步检查 git 差异并提交本次任务变更。
- 已完成提交前检查，提交 W1-3 任务变更，并确认工作区无未提交变更。
