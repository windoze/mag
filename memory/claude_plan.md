# 执行计划

## 当前约束

- `TODO.md` 是任务排序、任务要求、依赖、验证和完成记录的唯一权威来源。
- 本次调用只完成第一个标题未带 `[DONE]` 的任务，完成后提交 Git 并停止。
- 在开始实现前先读取 `TODO.md`，必要时查看最新提交是否明确提到与当前任务直接相关的未完成问题。
- 不做开放式历史问题扫描；只有阻塞当前任务、使当前任务行为无效、或由当前任务引入的回归才纳入本次范围。
- 若发现未调度的测试失败，必须修复或在 `TODO.md` 中加入最小必要的前置任务，不能把当前任务标记为完成。
- 修改计划、完成关键步骤、遇到阻塞或完成任务时，更新本文件。

## 步骤

1. 读取 `TODO.md`，识别第一个标题未带 `[DONE]` 的任务。
2. 查看最新 Git 提交信息，判断是否存在与该任务直接相关的未完成事项。
3. 阅读与当前任务相关的源码、测试和文档，确定实现边界。
4. 如任务可直接完成，按仓库现有结构实现；如存在必须先修复的具体前置问题，更新 `TODO.md` 并停止。
5. 按要求运行格式化、lint 和测试：先 `cargo fmt`，再 `cargo clippy --all-targets -- -D warnings`，最后运行相关或完整测试套件。
6. 更新 `TODO.md`：在任务标题前加 `[DONE]`，填写完成记录；只有阶段级计划变化才更新 `PLAN.md`。
7. 提交所有本任务相关更改，提交信息包含任务编号和清晰描述。
8. 停止，不处理下一个任务。

## 进度记录

- 已创建初始执行计划；下一步读取 `TODO.md` 并定位第一个未完成任务。
- 已读取 `TODO.md`；第一个未完成任务是 `C0-3 Engine 空壳 + 内存事件总线 + id source`。
- 下一步检查最新提交信息是否含有与 `C0-3` 直接相关的未完成事项，然后阅读相关设计与源码。
- 最新提交为 `[C0-2] Implement protocol wire types`，没有直接提到与 `C0-3` 相关的未完成事项。
- 已核对 `DESIGN.md` §3/§4、`PLAN.md` R-E、`mag-protocol` 的 `Command`/`Event` 形状，以及 `agent-lib`
  的 `RequirementIds` / `ToolExecutionIds` / `FacadeIds` 参考实现。
- 实现方案：
  1. 在 workspace 依赖中加入 `tokio`、`tokio-stream`，供 `mag-core` 内存事件总线使用；`mag-core`
     额外使用 `uuid`。
  2. 新增 `event_bus` 模块：基于 `tokio::sync::broadcast`，提供 `EventBus::emit` 与
     `EventBus::subscribe() -> EventStream`，`EventStream` 实现 `Stream<Item = Event>`。
  3. 新增 `engine` 模块：`Engine` 持有空壳 `SessionManager` 和事件总线；`CreateSession` 建会话并 emit
     `SessionCreated`；`ListSessions` 通过 `CommandOutput::Sessions` 返回当前会话；会话级未实现命令 emit
     `RunError`。
  4. 新增 `ids` 模块：`MagIds` 使用共享单调计数器，从 1 起发放 UUID-backed agent-lib ID，实现
     `RequirementIds` 与 `ToolExecutionIds`，并提供 `continuing_after(high_water)` 续号入口。
  5. 添加聚焦单元测试覆盖 `CreateSession` 事件、`ListSessions`、多订阅者、会话级未实现错误事件和
     `MagIds` 唯一性/续号。
- 计划验证：`cargo fmt --all -- --check`，`cargo test -p mag-core engine::skeleton`，`cargo test -p mag-core ids`，
  `cargo clippy --all-targets -- -D warnings`，`cargo test --workspace`，`cargo doc --no-deps --workspace`。
- 已完成初版模块实现并运行 `cargo fmt --all`。
- 第一次聚焦测试编译发现 `mag_protocol::Command` 为 `#[non_exhaustive]`，`mag-core` 跨 crate match
  需要通配分支；已补 `unknown` unsupported 分支。
- 聚焦测试已通过且无 warning：`cargo test -p mag-core engine::skeleton`、`cargo test -p mag-core ids`。
- 下一步执行完整验证序列；若发现 warning、测试失败或 rustdoc 缺口，先修复再更新 `TODO.md`。
- 完整验证已通过：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、
  `cargo test --workspace`（30 分钟上限包装）、`cargo doc --no-deps --workspace`。
- 下一步更新 `TODO.md` 的 `C0-3` 标题和完成记录，然后检查 diff 并提交。
- `TODO.md` 已将 `C0-3` 标记为 `[DONE]` 并补完成记录；diff 检查无空白错误。
- 下一步提交 `[C0-3] Add engine skeleton and id source`，提交后停止。
