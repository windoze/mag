# 当前执行计划

## 范围
- 只处理 `TODO.md` 中第一个标题未以 `[DONE]` 开头的任务。
- 不做开放式历史问题排查；仅处理当前任务、阻塞当前任务的问题，以及验证中发现且未被明确排期的失败。
- 完成一个任务后停止，不继续推进后续任务。

## 步骤
1. 阅读 `TODO.md`，按文件顺序识别第一个未完成任务，确认其要求、依赖、验证方式和完成记录格式。
2. 如有需要，阅读 `PLAN.md`、最近提交和相关源码，限定在理解当前任务所需范围内。
3. 检查工作区状态，避免覆盖他人改动；如发现与当前任务直接冲突的未预期改动，则暂停并询问。
4. 实现当前任务要求；如果遇到必须先修复的具体前置问题，则把最小前置任务插入 `TODO.md`，提交后停止。
5. 按要求运行格式化、lint 和相关测试；若观察到未明确排期的测试失败，则修复或在 `TODO.md` 中排入必要前置任务。
6. 更新 `TODO.md`：在完成任务标题前加 `[DONE]`，并填写完成记录。仅在阶段计划真实变化时更新 `PLAN.md`。
7. 提交本次任务涉及的所有必要改动，提交信息包含任务编号或清晰描述。
8. 停止，等待下一次调用。

## 当前状态
- 已读取 `TODO.md` 并确认本轮任务为 `M4-R M4 review`，当前已在 `TODO.md` 标记为 `[DONE]`。
- 已读取 M4 相关设计、源码和最新提交；最近提交 `2d6b3fe [M4-3] Gate delegate starts behind approval` 与本 review 直接相关，但标题未标出未完成事项。
- 发现一个需修复的问题：`M4-R` 要求校验 feature gating（不开 feature 时编译过、external 配置报明确错误），但当前 `mag-core` 直接启用并导入 `agent-lib/external-acp`，没有可关闭 feature。
- 已实现修复：`mag-core` 新增默认开启的 `external-acp` feature；external ACP 运行时代码和 Unix fake ACP e2e 测试均按 feature 包裹；关闭 feature 时，包含 `[external_agents.*]` 的配置在 `Engine::from_config` 阶段返回 `EngineError::ExternalAgentUnsupported`。
- 已完成验证：`cargo fmt --all -- --check`、`cargo test -p mag-core delegation`、`cargo test -p mag-core --no-default-features from_config_rejects_external_acp_config_when_feature_is_disabled`、`cargo check -p mag-core --no-default-features --all-targets`、`cargo clippy -p mag-core --no-default-features --all-targets -- -D warnings`、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`、`cargo doc --no-deps --workspace` 均通过。
- 已更新 `TODO.md`：`M4-R` 标记为 `[DONE]` 并写入完成记录。
- 下一步：检查 git diff/status，提交本次 M4-R review 改动，然后停止。
