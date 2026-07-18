# 执行计划

## 当前约束

- 输出使用中文。
- `TODO.md` 是任务顺序和完成状态的权威来源。
- 本轮只完成第一个标题未带 `[DONE]` 的任务，然后停止。
- 完成后需要更新 `TODO.md` 的任务标题和完成记录，并提交 Git commit。
- 若发现阻塞当前任务的规格不匹配、缺失前置条件或未安排的测试失败，需要先修复，或把最小前置任务插入 `TODO.md` 后提交并停止。
- 不把例行进展写入 `PLAN.md`，除非阶段级计划、依赖或完成标准发生变化。

## 步骤计划

1. 读取 `TODO.md`，按文件顺序找出第一个标题未带 `[DONE]` 的任务。
2. 检查最新提交信息是否明确提到与该任务直接相关的未完成问题。
3. 读取当前任务涉及的代码、测试和文档，只做与当前任务相关的调查。
4. 按任务要求实现完整变更；如遇必须先解决的阻塞问题，更新 `TODO.md` 记录前置任务并停止。
5. 在关键实现步骤完成后更新本文件，记录实际进展和计划调整。
6. 运行验证：先 `cargo fmt`，再 `cargo clippy --all-targets -- -D warnings`，最后运行任务要求的测试或完整测试套件。
7. 若有未安排的测试失败，修复或在 `TODO.md` 中插入最小前置任务，不把当前任务标为完成。
8. 成功后更新 `TODO.md`：在任务标题前加 `[DONE]`，补充完成记录和验证结果。
9. 检查 Git diff，提交本轮所有相关变更。
10. 停止，不处理下一个任务。

## 进展记录

- 已创建本执行计划文件，下一步读取 `TODO.md` 确认本轮任务。
- 已读取 `TODO.md`，首个未完成任务为 `C0-R Review：骨架 + 协议一致性`。
- 最新提交为 `[C0-3] Add engine skeleton and id source`，与当前 review 相关，但提交信息未指出需要先处理的未完成 issue。
- 当前执行重点：核对 `DESIGN.md` §2/§4 与 workspace/protocol 实现一致性，形成协议对照表，运行完整验证序列，通过后更新 `TODO.md` 并提交。
- 协议对照发现 `DESIGN.md` §4.2 中的 `DelegationMessage` 事件尚未实现；这是当前 review 的直接一致性缺口，将在本轮补齐并增加 serde round-trip 覆盖。
- 已补齐 `Event::DelegationMessage` 与 `DelegationMessageWire`，并加入事件 round-trip/tag 测试。
- 已运行 `cargo fmt --all` 与 `cargo test -p mag-protocol`，当前均通过；`cargo tree -p mag-protocol` 确认无 `agent-lib` 依赖。
- 完整验证已通过：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`（30 分钟上限包装）、`cargo doc --no-deps --workspace`。
- 已将 `TODO.md` 中 `C0-R Review：骨架 + 协议一致性` 标记为 `[DONE]`，并补充完成记录、协议对照表和验证结果。
- 下一步：复查 git diff/status，提交本轮变更后停止。
