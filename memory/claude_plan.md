# Claude Execution Plan

本文件记录本次调用的可审计执行计划、关键决策和进度更新。不会记录私有逐步思维链，但会记录足够的事实依据、约束和执行步骤，便于检查进展。

## 当前约束

- `TODO.md` 是任务顺序、完成状态和验收要求的唯一权威来源。
- 只完成第一个标题未带 `[DONE]` 的任务，然后停止。
- 若发现阻塞当前任务的缺陷、规格不匹配或未排期失败测试，必须修复或在 `TODO.md` 中加入最小必要前置任务后提交并停止。
- 完成任务后必须更新 `TODO.md`，运行必要验证，并提交 Git commit。
- `PLAN.md` 只在阶段级计划、依赖或完成标准变化时更新。

## 初始执行计划

1. 读取 `TODO.md`，按文件顺序定位第一个标题未带 `[DONE]` 的任务。
2. 检查该任务的正文、依赖、验证要求和完成记录，确认是否需要查看最近提交中与该任务直接相关的未完成事项。
3. 基于任务内容只读取相关代码、测试和文档，避免无关历史问题扫查。
4. 实现当前任务要求；若遇到阻塞当前任务的规格缺口或失败测试，优先修复，或在 `TODO.md` 插入最小必要前置任务并停止。
5. 按要求运行格式化、lint 和相关测试；如代码有实质变更，按顺序执行 `cargo fmt`、`cargo clippy --all-targets -- -D warnings`，再执行必要测试或完整测试套件。
6. 更新 `TODO.md`：在已完成任务标题前加 `[DONE]`，补充完成记录和验证结果。仅在阶段级计划变化时更新 `PLAN.md`。
7. 检查 Git 状态和差异，确认只提交本次任务相关文件；若是恢复失败调用并完成任务，则包含当前未提交文件。
8. 创建描述清晰的提交，然后停止，不继续下一个任务。

## 进度日志

- 已创建本执行计划文件，下一步读取 `TODO.md` 确认第一个未完成任务。
- 已读取 `TODO.md`，第一个标题未带 `[DONE]` 的任务是 `W3-4 app-web 壳：对话闭环`。
- 当前任务执行重点：检查最近提交是否有直接相关未完成事项；阅读 `@mag/app-web`、`@mag/client`、`@mag/ui` 的现状；实现 app 壳装配、token fragment/sessionStorage、SessionStore 单例、会话列表、新建/恢复/删除、流式 thread、审批提交、pivot 409 回落与 cancel；补壳级 vitest mock transport 测试；运行任务要求验证；更新 `TODO.md` 完成记录并提交。
- 已实现 app-web 壳容器：token fragment 捕获、SessionStore 装配、会话列表/新建/恢复/删除、thread/composer 联通、审批响应、pivot 409 回落、cancel、Sources/Config 占位路由与 config_changed notice。
- 已补 `SessionStore::deleteSession`，并修复 list session 同步时新会话重复进入 `sessionOrder` 的问题。
- 已替换 app-web vitest：mock transport 覆盖 token 捕获、会话恢复/历史渲染、工具/审批卡、审批提交、pivot fallback、cancel、新建/删除和占位路由；`@mag/app-web` test/build 已单独通过。
- 已在 `ui/README.md` 追加 W2 server 手动 web smoke 流程。下一步执行格式化、lint、前端 workspace 验证与 Rust 默认验证序列。
- 已完成验证：Rust fmt/clippy/test/doc 与前端 format/lint/test/build 均通过。已将 `TODO.md` 中 `W3-4` 标记为 `[DONE]` 并写入完成记录。下一步检查 git diff/status 并提交。
- 提交前将 token fragment/sessionStorage 逻辑拆入 `ui/apps/web/src/token.ts`，重新运行 `pnpm lint`、`@mag/app-web test`、`@mag/app-web build` 均通过；Rust 代码未再变化。
