# 执行计划

## 目标

完成 `TODO.md` 中按顺序出现的第一个未完成任务，验证后更新任务记录并提交一次 Git commit，然后停止。

## 步骤

1. 读取 `TODO.md`，按标题是否带有 `[DONE]` 判断第一个未完成任务。
2. 检查该任务的正文、依赖、验证要求和完成记录；必要时查看最新提交是否明确提到与该任务直接相关的未完成问题。
3. 根据任务内容只收集必要代码上下文，避免开放式历史问题排查。
4. 实现该任务；如果发现阻塞当前任务的具体前置问题，则将最小必要前置任务插入 `TODO.md`，提交后停止。
5. 运行格式化、lint 和相关测试；如有未排期失败，修复或把最小必要任务排到当前任务之前。
6. 将已完成任务标题加上 `[DONE]`，更新其完成记录；仅在阶段级计划改变时更新 `PLAN.md`。
7. 检查 Git 状态和差异，提交本次任务涉及的全部变更。
8. 停止，不继续下一个任务。

## 当前状态

- 已写入初始执行计划。
- 已读取 `TODO.md`，首个未完成任务为 `M5-R [TODO] M5 review`。
- 本轮只处理 M5 review：对照 `docs/CLI.md` §5 P6 检查 `ask_user` 的交互桥复用一致性、cancel 语义、tool profile 开关与 rustdoc；发现问题则直接修复并补测试。
- 已检查最新提交 `[M5-1] Implement ask_user ToolPlugin`，提交说明未列未完成事项。
- Review 发现：`ask_user` 的桥调用被包进 `tokio::spawn`；取消分支返回时只丢弃 `JoinHandle`，不会取消桥 future。mag-core 的桥会自行观察 cancel，但插件契约层不应依赖桥实现主动退出。
- 修复计划：改为在 `tokio::select!` 中直接等待 `bridge.ask_user(ctx, request)`，取消时直接 drop 桥 future；新增 mag-tools 运行中取消测试验证桥 future 被 drop；同时补强 mag-core ask_user cancel 测试，断言取消后迟到响应不再命中 pending 交互。
- 已完成修复：`AskUserTool` 直接 select 桥 future；`IpcApproval::emit_and_await` 增加 pending cleanup guard，确保桥 future 被 drop 时 request id 从 pending map 移除。
- 已通过验证：`cargo fmt --all -- --check`、`cargo test -p mag-tools ask_user`（5 passed）、`cargo test -p mag-core ask_user`（3 passed）。
- 已通过完整门禁：`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`（全绿，1 ignored 为既有 zed 联调测试）、`cargo doc --no-deps --workspace`（0 warning）。
- 已更新 `TODO.md`：`M5-R` 标题标记为 `[DONE]`，完成记录列出 review 结论、修复项与验证结果；`PLAN.md` 未改，因为阶段级计划未变化。
- 下一步：提交本任务变更后停止。
