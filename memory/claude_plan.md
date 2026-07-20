执行计划

1. 读取 `TODO.md`，按标题是否带有 `[DONE]` 判断第一个未完成任务，并核对该任务的验证要求与依赖。
2. 检查最近提交和当前工作区状态，只关注与当前任务直接相关或会阻塞当前任务的问题，不做开放式历史问题扫查。
3. 阅读当前任务涉及的代码、测试和文档，确定最小正确实现范围；如发现必须先修复的具体前置问题，则更新 `TODO.md` 并停止。
4. 按任务要求实现代码或文档变更，避免 workaround 和偏离规格的实现。
5. 按要求先运行格式化，再运行 lint，再运行相关测试和必要的完整测试；如出现未调度失败，修复或在 `TODO.md` 中加入最小前置任务。
6. 更新 `TODO.md`：在完成任务标题前加 `[DONE]`，补全完成记录；仅当阶段级计划变化时才更新 `PLAN.md`。
7. 检查变更，提交所有本次任务相关改动，提交后停止，不继续下一个任务。

进度记录

- 已创建本执行计划文件，下一步读取 `TODO.md` 确认第一个未完成任务。
- 已读取 `TODO.md`，首个未完成任务为 `M6-1 [TODO] mag-cli 骨架：双任务 REPL + 基本对话渲染`。
- 当前执行重点：新增只依赖 `mag-service` 与允许库的 `mag-cli` crate，提供 `Cli::run(Arc<dyn MagService>, opts)`，实现 stdin/rustyline 输入任务、service 事件渲染任务、基本会话创建、消息发送、`/new`、`/quit`，并补 scripted service 的管道 e2e。
- 下一步检查工作区状态与最近提交，只处理与 M6-1 直接相关的未完成事项。
- 已新增 `crates/mag-cli` crate、workspace 成员、`Cli::run` / `Cli::run_with_io`、基础 REPL 分派、事件渲染和 pipe e2e 测试；下一步运行格式化与聚焦测试，按编译反馈修正。
- 验证已通过：`cargo fmt --all -- --check`、`cargo test -p mag-cli`、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`、`cargo doc --no-deps --workspace`。
- 已将 `TODO.md` 中 M6-1 标记为 `[DONE]` 并补完成记录；下一步检查 diff 后提交。
- 复查时补强 TTY prompt 输出路径，使其与 pipe prompt 一样经共享 stdout 锁写出；已重新通过完整验证序列。
## 本轮执行计划

1. 读取 `TODO.md`，按标题是否带 `[DONE]` 判断第一个未完成任务，并核对相关依赖、验证要求和完成记录。
2. 检查最近提交和当前工作区状态，仅确认是否存在与当前任务直接相关的未完成事项或未提交改动。
3. 针对第一个未完成任务阅读必要代码与测试，明确最小正确实现范围，不做开放式历史问题扫描。
4. 实现该任务；如果发现阻塞当前任务的规格不匹配或未排期失败测试，则按要求在 `TODO.md` 添加最小前置任务并停止。
5. 运行格式化、lint 和相关测试；若代码变更需要完整验证，则在 lint 通过后运行完整测试套件。
6. 在验证通过后，将当前任务标题加上 `[DONE]`，更新完成记录；仅当阶段级计划变化时更新 `PLAN.md`。
7. 检查 diff 和 git 状态，提交本轮全部相关改动，然后停止，不推进下一项任务。

进度：已读取 `TODO.md`，本轮第一个未完成任务为 `M6-2 [TODO] PromptCoordinator：交互提示（审批 + Question/Choice）`。

## M6-2 具体执行步骤

1. 检查 git 状态与最近提交，只处理与 M6-2 直接相关的未完成事项。
2. 阅读 `crates/mag-cli` 现有双任务 REPL、测试与 `mag-service` 交互 wire 类型。
3. 在 `mag-cli` 内实现交互协调：`InteractionRequested` 入队、非流式输出时逐条提示、按类型读取用户输入并调用 `respond_interaction`。已完成：render task 将交互事件转交 coordinator，coordinator 持有单一队列与 active prompt。
4. 审批提示渲染可用 tool call 摘要、requirement reason 和 origin 前缀；Question 读取文本；Choice 渲染编号菜单并回传 index；pending 交互中的 Ctrl-C/EOF 映射为保守取消响应。已完成。
5. 增加 scripted service 管道 e2e，覆盖审批、Question、Choice、delegate origin 标注和多条交互顺序。已完成。
6. 运行规定验证序列，修复发现的问题。已通过：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test -p mag-cli`、`cargo test --workspace`、`cargo doc --no-deps --workspace`。
7. 将 M6-2 标记 `[DONE]` 并补完成记录；提交本轮相关改动后停止。已更新 `TODO.md`，下一步检查 diff 并提交。
