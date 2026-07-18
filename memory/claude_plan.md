# 执行计划

## 当前约束
- 输出与执行记录使用中文。
- `TODO.md` 是任务顺序、要求、验证和完成记录的权威来源。
- 本轮只处理第一个标题未带 `[DONE]` 的任务，完成后提交并停止。
- 若发现阻塞当前任务的缺陷或未排期失败测试，先修复；无法直接修复时在 `TODO.md` 中插入最小必要前置任务并提交后停止。
- 不以缩小范围、绕过实现、降低规格或任务私有特判作为完成方式。
- 代码变更后按要求先运行 `cargo fmt`，再运行 `cargo clippy --all-targets -- -D warnings`，最后运行必要测试；完整 Rust 测试套件超时不超过 30 分钟。

## 初始执行思路
1. 读取 `TODO.md`，按标题是否带 `[DONE]` 判断第一个未完成任务。
2. 检查最近提交信息，只有当其明确提到与当前任务直接相关的未完成事项时，才将其纳入当前任务或作为前置任务记录。
3. 阅读当前任务涉及的代码、测试和文档，确认实现边界与验证要求。
4. 如任务可直接完成，按现有代码风格做最小但完整的实现，并补充或调整针对性测试。
5. 如遇到阻塞当前任务的规格缺口、回归、失败测试或实现边界，优先处理；若无法在本轮完成，则更新 `TODO.md` 加入最小前置任务并停止。
6. 执行格式化、lint 和相关测试；若观察到未排期失败测试，按失败测试政策修复或记录为前置任务。
7. 将当前任务标题标为 `[DONE]`，补全完成记录；只有阶段级计划变化时才更新 `PLAN.md`。
8. 检查 git 状态，将本轮相关变更一次性提交，提交信息包含任务编号或明确描述。
9. 完成一个任务后停止，不继续处理下一个任务。

## 进度记录
- 已写入初始计划。下一步读取 `TODO.md` 并识别本轮任务。
- 已识别本轮任务：`C0-1 建 workspace 骨架 + crate 划分`。
- 最近提交为 `82ab1af Initial commit`，未明确提到与 C0-1 直接相关的未完成事项。
- 下一步读取 `DESIGN.md`/必要 `PLAN.md` 片段，确认 workspace 与 crate 依赖边界后开始建骨架。
- 已核对 `DESIGN.md` §2 与 `../agent-lib/Cargo.toml`：workspace 使用 `resolver = "3"`，edition 采用 2024；本轮只创建 `mag-protocol`、`mag-core`、`mag-tools`、`mag-sources`。
- 已创建四个 crate 的目录骨架，下一步写入 Cargo 配置与最小库文件。
- 已写入 root `Cargo.toml`、四个 crate 的 `Cargo.toml` 与最小 `src/lib.rs`，并补充根 `README.md`。
- 下一步执行验证：`cargo fmt`/fmt check、`cargo build --workspace`、`cargo tree -p mag-protocol`、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`、`cargo doc --no-deps --workspace`。
- 验证已全部通过：格式化、构建、`mag-protocol` 依赖图、clippy、workspace 测试与文档生成均为绿色。
- 已将 `TODO.md` 中 C0-1 标题改为 `[DONE]` 并补充完成记录。
- 已新增 `.gitignore` 忽略 `/target/`，避免提交构建产物；`Cargo.lock` 作为 workspace 依赖锁定文件保留。
- 下一步检查 git 状态并提交本轮变更。
- C0-1 已完成并随本轮 git 提交保存；本轮按要求停止，不继续处理 C0-2。
