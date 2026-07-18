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

## C0-2 执行记录（2026-07-18）

## 已识别任务
- 首个未完成任务：`C0-2 mag-protocol：Command / Event + payload（全 serde）`。
- 最新提交为 `[C0-1] Build workspace skeleton`，未提到与当前任务相关的未完成问题。
- 当前工作区只有 `memory/claude_plan.md` 是本轮新增/修改，后续提交需要包含它。

## C0-2 具体计划
1. 对照 `DESIGN.md` §4.1/§4.2 与 §8.2，定义 `Command`、`Event`、`SessionConfig`、ID 包装类型、交互 wire 类型、工具/委派/来源 payload。
2. 从 `agent-lib` 源码核对 `InteractionKind`、`InteractionResponse`、`PermissionRequest`、`PermissionRisk` 的字段语义；只镜像 wire 结构，不让 `mag-protocol` 依赖 `agent-lib`。
3. 为 `mag-protocol` 增加必要依赖：`uuid` 用于 `SessionId`/`RequestId`/`RunId`，`serde_json` 用于测试。
4. 用小而集中的模块实现协议类型，所有公开 API 保持 rustdoc，增长型 enum 标注 `#[non_exhaustive]`，顶层 enum 使用 `#[serde(tag = "type", rename_all = "snake_case")]`。
5. 添加单元测试：覆盖每个 `Command` 与 `Event` 变体的 `serde_json` round-trip，并显式断言若干 tag 名称稳定。
6. 运行验证：`cargo fmt --all -- --check`、`cargo test -p mag-protocol`、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`、`cargo doc --no-deps --workspace`。
7. 验证通过后，把 TODO.md 中 C0-2 标题改为 `[DONE]` 并补完成记录。
8. 提交本轮改动并停止，不进入 C0-3。

## 进展
- 已读取 `TODO.md`、最新提交、工作区状态、`DESIGN.md` 协议章节和当前 crate 骨架。

## 进展更新
- 已为 workspace 增加 `uuid` 与 `serde_json` 依赖。
- 已在 `mag-protocol` 中实现 `Command`/`Event`、UUID 包装 ID、会话配置、工具/委派/来源 payload、交互请求/响应 wire 类型与 round-trip 测试。
- 下一步：格式化并运行聚焦测试，修正编译或 serde 细节。

## 进展更新
- `cargo fmt --all` 已执行。
- `cargo fmt --all -- --check` 通过。
- `cargo test -p mag-protocol` 通过：5 个单元测试和 doctest 全绿。
- 下一步：运行 clippy、workspace 测试和文档构建。

## 进展更新
- `cargo clippy --all-targets -- -D warnings` 通过。
- 下一步：用 30 分钟上限运行 `cargo test --workspace`，随后运行 `cargo doc --no-deps --workspace`。

## 进展更新
- `cargo test --workspace` 已通过：8 个单元测试与 doctest 全绿。
- `cargo doc --no-deps --workspace` 已通过。
- `TODO.md` 已将 C0-2 标记为 `[DONE]` 并补完成记录。
- 下一步：检查依赖图和工作区状态，然后提交。

## 进展更新
- `cargo tree -p mag-protocol` 已确认依赖只有 `serde`、`serde_json`、`uuid`，不包含 `agent-lib`。
- 最终待提交文件：`Cargo.lock`、`Cargo.toml`、`TODO.md`、`crates/mag-protocol/Cargo.toml`、`crates/mag-protocol/src/lib.rs`、`memory/claude_plan.md`。
- 下一步：提交 `[C0-2] Implement protocol wire types`，然后停止。
