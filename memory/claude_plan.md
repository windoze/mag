# 当前执行计划

说明：此文件记录可共享的执行计划、关键决策和进度更新；不记录私有推理链。

## 初始计划

1. 阅读 `TODO.md`，按规则识别第一个标题未带 `[DONE]` 的任务，并确认其要求、依赖、验证方式和完成记录格式。
2. 如有必要，查看 `PLAN.md`、最近提交和与当前任务直接相关的代码区域；不做开放式历史问题扫查。
3. 针对第一个未完成任务做最小正确实现；若发现阻塞当前任务的规格不匹配或缺失前置条件，则按要求在 `TODO.md` 插入最小前置任务并停止。
4. 运行必要验证，顺序为 `cargo fmt`、`cargo clippy --all-targets -- -D warnings`、相关测试和需要时的完整测试套件；若仅文档变更且有可复用绿色结果，则记录跳过原因。
5. 更新 `TODO.md`：只有任务实际完成并验证通过后，才在任务标题前加 `[DONE]` 并补全完成记录；仅当阶段计划变化时才更新 `PLAN.md`。
6. 提交所有与本次任务相关的改动，提交信息包含任务编号和简明说明，然后停止，不处理下一个任务。

## 进度

- 2026-07-20：初始化执行计划，下一步读取 `TODO.md` 确认当前任务。
- 2026-07-20：已确认首个未完成任务为 `M5-1 [TODO] mag-tools：ask_user ToolPlugin`。执行重点：阅读 `docs/CLI.md` §5 P6 / D6、`mag-tools` 插件接口、`mag-core` 现有交互/approval 注入路径；实现 `ask_user` 工具并注册；补 fake LLM 离线测试覆盖 Question/Choice 回灌与 cancel；完成后运行格式化、聚焦测试、clippy、workspace 测试和文档构建，再更新 `TODO.md`、提交并停止。
- 2026-07-20：实现已完成到验证前状态：`mag-tools` 增加 `UserInteractionBridge` / `ToolInvocation`、`AskUserTool` 与注册表桥绑定；内置工具集加入 `ask_user`；`mag-core` 在 facade tool 投影处为每个会话注入基于 `IpcApproval` 的用户交互桥；新增 `mag-tools` 单/集成测试与 `mag-core` 端到端测试覆盖 Question、Choice 和 cancel。下一步运行 `cargo fmt --all -- --check`。
- 2026-07-20：格式化通过；聚焦测试通过：`cargo test -p mag-tools`（23 passed）和 `cargo test -p mag-core ask_user`（3 passed）。下一步运行 clippy、workspace 测试和文档构建。
- 2026-07-20：完整验证通过：`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`、`cargo doc --no-deps --workspace` 均成功。下一步更新 `TODO.md` 的 M5-1 完成记录并提交。
