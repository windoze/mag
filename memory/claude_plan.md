# 当前执行计划

## 范围
- 按 `TODO.md` 的顺序选择第一个标题未以 `[DONE]` 开头的任务。
- 本次只完成该一个任务；完成后更新记录、提交 Git，然后停止。
- `TODO.md` 是任务细节和完成状态的唯一权威来源；仅当阶段级计划发生变化时更新 `PLAN.md`。

## 步骤
1. 读取 `TODO.md`，识别第一个未完成任务及其验证要求、依赖和完成记录格式。
2. 查看最近提交信息，若其明确提到与当前任务直接相关的未完成问题，将其纳入当前任务或作为前置项记录到 `TODO.md`。
3. 根据当前任务读取相关代码、测试和文档，避免无关历史问题扫查。
4. 以最小正确改动实现任务；如果发现当前任务被具体缺陷或规格不匹配阻塞，优先修复该问题，或在 `TODO.md` 中插入最小前置任务并停止。
5. 按要求运行格式化、lint 和相关测试；若观察到未排期的失败测试，修复或在 `TODO.md` 中排期，且不将当前任务标记为完成。
6. 完成后更新 `TODO.md`：在任务标题前加 `[DONE]`，并填写完成记录、验证结果和关键改动。
7. 检查 Git 状态和差异，提交所有与本次任务相关的更改；如是恢复前一次未完成任务，则按要求纳入当前未提交文件。
8. 提交后停止，不继续处理下一项任务。

## 进度记录
- 已写入初始执行计划，下一步读取 `TODO.md` 识别首个未完成任务。
- 已识别首个未完成任务：`M6-R [TODO] M6 review`。最近提交 `[M6-5] Wire mag CLI binary e2e` 是该任务直接前置，不需要新增前置任务。
- M6-R 执行计划：读取 `docs/CLI.md` §0 与 M6 相关实现/测试；逐项核对流式对话、工具权限、通用交互、多 agent 编排（含 external ACP）、协作、持久化恢复、cancel、pivot、配置动态生效；检查 `mag-cli` 依赖边界；如发现缺口，直接修复并补测试；随后按默认验证序列执行并更新 `TODO.md` 完成记录，最后提交。
- review 已发现并修复第一批缺口：后台会话 interaction 不再丢失、`Delegation*`/tool 事件有 CLI 摘要渲染、Permission 交互纳入 scripted e2e。已运行 `cargo fmt --all` 与 `cargo test -p mag-cli`，10 个 mag-cli 测试通过。
- 已补强真实 Engine+CLI e2e：`/config apply` 后下一轮使用新 model、跨 Engine 持久化恢复后上下文包含重启前历史、pivot 文本进入后续 LLM request、local/external delegation 生命周期在 CLI 输出可见。已运行 `cargo test -p mag --test engine_cli`，4 个测试通过。
- 依赖边界检查：`cargo tree -p mag-cli -e normal --depth 1` 显示 direct normal 依赖仅为 futures/mag-service/rustyline/tokio；完整 tree 无 mag-core/agent-lib，mag-config 仅经 mag-service 契约传递出现。
- 默认验证序列已通过：`cargo fmt --all -- --check`、`cargo test -p mag-cli`、`cargo test -p mag --test engine_cli`、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`、`cargo doc --no-deps --workspace`。
- `TODO.md` 已将 `M6-R` 标记为 `[DONE]`，并记录 review 发现/修复、§0 验证清单结论、依赖边界与门禁结果。下一步检查 git 状态/diff 后提交。
