# 执行计划

## 本次调用目标

目标：只完成 `TODO.md` 中第一个未完成任务，完成记录、验证和提交后停止。

## 步骤计划

1. 优先读取 `TODO.md`，识别第一个标题未带 `[DONE]` 前缀的任务。
2. 检查最近提交是否明确留下与该任务直接相关的未完成问题。
3. 阅读任务正文、依赖、验证要求和相关代码路径。
4. 按任务原要求实现，不缩小范围，不引入 workaround。
5. 按要求运行格式化、lint 和测试：先 `cargo fmt`，再 `cargo clippy --all-targets -- -D warnings`，之后运行相关/完整测试。
6. 若发现未排期的失败测试或阻塞当前任务的规格不匹配，优先修复；若无法在当前任务内修复，则在 `TODO.md` 插入最小前置任务并停止。
7. 任务完成且验证通过后，在 `TODO.md` 中将任务标题前缀改为 `[DONE]` 并补完成记录。
8. 仅当阶段级顺序、依赖、假设或完成条件变化时才更新 `PLAN.md`。
9. 检查 git status/diff/log，提交本任务相关变更。
10. 停止，不开始下一个任务。

## 进度记录

- 已在读取项目任务文件或运行命令前初始化本计划文件。
- 已识别首个未完成任务：`W1-4 ts-rs 生成管线 + @mag/protocol`。
- 最近提交 `7677250 [W1-3] Record completion progress` 未明确留下 W1-4 的直接前置问题。
- 当前范围：检查现有 wire DTO 与构建结构，增加 feature-gated `ts-rs` 导出支持，创建 `@mag/protocol`，完成验证，更新 `TODO.md` 并提交。
- 已完成初始实现：新增默认关闭的 `ts-export` Cargo feature、为 service/config DTO 添加 `TS` 派生、添加 `export_ts` 导出测试、创建 protocol 包骨架、补 fixture 检查和 README 再生成说明。
- `cargo fetch` 已取得 `ts-rs v12.0.1`，无需启用降级方案。
- `cargo test -p mag-service --features ts-export export_ts` 已通过并生成 protocol 绑定。
- Rust JSON fixture 测试与 TypeScript 编译期 fixture 校验均已通过。
- 必要验证已全部通过：格式检查、聚焦测试、默认 clippy、`ts-export` clippy、完整 workspace 测试、workspace 文档构建、TypeScript 编译检查。
- `TODO.md` 已将 `W1-4` 标记为 `[DONE]` 并补完成记录；`PLAN.md` 未变，因为阶段级计划未变化。
- 下一步：重新暂存更新后的计划文件，提交 W1-4 变更并停止。
