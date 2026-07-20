# M6-4 执行计划

当前任务：`M6-4 [TODO] /config 命令`。

## 目标

- 在 `mag-cli` 中实现 `/config show`、`/config reload`、`/config apply`。
- 渲染 `ConfigChanged{revision}` 事件。
- 补充 e2e 覆盖三个子命令及配置变更事件。
- 完成格式化、聚焦测试、clippy、workspace 测试、doc 验证。
- 将 `TODO.md` 的 `M6-4` 标记为 `[DONE]` 并填写完成记录。
- 提交本任务全部改动后停止。

## 执行步骤

1. 检查最近提交是否有与 `M6-4` 直接相关的未完成事项。
2. 阅读 `mag-cli` 的命令分发、事件渲染与 e2e 测试代码，确认现有 slash 命令和 scripted service 测试结构。
3. 如 `mag-cli` 已可通过 `mag-service` re-export 使用 `ConfigDto`，直接在 CLI 层序列化 TOML；若需要额外依赖，优先复用 `mag-service` 公开 API，保持 `mag-cli` 不直接依赖 `mag-config`。
4. 实现 `/config` 命令分发：`show` 调 `get_config()` 并输出 TOML；`reload` 调 `reload_config()` 并打印结果；`apply` 调 `apply_config()` 并打印下一 turn 边界生效提示；未知子命令输出错误和帮助。
5. 在事件渲染中处理 `ConfigChanged{revision}`，输出一行配置 revision 变更提示。
6. 扩展 `mag-cli` e2e scripted service，记录配置方法调用并模拟 `ConfigChanged` 事件；新增覆盖三个子命令和事件渲染的测试。
7. 运行 `cargo fmt --all -- --check`；如有格式差异，运行 `cargo fmt --all` 后复检。
8. 运行聚焦测试 `cargo test -p mag-cli`。
9. 运行 `cargo clippy --all-targets -- -D warnings`。
10. 运行 `cargo test --workspace`。
11. 运行 `cargo doc --no-deps --workspace`。
12. 更新 `TODO.md`：将 `M6-4` 标题改为 `[DONE]`，填写实现、测试和门禁完成记录。
13. 检查 git 状态、diff 与最近提交，提交全部本任务改动。

## 进度记录

- 已读取 `TODO.md` 并确认首个未完成任务为 `M6-4`。
- 已检查最近提交 `5fa8c82 [M6-3] Implement CLI pivot cancel session commands`，提交信息未提示与 `M6-4` 直接相关的未完成缺口。
- 已确认 `mag-cli` 可通过 `mag-service::ConfigDto` 使用 `to_string_pretty()`，无需新增 `mag-config` 直接依赖。
- 已实现 `/config show|reload|apply`、`ConfigChanged{revision}` 渲染，并新增配置命令 e2e 测试。
- `cargo fmt --all -- --check` 初次发现 rustfmt 差异；已运行 `cargo fmt --all` 并复检通过。
- `cargo test -p mag-cli` 通过（8 passed）。
- `cargo clippy --all-targets -- -D warnings` 通过。
- `cargo test --workspace` 通过（全绿，1 ignored 为既有联调测试）。
- `cargo doc --no-deps --workspace` 通过（0 warning）。
- 已将 `TODO.md` 的 `M6-4` 标记为 `[DONE]` 并填写完成记录。后续只做 diff/依赖边界检查与提交。
- 已检查 `cargo tree -p mag-cli -e normal --depth 1`：直接依赖仍为 `futures`、`mag-service`、`rustyline`、`tokio`。
- 已检查 git diff/status/log 与 `git diff --check`，准备提交。
