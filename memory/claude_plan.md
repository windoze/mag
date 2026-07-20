# 执行计划

## 当前状态
- 已读取 `TODO.md`；本次执行的首个未完成任务是 `M4-2 mag-core：external ACP agent 委派（决策 D3，核心）`，现已在 `TODO.md` 标为 `[DONE]`。
- 已检查最近提交 `497adb4 feat(mag-core): local LLM subagent 委派 + Delegation* 事件映射（M4-1）`；提交正文记录的是 M4-1 的已知限制，未发现必须先插入到 `TODO.md` 的 M4-2 前置 blocker。
- 当前实现、测试、完成记录和验证均已完成；下一步是提交本次任务变更并停止。
- 本文件只记录可审计计划和进度，不记录私有推理链。

## 步骤
1. 读取 `TODO.md`，确定第一个未完成任务，并检查该任务的约束、依赖、验证要求和完成记录。
2. 必要时读取 `PLAN.md`、相关源码、测试和最近提交，范围仅限于理解当前任务和直接相关 blocker。
3. 按当前任务要求实现最小正确变更；若遇到必须先修复的具体前置问题，则更新 `TODO.md` 添加最小 prerequisite 并停止。
4. 运行格式化、lint 和相关测试；若发现未计划的测试失败，修复或在 `TODO.md` 中按策略添加前置任务。
5. 更新 `TODO.md`：将完成任务标题加上 `[DONE]` 并填写完成记录；仅当阶段计划变化时更新 `PLAN.md`。
6. 检查 git 状态和 diff，提交本次任务的所有相关变更，然后停止，不处理下一个任务。

## 进度记录
- 已创建初始执行计划。
- 已定位当前任务为 M4-2，并完成最近提交/工作区初检。
- 已完成相关上下文阅读：M4-1 local delegate 只覆盖 `agents.<name>`；`external_agents.<name>` 目前仅注册为 source 占位；agent-lib 已提供 `ManagedExternalAgent::acp`、`RegistryExternalSessionHandler`、`AcpAdapter` 和 cleanup API。
- 选定实现路径：mag-core 开启 `agent-lib/external-acp`；从配置解析 external delegate binding；driver 为每个 ACP external delegate 构造 `ManagedExternalAgent::acp(..)` 并注入 registry-backed handler，handler 使用 `AcpConfig` 承载配置 env；session actor 在删除/退出前显式 cleanup 已完成 external sessions。
- 已实现 M4-2 主体：external ACP delegate 进入 `ask_<name>` tool surface；source listing/probe 从当前配置投影外部 ACP source 并检查命令可用性；fake ACP 进程测试覆盖成功、崩溃失败映射、source availability、session delete cleanup。
- 聚焦验证：`cargo test -p mag-core delegation` 连续 3 次通过（9 passed）。
- 完整验证序列通过：`cargo fmt --all -- --check`、`cargo test -p mag-core delegation`、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`、`cargo doc --no-deps --workspace`。

## M4-2 执行步骤
1. 阅读现有 M4-1 delegation 装配、`mag-sources` ACP slot、`agent-lib` external ACP API、`mag-core` feature/依赖配置和相关测试夹具。
2. 最小化实现：开启 `agent-lib/external-acp` feature；从配置中的 external ACP agent 装配 `ManagedExternalAgent::acp(..)` + 默认 external session handler；把 external delegate 纳入 `ask_<name>` tool surface；保持 Engine 启动不因探测/启动失败而失败。
3. 接通 `list_sources()`/`probe_local_agents()` 对 external ACP slot 的可用性与 capabilities 反映；委派调用失败时经既有 delegation event 映射落为 `DelegationFailed`。
4. 补充本地 fake ACP 进程离线测试，覆盖成功委派、进程崩溃失败映射、source 可用性、会话清扫；真实外部联调若需要只加 `#[ignore]`。
5. 运行 `cargo fmt --all -- --check`、聚焦测试、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`、`cargo doc --no-deps --workspace`。
6. 更新 `TODO.md` 中 M4-2 标题为 `[DONE]` 并填写完成记录，提交本次任务所有相关变更后停止。
