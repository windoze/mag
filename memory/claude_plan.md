本轮执行计划

约束说明
- 按用户要求，TODO.md 是唯一任务顺序与完成状态来源。
- 本文件记录可公开的执行计划、关键进度和变更，不记录不可公开的内部推理链。
- 本轮只完成 TODO.md 中第一个标题未带 [DONE] 的任务，然后停止。

步骤
1. 读取 TODO.md，识别第一个未完成任务；只在需要判断阶段级影响时读取 PLAN.md。
2. 检查最新提交是否明确提到与该任务直接相关的未完成事项。
3. 阅读当前任务相关代码、测试和文档，确认任务要求、依赖和验证方式。
4. 实现该任务；如果发现阻塞当前任务的具体前置问题，则在 TODO.md 中插入最小必要前置任务并停止。
5. 运行格式化、lint 和相关测试；如有未调度失败，修复或在 TODO.md 中加入必要前置任务。
6. 更新 TODO.md：完成时在任务标题前加 [DONE]，并填写 completion record；仅当阶段计划实际变化时更新 PLAN.md。
7. 检查 git 状态和 diff，提交本轮所有相关变更。
8. 停止，不进入下一项任务。

进度记录
- 已创建本轮计划文件，下一步读取 TODO.md。
- 已读取 TODO.md，首个未完成任务为 M4-3：委派审批 + restore 重注册 delegate。
- 最新提交为 M4-2，内容与 M4-3 相邻但未改变任务顺序；本轮聚焦 M4-3，不做无关历史扫描。
- M4-3 验证目标：覆盖委派默认审批、deny 路径、approve 后执行、resume 后 delegate/tool surface/审批策略仍生效。
- 已修改 mag-core driver 审批策略：所有 local/external delegate 的 `ask_<name>` start tool 默认走审批，且 `[tools.ask_<name>].approval` 可显式覆盖。
- 已补 M4-3 专项测试：local deny、local approve、resume 后 local delegate 审批恢复、external ACP start approve；旧 M4-1/M4-2 生命周期测试改用显式 allow 保持测试焦点。
- 已完成验证：`cargo fmt --all`、`cargo test -p mag-core delegation`、`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`、`cargo doc --no-deps --workspace` 全部通过。
- 已更新 TODO.md：M4-3 标题改为 [DONE]，并填写完成记录。下一步检查 diff 并提交本轮变更。
