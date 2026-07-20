# 当前执行计划

## 范围
- 按 `TODO.md` 的顺序选择第一个标题未以 `[DONE]` 开头的任务。
- 本次只完成该一个任务；完成后更新记录、提交 Git，然后停止。
- `TODO.md` 是任务细节、依赖、验证要求和完成状态的唯一权威来源。
- 仅当阶段级计划、依赖结构或完成标准变化时更新 `PLAN.md`。

## 步骤
1. 读取 `TODO.md`，识别第一个未完成任务及其要求。
2. 查看最近提交信息；若最新提交明确提到与当前任务直接相关的未完成问题，将其纳入当前任务或在 `TODO.md` 中加入前置任务。
3. 只围绕当前任务读取相关代码、测试和文档，不做开放式历史问题扫查。
4. 以最小正确改动实现任务；若发现当前任务被具体规格缺口或缺陷阻塞，先修复该阻塞，或在 `TODO.md` 插入最小前置任务并停止。
5. 先运行格式化，再运行 lint，最后运行相关或完整测试；任何未排期失败测试都必须修复或排期，不能忽略。
6. 完成后更新 `TODO.md`：在任务标题前加 `[DONE]`，并写入完成记录和验证结果。
7. 检查 Git 状态、差异和最近提交，提交本次任务相关变更。
8. 提交后停止，不继续执行下一项任务。

## 进度记录
- 已写入本次调用的初始计划，下一步读取 `TODO.md` 识别首个未完成任务。
- 已识别首个未完成任务：`F-R [TODO] 全计划 review`。最近提交 `[M6-R] Review CLI milestone` 是该任务的直接前置，未发现需要先插入的新前置任务。
- F-R 执行计划：对照 `docs/CLI.md` 全节与决策 D1-D6 审查 M1-M6 当前实现；重点核对契约只加不改、配置 DTO↔DO/快照隔离、pivot/cancel 竞态、delegate restore、external ACP 生命周期、依赖边界、离线测试纪律与 rustdoc 完整性。若发现问题，直接修复并补测试；随后按默认验证序列执行，更新 `TODO.md` 完成记录，提交后停止。
- 已完成并行只读审查，返回的需核实问题包括：配置 apply 绑定快照与 system prompt 生效、持久化恢复钉住配置、interaction 错误应答/取消语义、CLI 多会话渲染状态、`/new [agent]`、delete in-flight external cleanup、快速 cancel 分类、bin 测试超时，以及若干契约/诊断细节。下一步逐项核实并修复确认为当前规格缺口的问题。
- 已修复并补测第一批确认缺口：CLI `/new [agent]`、prompt 去重、Question/Choice 取消、每 session 渲染状态；`IpcApproval` 无效应答保留 pending；secret 解析错误不回显裸值；`InteractionOrigin::is_root()` 严格判定；新增 service 能力默认返回 `Unsupported`；`apply_config` 捕获调用时快照并修 spawn 竞态；配置 system prompt 通过 mutable overlay 初始化/替换/清空；driver 快速 cancel 和无 `Done` 终态分类；会话关闭时 running driver 先 cancel 再 cleanup；bin subprocess 测试增加超时和 BrokenPipe 容忍。下一步运行 `cargo fmt` 和聚焦测试。
- 验证已通过：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`、`cargo doc --no-deps --workspace`。聚焦测试也已通过：`cargo test -p mag-cli`、`cargo test -p mag-config`、`cargo test -p mag-service`、`cargo test -p mag-core approval`、`cargo test -p mag-core config_apply`、`cargo test -p mag-core driver`、`cargo test -p mag --test cli`。下一步更新 `TODO.md` 的 F-R 完成记录。
- `TODO.md` 已将 `F-R` 标记为 `[DONE]` 并写入完成记录。下一步检查 git 状态/diff/log，确认变更后提交。
