# Claude 执行计划 — M1-1 新建 `mag-acp` crate + `map` 纯函数

## 当前任务
`TODO.md` 首个未完成任务 = **M1-1**（标题带 `[TODO]`）。
- 唯一设计输入：`docs/ACP.md`（§1/§3.1/§4）+ `docs/DESIGN.md` §3.0/§5、`PLAN.md`。
- 范围：只建 `mag-acp` crate 骨架 + 无 IO 的 `map` 模块最小子集（3 个纯函数）。
  **不**注册 handler、**不**接传输（那是 M1-2/M1-3）。

## 依赖边界（硬约束）
- `mag-acp/Cargo.toml` 只依赖 `mag-service`（path）+ `agent-client-protocol`（"1"）。
- **不得** 依赖 `mag-core` / `agent-lib` / tauri / axum。
- M1-1 的 `map` 模块无需 serde/tokio/futures/uuid —— 保持最小。
  - 为避免引入 `uuid` 依赖（不在允许清单），`acp_session_id_to_mag` 不外泄 `uuid::Error`，
    改用 mag-acp 本地错误类型 `InvalidSessionId`（含 offending 字符串）。
  - 测试不引入 uuid：用 `mag_service::SessionId::parse_str("<合法 uuid 字符串>")` 造 mag SessionId。

## 已核对的 acp crate 真实 API（cargo 缓存 v1.2.0 / schema v1.4.0）
- 路径：`agent_client_protocol::schema::v1::{SessionId, AgentCapabilities, PromptCapabilities}`。
- `SessionId(pub Arc<str>)`，`#[non_exhaustive]`，`SessionId::new(impl Into<Arc<str>>)`；
  `String: Into<Arc<str>>`，字段 `.0` 为 pub。
- `AgentCapabilities`：`#[derive(Default)]`+`#[non_exhaustive]`；`new()==default()`（全 false）；
  builder `.load_session(bool)` / `.prompt_capabilities(PromptCapabilities)` 等。
- `PromptCapabilities`：`#[derive(Default)]`+`#[non_exhaustive]`；字段 `image/audio/embedded_context:bool`。
- 默认 features 空；schema::v1 类型无需任何 unstable feature。与 docs/ACP.md §1/§3.1 一致，无需修正锚点。

## 做什么
1. `crates/mag-acp/Cargo.toml`：deps mag-service(path) + agent-client-protocol="1"。
2. 根 `Cargo.toml` `[workspace] members` 加 `"crates/mag-acp"`。
3. `crates/mag-acp/src/lib.rs`：`#![warn(missing_docs)]` + crate rustdoc + `pub mod map;`。
4. `crates/mag-acp/src/map.rs`：mag_session_id_to_acp / InvalidSessionId / acp_session_id_to_mag /
   agent_capabilities（保守）+ tests。

## 验证序列
1. `cargo fmt --all -- --check`  2. `cargo test -p mag-acp map::`  3. `cargo clippy --all-targets -- -D warnings`
4. `cargo test --workspace`  5. `cargo doc --no-deps --workspace`；依赖边界自检。

## 进度
- [x] 定位首个未完成任务 = M1-1
- [x] 核对 acp crate 真实 API（cargo 缓存）
- [x] 建 crate + workspace 成员 + map.rs
- [x] 验证序列 1–5（全绿）
- [x] TODO.md 标 M1-1 [DONE] + 完成记录
- [ ] 提交并停

## 完成后停止
只做 M1-1，完成后 `git commit`，不进入 M1-2。
