# 执行计划

本文件记录当前调用的执行计划与关键进展。为避免记录不可审计的私有推理，这里只写可执行步骤、约束和后续更新。

## 初始计划

1. 读取 `TODO.md`，按任务标题是否带 `[DONE]` 判断第一个未完成任务。
2. 检查最新提交信息是否明确提到与该任务直接相关的未完成问题；只处理会阻塞当前任务的问题。
3. 阅读当前任务涉及的代码、测试和文档，确认验收要求与依赖。
4. 按任务要求做最小且完整的实现；如遇到阻塞当前任务的规格缺口或失败测试，优先修复或在 `TODO.md` 中插入最小前置任务并停止。
5. 运行格式化、lint 和相关测试；若代码有变更，按要求先 `cargo fmt`，再 `cargo clippy --all-targets -- -D warnings`，最后运行必要测试或完整测试套件。
6. 更新 `TODO.md`：完成时给任务标题加 `[DONE]` 并填写完成记录；仅当阶段计划变化时才更新 `PLAN.md`。
7. 检查 git 状态和 diff，提交本次任务的所有相关变更，然后停止，不继续下一个任务。

## 进展记录

- 已写入初始执行计划，下一步读取 `TODO.md`。
- 已读取 `TODO.md`，首个未完成任务为 `M6-3 [TODO] pivot/cancel/会话命令`。
- 已检查最新提交：`7e29bf1 [M6-1/M6-2] Add mag-cli REPL and prompt coordinator`，未发现提交信息中明确标注的未完成阻塞项。
- 下一步检查 `crates/mag-cli` 当前实现和测试，补齐 pivot、cancel 与会话/source slash 命令，并扩展 e2e 覆盖。
- 已完成初版代码改动：`mag-cli` 现在按 session 跟踪活动 run，普通文本在当前 run 进行中时先调 `pivot_message`，`NotPivotable` 自动回落 `send_message`；Ctrl-C 和 `/cancel` 调 `cancel`；新增 `/sessions`、`/resume`、`/delete`、`/sources` 命令；渲染 `PivotQueued/Applied/Dropped` 状态。
- 已扩展 `mag-cli` e2e scripted service 和测试，覆盖 pivot 成功、NotPivotable 回落、Ctrl-C 取消、slash 命令服务调用。下一步运行格式化与测试。
- `cargo test -p mag-cli` 首次编译发现 `session_id` 同时可变/不可变借用，已通过提前计算 `current_session_running` 修复。下一步重跑格式化与聚焦测试。
- 聚焦测试通过后，`cargo clippy --all-targets -- -D warnings` 发现两个 `collapsible_if`，已按 lint 建议改为链式判断。下一步重跑格式化、聚焦测试和 clippy。
- 随后 `not_pivotable_falls_back_to_send_message` 暴露同一 session 的旧 terminal 事件可在回落启动新 run 后清掉活动标记，导致 `/quit` 提前退出。已在 `NotPivotable` 回落路径增加“跳过下一条旧 terminal”计数，防止新 run 输出被丢弃。
- 验证已通过：`cargo fmt --all -- --check`、`cargo test -p mag-cli`（7 passed）、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`、`cargo doc --no-deps --workspace`、`cargo tree -p mag-cli -e normal --depth 1`。
- 已将 `TODO.md` 中 `M6-3` 标记为 `[DONE]` 并补完成记录。下一步检查 git diff/status 后提交。
