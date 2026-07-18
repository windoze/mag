# Claude 执行计划 — M1-2 bin 装配 + `initialize` handler + 内存管道 e2e 骨架

## 当前任务
`TODO.md` 首个未完成任务 = **M1-2**（M1-1 已 `[DONE]` 并提交）。
- 唯一设计输入：`docs/ACP.md` §1/§2/§3.1/§7 + `PLAN.md`。
- 范围：mag-acp 库暴露 `serve`（Agent.builder 注册 initialize + authenticate 占位 → connect_to），
  新增顶层 `mag` bin（唯一同时见 mag-core + mag-acp 的装配点），内存管道 e2e 证明 initialize 往返。

## 已核对的 acp crate 真实 API（cargo 缓存 v1.2.0 / schema v1.4.0）
- agent 端：`agent_client_protocol::Agent.builder().name(..)`
  `.on_receive_request(closure, on_receive_request!())` `.connect_to(transport: impl ConnectTo<Agent>+'static).await`。
- request handler 闭包：`async move |req: XxxRequest, responder: Responder<XxxResponse>, cx: ConnectionTo<Client>|
  -> Result<(), acp::Error>`；`responder.respond(resp) -> Result<(), Error>`（`()` 实现 IntoHandled → Handled::Yes）。
- `InitializeRequest{protocol_version, client_capabilities, ..}`；
  `InitializeResponse::new(protocol_version).agent_capabilities(caps)`（`auth_methods` 默认空）。
- `AuthenticateResponse::new()`（Default 空响应）。
- 内存传输：acp crate **自带** `agent_client_protocol::Channel::duplex() -> (Channel, Channel)`，
  `impl<R:Role> ConnectTo<R> for Channel`，纯 in-memory mpsc 交叉连接。→ 已有现成内存构造子，
  **无需**自搭 tokio::io::duplex 适配（任务放行条件："若无现成内存构造子才搭"）。也有 `ByteStreams`(AsyncRead/Write) 备选。
- `Client.builder().connect_with(transport: impl ConnectTo<Client>, |cx| async { .. })`，
  `cx.send_request(InitializeRequest::new(ProtocolVersion::V1)).block_task().await?`。
- `ProtocolVersion` 在 `agent_client_protocol::schema::ProtocolVersion`（不在 schema::v1）。
- `Client`/`Agent`/`Channel`/`ConnectionTo`/`Responder`/`Stdio`/`ConnectTo` 均 crate 根 re-export。
- 与 docs/ACP.md §1 一致，无需修正锚点。

## 依赖边界
- mag-acp/Cargo.toml：仍只 `mag-service` + `agent-client-protocol`（正常依赖不加 mag-core/agent-lib）。
  dev-deps：tokio(macros,rt-multi-thread,time) + futures + async-trait（仅测试/fake service 用）。
- 新 `mag` bin：依赖 mag-core + mag-acp + agent-client-protocol(Stdio) + tokio。唯一装配点。

## 做什么
1. `mag-acp/Cargo.toml`：加 dev-deps。
2. `mag-acp/src/handlers.rs`：`initialize` / `authenticate` 两个 `pub(crate) async fn`（带 rustdoc）。
3. `mag-acp/src/lib.rs`：`mod handlers;` + `pub async fn serve<T: ConnectTo<Agent>+'static>(service: Arc<dyn MagService>, transport: T) -> Result<(), acp::Error>`。
4. `mag-acp/tests/e2e.rs`：内存管道夹具（Channel::duplex）+ 最小 fake `Arc<dyn MagService>` + `initialize` 往返断言 caps==agent_capabilities()。
5. `crates/mag`：bin crate（`mag --acp` → `Engine::new()` 装 Arc<dyn MagService> → `serve(service, Stdio::new())`）。加入 workspace members。

## 验证序列
1. `cargo fmt --all -- --check`  2. `cargo test -p mag-acp initialize`  3. `cargo clippy --all-targets -- -D warnings`
4. `cargo test --workspace`  5. `RUSTDOCFLAGS=-D warnings cargo doc --no-deps --workspace`；依赖边界自检。

## 进度
- [x] 定位首个未完成任务 = M1-2
- [x] 核对 acp crate agent/client/transport API
- [x] 实现 serve + handlers + bin + e2e
- [x] 验证序列 1–5 全绿（+真实 stdio 冒烟绿）
- [x] TODO.md 标 M1-2 [DONE] + 完成记录
- [x] 提交并停

## 完成后停止
只做 M1-2，完成后 `git commit`，不进入 M1-3。
