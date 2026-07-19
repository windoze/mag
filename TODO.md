# TODO：mag-acp 落地任务单（mag 作为 ACP server）

> 依据 [`PLAN.md`](PLAN.md) 与**唯一设计输入** [`docs/ACP.md`](docs/ACP.md)（参照 [`docs/DESIGN.md`](docs/DESIGN.md)
> §5 概览、§3.0 `MagService` trait）。
> **范围：只做 `mag-acp` crate（mag 第一个 interface，ACP agent 端）。** service 主干（`mag-service` +
> `mag-core` + `mag-tools`/`mag-sources`）已全部 `[DONE]`、`MagService` 契约已冻结，其已完成计划归档在
> [`docs/archive/2026-07-19-mag-service/`](docs/archive/2026-07-19-mag-service/)。其它 interface（web / Tauri /
> 前端）不在本单内。

## 通用执行规则

- **一次一个任务**：每次只执行「首个标题带 `[TODO]` 的任务」。完成后把 `[TODO]` 改为 `[DONE]`，在任务末尾
  补「完成记录」，然后停止，等待下一次调用。
- **完成的定义**：只有标题带 `[DONE]` 才算完成。仅填完成记录而标题仍 `[TODO]`，按未完成处理。review 任务
  （`M<n>-R`）是真实任务，不得跳过。
- **编号**：任务按实现顺序编号 `M<里程碑>-<序号>`（如 `M1-1` = milestone 1 第一个任务）；每个里程碑末尾有
  独立 review 任务 `M<n>-R`。
- **依赖边界（硬约束）**：`mag-acp` 的 `Cargo.toml` **只依赖** `mag-service` + `agent-client-protocol`
  （+ 其 schema crate、tokio、futures、serde/serde_json、async-trait 视需要）；**不得**依赖 `mag-core` /
  `agent-lib` / tauri / axum。mag-acp 库对 service 只见 `Arc<dyn MagService>`；装配 `mag-core::Engine` 注入的
  是上层 bin。
- **不改已冻结的 `MagService` 契约**：只消费现有 trait 方法与 `ServiceEvent` 变体（`docs/DESIGN.md` §3.0）。
  若缺映射/缺字段，**不得**在 mag-acp 侧臆造 ACP 语义或 papering over——在本文件正确依赖位置插最小前置任务
  （回 service 主干按向后兼容加：只加方法/变体/字段），让被阻塞任务显式依赖它，然后提交并停止。
- **acp crate 缺口同理**：遇缺 API / 类型不匹配 / 无内存传输构造子，插最小前置任务修正锚点或搭适配，不绕过。
- **离线测试纪律**：所有 mag-acp 测试必须离线——映射纯函数直接单测；handler 级注入 fake/scripted
  `Arc<dyn MagService>`；协议级 e2e 用 acp crate 自身 client 经**内存管道**（如 `tokio::io::duplex`）驱动
  mag-acp，service 侧注入 fake LLM。不依赖网络 / 真实凭据 / 真实 Zed / 子进程。每个测试须 1 分钟内完成，卡住
  即为 bug，须立刻修。真实 Zed 联调一律 `#[ignore]`，缺环境干净跳过（绿），不输出 secret。
- **审批是异步暂停点**：`bridge_permission` 必须真正 `await` client 的 `RequestPermissionResponse` 再
  `respond_interaction`（`docs/ACP.md` §5）；测试须断言 driver 在 outcome 回灌前不前进。
- **能力如实宣告**：`initialize` 只打开实现确已支持的 `AgentCapabilities` 位（`docs/ACP.md` §3.1/§7）。
- **默认完整验证序列**（任务另有放宽以任务为准）：
  1. `cargo fmt --all -- --check`
  2. 聚焦测试（任务给出精确过滤名）
  3. `cargo clippy --all-targets -- -D warnings`
  4. `cargo test --workspace`
  5. `cargo doc --no-deps --workspace`
- **公开 API 必须带 rustdoc**（crate 开 `#![warn(missing_docs)]`）。

### 复用锚点（各任务通用，避免反复翻库）

- **acp crate**（`agent-client-protocol` v1.2.0；schema `agent-client-protocol-schema` v1.4.0，经
  `agent_client_protocol::schema::v1::*`；来源 `docs/ACP.md` §1）：角色标记
  `agent_client_protocol::Agent` → `.builder()`；`.on_receive_request(closure, on_receive_request!())` /
  `.on_receive_notification(closure, on_receive_notification!())`；`.connect_to(Stdio::new()).await`（run loop）。
  request handler 闭包：`async move |req: XxxRequest, responder: Responder<XxxResponse>, cx:
  ConnectionTo<Client>| -> Result<_, acp::Error>`；`responder.respond(resp)` / `respond_with_error(err)`。
  出站：`cx.send_notification(SessionNotification::new(sid, SessionUpdate::..))`；
  `cx.send_request(RequestPermissionRequest::new(..)).block_task().await?`。
- **`MagService` trait**（`crates/mag-service/src/service.rs`，object-safe，`Arc<dyn MagService>`）：
  `create_session(SessionConfig)->SessionId`、`list_sessions()->Vec<SessionInfo>`、`resume_session(SessionId)`、
  `delete_session(SessionId)`、`send_message(SessionId,UserInput)->RunId`、`cancel(SessionId)`、
  `respond_interaction(SessionId,RequestId,InteractionResponseWire)`、
  `subscribe(Option<SessionId>)->BoxStream<'static,ServiceEvent>`、`list_sources()`、`probe_local_agents()`；
  async 方法返回 `Result<_,ServiceError>`。
- **`ServiceEvent`**（`crates/mag-service/src/service.rs`，`#[serde(tag="type",rename_all="snake_case")]`）：
  `SessionCreated{id,config}`、`RunStarted{id,run_id}`、`RunFinished{id,output:RunOutput}`、
  `RunError{id,message,kind:RunErrorKind}`、`TextDelta{id,text}`、`ToolStarted{id,trace:ToolTrace}`、`ToolFinished{id,trace}`、
  `InteractionRequested{id,request_id:RequestId,kind:InteractionKindWire}`、
  `DelegationStarted/Finished/Failed{id,trace:DelegationTrace}`、`DelegationMessage{id,message}`、
  `LocalAgentsProbed{available}`；`ServiceEvent::session_id()->Option<SessionId>`。
- **wire 类型**（`crates/mag-service/src/lib.rs`）：`SessionId`/`RunId`/`RequestId`（`transparent` 包 `Uuid`；
  `new`/`parse_str`/`as_uuid`/`Display`/`FromStr`）；`UserInput{text,attachments}`（`UserInput::text(..)`）；
  `SessionConfig{provider,model,tool_profile,cwd,routing:RoutingMode,budget:Option<SessionBudget>}`；
  `SessionBudget{max_steps,max_tokens,max_cost_micros,max_wall_time_secs}`（全 `Option<u64>`）；
  `RunErrorKind::{Other,Cancelled,LoopLimitExceeded,BudgetExhausted}`（`#[serde(default)]`，缺省 `Other`）；
  `RunOutput{text,usage}`；
  `ToolTrace{run_id,call_id:ToolCallIdWire,name,input,output,status:ToolStatusWire,message}`；
  `InteractionKindWire{Approval{call_id,requirement},Question{prompt},Choice{prompt,options},
  Permission{action_id,actor,category,risk,summary,subject,reason}}`；
  `InteractionResponseWire{Approval{step_id,call_id,decision:ApprovalDecisionWire,message},Answer{text},
  Choice{index},Permission{action_id,decision:PermissionDecisionWire}}`；
  `ApprovalDecisionWire::{Approve,Deny,Timeout,Cancel}`；`PermissionDecisionWire::{Approve,Deny{reason},Cancel}`。

---

## Milestone M1 — crate 骨架 + `initialize` + `session/new` + stdio 跑通

目标：新建 `mag-acp` crate（依赖边界正确），落地无 IO 的 `map` 纯函数（SessionId 映射 + 保守能力宣告），
组 acp `Agent.builder()` 注册 `initialize` / `session/new` handler、`connect_to` 跑起来，并用内存管道 e2e
证明 stdio 路径的握手与建会话往返。对应 `docs/ACP.md` §1 / §2 / §3.1 / §3.2。

### [DONE] M1-1 新建 `mag-acp` crate + `map` 纯函数（SessionId 映射 + 能力宣告）

**上下文**：

- mag-acp 是 mag 第一个 interface（`docs/ACP.md` §0/§2）：一层纯协议翻译器，只依赖 `mag-service` + acp
  crate，面对 `Arc<dyn MagService>`。本任务只建 crate 骨架 + 无 IO 的 `map` 模块最小子集，不注册 handler、不接
  传输（那在 M1-2）。
- 需要的两个纯函数：
  1. **SessionId 映射**（`docs/ACP.md` §4）：ACP `SessionId`（`Arc<str>`）↔ mag `SessionId`（uuid）。第一版
     **直接用 mag `SessionId` 的字符串形式作 ACP SessionId**，省一张双向表：`mag→acp` = `sid.to_string()`；
     `acp→mag` = `mag_service::SessionId::parse_str(s)`（返回 `Result`，非法字符串是协议错误）。
- **能力宣告**（`docs/ACP.md` §3.1/§7）：一个纯函数 `agent_capabilities() -> AgentCapabilities`，第一版保守：
  `load_session=false`（M4-2 再按恢复就绪度收口）、`prompt_capabilities` 的 `image`/`audio`/`embedded_context`
  全 `false`（文本优先）、`mcp_capabilities`/`session_capabilities`/`auth` 按未实现填保守值、`auth_methods` 留空。

**做什么**：

1. 新建 `crates/mag-acp`：`Cargo.toml` 声明 `name = "mag-acp"`，`version.workspace`/`edition.workspace`；依赖
   `mag-service = { path = "../mag-service" }`、`agent-client-protocol = "1"`（若 API 需要，加其 schema crate）、
   `futures`/`serde`/`serde_json`（视需要）。**不得**加 `mag-core` / `agent-lib`。把 `crates/mag-acp` 加入根
   `Cargo.toml` 的 `[workspace] members`。
2. `src/lib.rs`：`#![warn(missing_docs)]` + crate 级 rustdoc（一句话定位 + 指向 `docs/ACP.md`）；`pub mod map;`。
3. `src/map.rs`：实现并导出
   - `pub fn mag_session_id_to_acp(id: mag_service::SessionId) -> acp::SessionId`（用 `Display`/`to_string`）。
   - `pub fn acp_session_id_to_mag(id: &acp::SessionId) -> Result<mag_service::SessionId, _>`（`parse_str`）。
   - `pub fn agent_capabilities() -> AgentCapabilities`（保守宣告，见上下文）。
   每个 pub item 带 rustdoc。
4. 就地对 `agent_client_protocol::schema::v1` 核对 `SessionId` / `AgentCapabilities` 的真实构造子与字段拼写；
   若与 `docs/ACP.md` §1/§3.1 有出入，**修正本任务锚点并在完成记录里注明**（不改主干）。

**验证条件**：

- 聚焦测试：`cargo test -p mag-acp map::`，覆盖：mag→acp→mag round-trip 相等；非法 ACP SessionId 字符串
  `acp_session_id_to_mag` 返回 `Err`；`agent_capabilities()` 的保守位断言（`load_session==false`、多模态位全
  `false`）。
- 完整验证序列 1–5（见「通用执行规则」）；`cargo clippy --all-targets -- -D warnings` 无警告；`cargo doc`
  无缺 doc 警告。
- 依赖边界自检：`crates/mag-acp/Cargo.toml` 不含 `mag-core` / `agent-lib`。

**完成记录（M1-1）**：

- 新建 `crates/mag-acp`：`Cargo.toml` 仅依赖 `mag-service = { path = "../mag-service" }` 与
  `agent-client-protocol = "1"`（无 `mag-core` / `agent-lib` / tauri / axum，依赖边界满足）；`version`/`edition`
  用 `.workspace`；已加入根 `Cargo.toml` 的 `[workspace] members`。
- `src/lib.rs`：`#![warn(missing_docs)]` + crate 级 rustdoc（定位 mag-acp 为 ACP agent 端翻译器 + 指向
  `docs/ACP.md`/`PLAN.md`/`TODO.md`），`pub mod map;`。
- `src/map.rs`（无 IO 纯函数，均带 rustdoc）：
  - `mag_session_id_to_acp(id) -> acp::SessionId`：用 `id.to_string()` 作 ACP `SessionId`（省双向表）。
  - `acp_session_id_to_mag(&acp::SessionId) -> Result<mag_service::SessionId, InvalidSessionId>`：
    `parse_str(id.0.as_ref())`，非法字符串（非 UUID）返回本地错误 `InvalidSessionId{value}`（带 Display+Error）。
    刻意不外泄 `uuid::Error`，从而 `Cargo.toml` 无需引入 `uuid`，保持依赖边界最小。
  - `agent_capabilities() -> acp::AgentCapabilities`：保守宣告——`load_session=false`、`prompt_capabilities`
    的 `image`/`audio`/`embedded_context` 全 `false`；`mcp_capabilities`/`session_capabilities`/`auth` 取默认
    （未宣告）、无 `auth_methods`。
- 锚点核对（就地对 cargo 缓存 `agent-client-protocol` v1.2.0 / schema v1.4.0）：
  路径 `agent_client_protocol::schema::v1::{SessionId, AgentCapabilities, PromptCapabilities}` 属实；
  `SessionId(pub Arc<str>)` + `SessionId::new(impl Into<Arc<str>>)`；`AgentCapabilities`/`PromptCapabilities`
  均 `#[derive(Default)]`+`#[non_exhaustive]`，`new()==default()`（保守位全 `false`），builder
  `.load_session/.prompt_capabilities/.image/.audio/.embedded_context` 存在。与 `docs/ACP.md` §1/§3.1 一致，
  **无需修正锚点**（default features 空，schema::v1 类型无需任何 unstable feature）。
- 验证（全绿）：1) `cargo fmt --all -- --check` OK；2) `cargo test -p mag-acp map::` 3 passed（round-trip
  相等 / 非法 SessionId → Err / 保守能力位断言）；3) `cargo clippy --all-targets -- -D warnings` 无警告；
  4) `cargo test --workspace` 全通过；5) `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace` 无缺 doc 警告。

### [DONE] M1-2 bin 装配 + `initialize` handler + 内存管道 e2e 骨架

**上下文**：

- acp 的 agent 端用 builder + typed-handler（`docs/ACP.md` §1）：`agent_client_protocol::Agent` → `.builder()`
  → `on_receive_request(closure, on_receive_request!())` → `.connect_to(transport).await`（run loop）。
- 本任务只接 `initialize`（`InitializeRequest`/`InitializeResponse`）：读 `protocol_version` /
  `client_capabilities`，回 `InitializeResponse::new(version).agent_capabilities(agent_capabilities())`
  （复用 M1-1 的纯函数）。`authenticate` 第一版不做实质认证（`docs/ACP.md` §3.1）：`auth_methods` 留空，
  handler 返回成功空响应或不注册（视 client 要求，按 acp crate 实际要求择一）。
- **内存传输**（`docs/ACP.md` §9、`PLAN.md` R-2）：协议级 e2e 需一个非 stdio 的内存管道把 acp client 与
  mag-acp server 对接。若 acp crate `connect_to` 只接受具体传输而无现成内存构造子，本任务搭一个基于
  `tokio::io::duplex` 的传输适配（属实现细节，非绕过）。这是后续 e2e 的公用夹具。
- mag-acp 库对 service 只见 `Arc<dyn MagService>`；用于测试的 fake service 可先用 mag-service 单元测试里那种
  最小 stub（返回默认值 / 空事件流）。

**做什么**：

1. `src/lib.rs`：暴露库入口 `pub async fn serve<T>(service: Arc<dyn MagService>, transport: T) -> Result<(),
   acp::Error>`（或等价签名）——内部 `Agent::builder()` 注册各 handler（本任务先只 `initialize` +
   `authenticate` 占位）后 `connect_to(transport).await`。handler 闭包捕获 `service.clone()`。
2. 新增 `src/handlers.rs`（或就近模块）：`initialize` handler，签名照「复用锚点」，回
   `InitializeResponse::new(req.protocol_version).agent_capabilities(agent_capabilities())`（协商版本回传，
   `docs/ACP.md` §7）。
3. 新增顶层 **`mag` bin crate**（`crates/mag` 或 `src/bin`；依赖 `mag-core` + `mag-acp`）：`mag --acp` 子命令
   装配 `mag_core::Engine` 得 `Arc<dyn MagService>`，调 `mag_acp::serve(service, Stdio::new())`。此 bin 是唯一
   同时见 mag-core 与 mag-acp 的地方，保持 mag-acp 库的依赖边界。加入 workspace members。
4. 测试用内存管道夹具：`tests/e2e.rs` 或 `#[cfg(test)]`，用 acp crate 自身 client 端经 `tokio::io::duplex`
   连到 mag-acp server，跑 `initialize` 往返，断言回的 `AgentCapabilities` 与 `agent_capabilities()` 一致。

**验证条件**：

- 聚焦测试：`cargo test -p mag-acp initialize`（含内存管道 `initialize` 往返 e2e）在 1 分钟内绿。
- 完整验证序列 1–5；clippy 无警告；`cargo doc` 无缺 doc 警告。
- 依赖边界自检：`mag-acp` 仍不依赖 `mag-core`/`agent-lib`；只有新 `mag` bin crate 依赖二者。

**完成记录（M1-2）**：

- **`serve` 库入口**（`crates/mag-acp/src/lib.rs`）：`pub async fn serve<T>(service: Arc<dyn MagService>,
  transport: T) -> Result<(), agent_client_protocol::Error> where T: ConnectTo<Agent> + 'static`。内部
  `Agent.builder().name("mag-acp")` 注册 `initialize` + `authenticate` 两个 request handler（经
  `on_receive_request!()` 宏），`.connect_to(transport).await` 跑 run loop。`service` 已进签名（未被
  initialize/authenticate 消费，后续 handler 捕获用；Rust 不对未用函数参数告警）。
- **handlers**（`crates/mag-acp/src/handlers.rs`，`pub(crate) async fn`，带 rustdoc）：
  - `initialize`：回 `InitializeResponse::new(req.protocol_version).agent_capabilities(map::agent_capabilities())`
    （版本协商回传 §7 + 复用 M1-1 保守能力 §3.1）。
  - `authenticate`：`initialize` 不宣告 `auth_methods`，正常 client 不会调用；占位返回
    `AuthenticateResponse::new()`（空成功）以稳健应对探测（§3.1）。
- **`mag` bin crate**（`crates/mag/`，已加入根 `Cargo.toml` `[workspace] members`）：依赖
  `mag-core` + `mag-acp` + `mag-service` + `agent-client-protocol`（唯一同时见 mag-core 与 mag-acp 的装配点，
  保住 mag-acp 库依赖边界）。`mag --acp` → `Arc::new(Engine::new()) as Arc<dyn MagService>` →
  `mag_acp::serve(service, Stdio::new()).await`；无 `--acp` 打印 usage 并 exit(2)。provider/model 装配随来源
  配置后续里程碑补齐（ACP 不传 LLM 选择），M1-2 bin 为握手骨架。
- **内存管道夹具 + e2e**（`crates/mag-acp/tests/e2e.rs`）：直接复用 acp crate **自带**的
  `agent_client_protocol::Channel::duplex()`（纯内存 mpsc 交叉连接，`impl<R:Role> ConnectTo<R> for Channel`）
  作内存传输——已有现成内存构造子，故**未**自搭 `tokio::io::duplex` 适配（任务放行条件："若 acp 无现成内存
  构造子才搭"；如需字节级序列化可改用 `ByteStreams`(AsyncRead/Write)，备选已核实）。测试用真实
  `Client.builder().connect_with(client_transport, |cx| …)` 发 `InitializeRequest::new(ProtocolVersion::V1)`，
  断言回吐的 `agent_capabilities` 与 `map::agent_capabilities()` 全等且保守位（`load_session`/`image` 为
  `false`）。服务端 `tokio::spawn`，客户端调用包 `timeout(10s)` 防挂起，完成后 `server.abort()`。
- **锚点核对**（就地对 cargo 缓存 acp v1.2.0 / schema v1.4.0）：agent 端 role-marker + builder + typed-handler
  模型属实（`Agent.builder()`/`on_receive_request(closure, on_receive_request!())`/`connect_to(impl
  ConnectTo<Agent>+'static)`）；handler 闭包 `async move |req, responder: Responder<Resp>, cx:
  ConnectionTo<Client>| -> Result<(), Error>`，`responder.respond(resp)->Result<(),Error>`（`()`→
  `Handled::Yes`）；`InitializeResponse::new(pv).agent_capabilities(..)` / `AuthenticateResponse::new()` /
  `ProtocolVersion::V1`（在 `schema::ProtocolVersion`，不在 `schema::v1`）均属实。与 `docs/ACP.md` §1 一致，
  **无需修正锚点**。
- **验证（全绿）**：1) `cargo fmt --all -- --check` OK；2) `cargo test -p mag-acp initialize` 1 passed（内存
  管道 `initialize` 往返，<0.01s，无挂起）；3) `cargo clippy --all-targets -- -D warnings` 无警告；
  4) `cargo test --workspace` 全通过（含 mag-acp e2e 1 + map:: 3，无失败）；5) `RUSTDOCFLAGS="-D warnings"
  cargo doc --no-deps --workspace` 无缺 doc 警告（修正 bin 一处 redundant-explicit-link）。额外真实 stdio 冒烟：
  `echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}' | mag --acp` 正确回
  `agentCapabilities`（`loadSession:false`、多模态位全 `false`、`authMethods:[]`）。
- **依赖边界自检**：`crates/mag-acp/Cargo.toml` 正常依赖仍只 `mag-service` + `agent-client-protocol`（新增
  `async-trait`/`futures`/`tokio` 仅 dev-dependencies，供 e2e 的 fake `Arc<dyn MagService>` 与运行时用）；
  无 `mag-core`/`agent-lib`/`tauri`/`axum`。

### [DONE] M1-3 `SessionConfig.cwd` 承载 + 会话 worktree 落位（service 主干向后兼容加字段）

**上下文**：

- 这是 M1-4（`session/new` → `create_session`）的**前置契约缺口修复**任务（M1-4 显式依赖本任务）。
  ACP `NewSessionRequest.cwd: PathBuf`（绝对路径，**必填**）是会话工作根（`docs/ACP.md` §3.2/§6）；mag 的
  内建工具（`read_file`/`shell`/`grep`/`list_dir`，`crates/mag-tools`）**全部**相对 facade `Agent` 的
  **worktree** 执行并受 `safe_join` 约束（`docs/DESIGN.md` §3.2）。
- **缺口（已就地核实）**：`mag_service::SessionConfig`（`crates/mag-service/src/lib.rs`）只有
  `provider/model/tool_profile/routing`，**无 cwd 字段**；`mag-core::SessionDriver::new`
  （`crates/mag-core/src/driver.rs`）建 facade `Agent` 时**从不**调 `.worktree(..)`，agent-lib 默认回落
  `WorktreeRef::new(".")`（`agent-lib/src/facade/agent.rs`）；`MagService::create_session(SessionConfig)` 是
  唯一入口，cwd 只能经 `SessionConfig` 流入。故 ACP 传入的 cwd 现在**无处承载会被丢弃**，工具会在 mag 进程
  cwd 而非客户端指定目录执行——违反 ACP §3.2/§6。按 `PLAN.md` line 36-37 / 本文件通用规则：回 service 主干
  **向后兼容**加字段（只加字段，不改既有语义），不在 mag-acp 侧丢 cwd。
- **向后兼容硬约束**：`SessionConfig` 被持久化（`crates/mag-core/src/persistence.rs`）。新字段须
  `#[serde(default)]`，旧快照（无 cwd 键）必须仍能反序列化为缺省值。

**做什么**：

1. `mag-service`（`crates/mag-service/src/lib.rs`）：给 `SessionConfig` 加
   `pub cwd: Option<std::path::PathBuf>`，`#[serde(default, skip_serializing_if = "Option::is_none")]`
   （与 `tool_profile` 同风格），带 rustdoc（会话工作根 / 映射到 agent worktree；`None` = 沿用默认 `"."`，
   兼容旧持久化）。补齐本 crate 内所有 `SessionConfig { .. }` 字面量（含测试）的 `cwd` 字段。
2. `mag-core`：
   - `SessionDriver::new`（`crates/mag-core/src/driver.rs`）：`config.cwd` 为 `Some(path)` 时对
     `Agent::builder()` 调 `.worktree(agent_lib::agent::WorktreeRef::new(path.clone()))`；`None` 保持现状
     （默认 `"."`）。`restore` 路径不动（worktree 已随 `AgentSnapshot`/`AgentSpec` 烘入，`docs/DESIGN.md` §3.6）。
   - 补齐 `mag-core` 内所有 `SessionConfig { .. }` 字面量（`engine.rs`/`persistence.rs`/`driver.rs` 及各自
     测试）的 `cwd`（默认 `None`；worktree 落位测试用 `Some`）。
3. 测试：
   - `mag-service`：`SessionConfig` serde round-trip 含 `cwd: Some(path)`；**向后兼容**——不含 `cwd` 键的旧
     JSON 反序列化 → `cwd: None`。
   - `mag-core`：断言 `SessionDriver::new(config cwd=Some(p), ..)` 建出的 agent worktree == `p`（经
     `self.agent.snapshot()` 序列化后断言 `worktree` 路径，或 agent-lib 若暴露可读访问器则直接读）；
     `cwd=None` → worktree == `"."`。（已核实 worktree 序列化进快照，故从 mag-core 侧可观测；若发现确实
     不可观测，按同规则插 agent-lib 前置任务。）

**验证条件**：

- 聚焦测试：`cargo test -p mag-service session_config` + `cargo test -p mag-core worktree`（1 分钟内绿）。
- 完整验证序列 1–5（`cargo fmt` → 聚焦 → `cargo clippy --all-targets -- -D warnings` → `cargo test --workspace`
  → `cargo doc`）。
- 依赖边界不变：**不动** mag-acp；本任务只改 `mag-service` + `mag-core`。完成后 M1-4 可无损把 ACP cwd 映射进
  `SessionConfig.cwd`。
- **完成后同步**：更新 `PLAN.md` §`mag-service` 契约清单（约 line 105）把 `SessionConfig` 字段补上 `cwd`
  （该处是契约事实描述，随字段落地一并订正；这是契约字段变更，非阶段计划改写）。

**完成记录（M1-3）**：

- **`mag-service`（`crates/mag-service/src/lib.rs`）**：`SessionConfig` 新增
  `pub cwd: Option<std::path::PathBuf>`，属性 `#[serde(default, skip_serializing_if = "Option::is_none")]`
  （与 `tool_profile` 同风格），带 rustdoc（会话工作根 → agent worktree；`None` = 默认 `"."`，且
  `#[serde(default)]` 保旧持久化兼容）。新增 `use std::path::PathBuf`。补齐本 crate 内 `SessionConfig` 字面量
  （`lib.rs` 测试 `config()`、`service.rs` 测试 `config()`）的 `cwd: None`。
- **`mag-core`**：
  - `SessionDriver::new`（`crates/mag-core/src/driver.rs`）：`config.cwd` 为 `Some(path)` 时对
    `Agent::builder()` 调 `.worktree(WorktreeRef::new(cwd.clone()))`（新 `use agent_lib::agent::WorktreeRef`）；
    `None` 保持现状（facade 默认 `"."`）。`restore` 路径不动（worktree 已随快照烘入）。rustdoc 补 cwd→worktree
    说明。
  - 补齐 `mag-core` 全部 `SessionConfig` 字面量的 `cwd: None`：`engine.rs` ×5、`persistence.rs` ×1、
    `tests/e2e_offline.rs` ×1（均为测试夹具，业务默认 `None`）。
- **测试**：
  - `mag-service`（`cargo test -p mag-service session_config`，3 passed）：`session_config_round_trips_cwd`
    （`cwd: Some("/work/session-root")` serde round-trip 且 JSON 现 `cwd` 键）；
    `session_config_without_cwd_is_backward_compatible`（旧 JSON 无 `cwd` 键 → `cwd: None`，且回序列化不写
    `null` 键）；原 `session_config_defaults_to_model_routed` 仍绿。
  - `mag-core`（`cargo test -p mag-core worktree`，2 passed）：`new_carries_config_cwd_into_agent_worktree`
    经 `driver.agent.snapshot()` 序列化断言 `["agent_state"]["spec"]["worktree"] == "/work/session-root"`；
    `new_without_cwd_keeps_default_worktree` 断言 worktree == `"."`。（快照可观测性已核实：`AgentSnapshot.agent_state`
    透明包 `AgentStateRecord.spec: AgentSpec.worktree: WorktreeRef`（transparent path）。）
- **锚点核对**（就地对 `../agent-lib`）：`agent_lib::agent::WorktreeRef`（`spec.rs:95`，`#[serde(transparent)]`
  包 `PathBuf`，`WorktreeRef::new(impl Into<PathBuf>)`）经 `agent/mod.rs:92` 导出；
  `Agent::builder().worktree(WorktreeRef)`（`facade/agent.rs:1086`）存在；默认回落 `WorktreeRef::new(".")`
  （`facade/agent.rs:1295`）。与 `docs/DESIGN.md` §3.2 一致，**无需插 agent-lib 前置任务**。
- **依赖边界**：只改 `mag-service` + `mag-core`；**未动** mag-acp（其依赖边界不变）。
- **验证（全绿）**：1) `cargo fmt --all -- --check` OK；2) 聚焦 `mag-service session_config` 3 passed +
  `mag-core worktree` 2 passed（均 <0.01s）；3) `cargo clippy --all-targets -- -D warnings` 无警告；
  4) `cargo test --workspace` 全通过（0 failed）；5) `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace`
  无缺 doc 警告。
- **PLAN.md 同步**：§`mag-service` 契约清单（line 105）`SessionConfig` 补 `cwd:Option<PathBuf>`（契约字段订正，
  非阶段计划改写）。M1-4 现可无损把 ACP `cwd` 映射进 `SessionConfig.cwd`。

### [DONE] M1-4 `session/new` handler → `create_session`（cwd → `SessionConfig.cwd`）

**依赖**：M1-3（`SessionConfig.cwd` 字段）。M1-3 未 `[DONE]` 前不得开始本任务。

**上下文**：

- `session/new`（`docs/ACP.md` §3.2）：`NewSessionRequest{cwd（绝对路径）, additional_directories,
  mcp_servers}`。组一个 `SessionConfig`（mag-service 类型）：`cwd` → 会话工作根（映射到 agent worktree /
  只读根，安全见 `docs/ACP.md` §6）；`provider`/`model` 用 mag 默认或配置来源（ACP 不传 LLM 选择）；
  `mcp_servers` 第一版可忽略（后续来源接入点，非本单范围）。
- 调 `service.create_session(cfg).await`，把返回的 mag `SessionId` 用 M1-1 的 `mag_session_id_to_acp` 映射为
  ACP `SessionId`，回 `NewSessionResponse::new(acp_session_id)`。
- `SessionConfig{provider,model,tool_profile,routing,cwd}`：第一版 `provider`/`model` 取一个明确的默认常量（如
  `provider="openai"`、`model` 用占位默认），`tool_profile=None`、`routing=RoutingMode::default()`；cwd 经 M1-3
  新增字段落位——`cwd = Some(req.cwd.clone())`（ACP cwd 为绝对路径，直接承载，不在 mag-acp 侧丢弃）。

**做什么**：

1. `map`：`pub fn new_session_request_to_config(req: &NewSessionRequest) -> SessionConfig`（纯函数，含默认
   provider/model + `cwd = Some(req.cwd.clone())`），带 rustdoc + 单测。
2. handler：`session/new` request handler，调 `service.create_session(..)`，映射并回 `NewSessionResponse`。
3. e2e：扩展 M1-2 的内存管道夹具，跑 `initialize → session/new`，断言 fake service 收到 `create_session`
   （且 `config.cwd == Some(req.cwd)`）且回的 ACP SessionId 可被 `acp_session_id_to_mag` 解回同一 mag SessionId。

**验证条件**：

- 聚焦测试：`cargo test -p mag-acp session_new`（纯函数 + handler 级 + 内存管道往返）1 分钟内绿。
- 完整验证序列 1–5；clippy / doc 无警告。
- 本任务显式依赖 M1-3，并在完成记录注明 cwd 经 `SessionConfig.cwd` 承载。

**完成记录（M1-4）**：

- **`map`（`crates/mag-acp/src/map.rs`）**：新增纯函数
  `pub fn new_session_request_to_config(req: &acp::NewSessionRequest) -> SessionConfig`，
  把 ACP 绝对 `cwd` 直接承载为 `SessionConfig.cwd = Some(req.cwd.clone())`（**不丢弃**，M1-3 字段落位）；
  `provider`/`model` 取新增常量 `DEFAULT_PROVIDER="openai"` / `DEFAULT_MODEL="gpt-5-codex"`（ACP 不传 LLM
  选择，与 `mag-service` 既有 fixture 一致）、`tool_profile=None`、`routing=RoutingMode::default()`；
  `additional_directories`/`mcp_servers` 第一版忽略（`docs/ACP.md` §3.2）。带 rustdoc。新增
  `use mag_service::{RoutingMode, SessionConfig}`。
- **handler（`crates/mag-acp/src/handlers.rs`）**：新增
  `pub(crate) async fn session_new(service, request, responder, connection)`：经
  `map::new_session_request_to_config` 组 `SessionConfig` → `service.create_session(cfg).await` →
  成功用 `map::mag_session_id_to_acp` 映射回 ACP `SessionId` 并回 `NewSessionResponse::new(id)`；
  `ServiceError` 经 `agent_client_protocol::Error::into_internal_error` 转内部 JSON-RPC 错误。新增
  `use std::sync::Arc` + `mag_service::MagService`。
- **装配（`crates/mag-acp/src/lib.rs`）**：`serve` 在 builder 上注册第三个 `on_receive_request`
  （`NewSessionRequest`），async 闭包 move 捕获 `service` 并每次 `Arc::clone` 供多次调用；移除原
  `let _ = &service;` 占位。模块级 rustdoc 更新为 M1-4（`initialize` / `session/new` / `authenticate`）。
- **测试**：
  - 纯函数单测 `new_session_request_carries_cwd_and_defaults`（`map.rs`）：`cwd` 承载 + 默认 provider/model/
    tool_profile/routing。
  - e2e（`tests/e2e.rs`）：`FakeService` 扩展为录制型（`recorded_config: Arc<Mutex<Option<SessionConfig>>>`，
    `create_session` 记录 config 并回固定 `SESSION_UUID`）；新增
    `session_new_round_trips_over_in_memory_pipe`：经 acp client 走 `initialize → session/new`，断言 fake 记录的
    `config.cwd == Some(req.cwd)`，且回的 ACP `SessionId` 经 `acp_session_id_to_mag` 解回同一 mag `SessionId`。
- **验证（全绿）**：1) `cargo fmt --all -- --check` OK；2) `cargo test -p mag-acp`（map 4 passed + e2e 2 passed，
  含 `session_new_round_trips_over_in_memory_pipe`，均 <0.01s）；3) `cargo clippy --all-targets -- -D warnings`
  无警告；4) `cargo test --workspace` 全通过（0 failed）；5) `cargo doc --no-deps --workspace` 无警告。
- **依赖边界**：只改 mag-acp（依赖 `mag-service` + `agent-client-protocol`）；未依赖 mag-core/agent-lib。
  显式依赖 M1-3（`SessionConfig.cwd`，已 `[DONE]`）；ACP `cwd` 经 `SessionConfig.cwd` 承载，未在 mag-acp 侧丢弃。

### [DONE] M1-R Review：M1 crate 骨架 + `initialize` + `session/new`

**上下文**：核对 M1 对 `docs/ACP.md` §1/§2/§3.1/§3.2 的实现完整性与正确性；这是真实任务，不得跳过。

**做什么**：

1. 对照 `docs/ACP.md` §1 方法↔类型表：确认 `initialize` / `session/new` 的请求/响应类型、handler 注册方式
   （builder + `on_receive_request!()` 宏）、`connect_to` run loop 均如实实现。
2. 核对依赖边界：`mag-acp` 不依赖 `mag-core`/`agent-lib`；`mag` bin 是唯一装配点。
3. 核对能力宣告保守性（§3.1/§7）：未实现的位一律未打开。
4. 核对 **M1-3 契约缺口修复**：ACP `cwd` 经 `SessionConfig.cwd`（向后兼容 `#[serde(default)]`）承载并在
   `SessionDriver::new` 落位为 facade `Agent` 的 worktree（`docs/ACP.md` §3.2/§6）；cwd 未在 mag-acp 侧被丢弃。
5. 确认无未调度失败测试；跑完整验证序列 1–5 并记录结果。
6. 汇总 M1 遗留缺口（若有 acp crate / 契约缺口已插前置任务，列出依赖关系；含 M1-3 → M1-4 依赖）。

**验证条件**：

- 完整验证序列 1–5 全绿；无未调度失败测试。
- review 结论写入本任务完成记录（对照表 + 缺口汇总 + 验证结果）。

**完成记录（M1-R）**：

对照 `docs/ACP.md` §1/§2/§3.1/§3.2 逐条核对 M1 成果，结论：M1 实现如实、完整、依赖边界正确。

1. **§1 方法↔类型表 & handler 注册**（`crates/mag-acp/src/lib.rs` `serve` + `src/handlers.rs`）：
   - `initialize`：`InitializeRequest → InitializeResponse`，经
     `Agent.builder().on_receive_request(closure, on_receive_request!())` 注册；回传
     `InitializeResponse::new(request.protocol_version)`（如实版本协商，§7）+
     `agent_capabilities(map::agent_capabilities())`。✔
   - `session/new`：`NewSessionRequest → NewSessionResponse`，同宏注册；handler 经
     `map::new_session_request_to_config` → `create_session` → `map::mag_session_id_to_acp` →
     `NewSessionResponse::new(acp_session_id)`；`ServiceError` 经 `Error::into_internal_error`
     转协议错误。✔
   - `authenticate`：`AuthenticateRequest → AuthenticateResponse`，注册为占位返回空成功响应
     （§3.1：mag 本地单用户、凭据由 `CredentialStore` 管，`auth_methods` 留空；handler 仅为鲁棒性
     兜底探测型 client）。✔
   - run loop：`.connect_to(transport).await`（server-only 常驻 run loop，无手写 spawn/loop）；
     bin 用 `Stdio::new()`，测试用 `Channel::duplex()` 内存管道。✔
2. **依赖边界**（`crates/mag-acp/Cargo.toml`）：仅依赖 `mag-service` + `agent-client-protocol`；
   dev-deps 为 `async-trait`/`futures`/`tokio`。**无** `mag-core` / `agent-lib` / tauri / axum。
   唯一装配点是 `mag` bin（`crates/mag/src/main.rs`，同时依赖 `mag-core` + `mag-acp`，构造
   `Arc<dyn MagService> = Arc::new(Engine::new())` 注入 `serve`）。✔
3. **能力宣告保守性**（§3.1/§7，`map::agent_capabilities`）：`load_session=false`（M4-2 收口）、
   `prompt_capabilities.image/audio/embedded_context` 全 `false`、`auth_methods` 留空、
   `mcp/session/auth` 取保守默认（未打开未实现位）。单测
   `agent_capabilities_are_conservative` + e2e `initialize_round_trips_over_in_memory_pipe`
   断言 client 观测到的能力与纯函数一致。✔
4. **M1-3 契约缺口修复核对**（cwd 未被丢弃）：
   - `mag_service::SessionConfig.cwd: Option<PathBuf>`（`#[serde(default, skip_serializing_if]`
     向后兼容；`crates/mag-service/src/lib.rs:293`），serde 测试证明旧快照无 `cwd` 键反序列化为
     `None` 且不回写 null 键。✔
   - `map::new_session_request_to_config` 以 `cwd: Some(req.cwd.clone())` 承载 ACP 绝对 cwd。✔
   - `mag_core::SessionDriver::new` 在 `config.cwd = Some(p)` 时
     `.worktree(WorktreeRef::new(p))`，`None` 保 facade 默认 `"."`（`driver.rs:94`）；
     driver 测试经快照 `agent_state.spec.worktree` 断言路径落位。✔
   - e2e `session_new_round_trips_over_in_memory_pipe` 端到端证明：fake service 记录的
     `config.cwd == 请求 cwd`，且回传 ACP `SessionId` 经 `acp_session_id_to_mag` 解回同一 mag id。✔
   - 依赖链：M1-3（trunk 加字段）→ M1-4（handler 承载 cwd）均 `[DONE]`，顺序正确。
5. **本次微修**：`serve` 的 rustdoc 原漏列 `session/new`（M1-4 加了 handler 但函数级 doc 未同步），
   已更正为「`initialize` 和 `session/new`，外加占位 `authenticate`」，消除 doc 与实现的不一致。
   仅注释改动，不影响编译产物。
6. **验证结果**（完整序列 1–5 全绿，无未调度失败测试）：
   1. `cargo fmt --all -- --check` → 干净。
   2. 聚焦 `cargo test -p mag-acp` → 4 unit + 2 e2e = 6 passed，0 failed，0 ignored。
   3. `cargo clippy --all-targets -- -D warnings` → 无警告。
   4. `cargo test --workspace` → 97 passed（mag 0 / mag-acp 4+2 / mag-core 48+2 / mag-service 12 /
      mag-sources 10 / mag-tools 6+13），0 failed，0 ignored。
   5. `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace` → 无缺 doc 警告。

**M1 遗留缺口汇总**：无阻塞性缺口。M1 范围（crate 骨架 + `initialize` + `session/new` + stdio 跑通）
已完整落地；acp crate 无缺 API（`Channel::duplex` 内存传输、`on_receive_request!()` 宏、
`InitializeResponse`/`NewSessionResponse` 构造子均可用），未插入任何前置任务。已按设计延后但已显式
调度的项：`load_session` 能力宣告收口（M4-2）、`session/prompt` 泵与类型映射（M2）、审批桥接（M3）、
cancel/load（M4）。M1-3→M1-4 依赖已在各自完成记录标注。

---

## Milestone M2 — `session/prompt` 泵 + 类型映射

目标：落地 mag-acp 最关键的一段——把 ACP 的 `session/prompt`**请求**与 mag 的**异步事件流**对接的"泵"，
以及支撑它的 `ServiceEvent → SessionUpdate` / `ContentBlock → UserInput` / stop reason 纯函数映射。
对应 `docs/ACP.md` §3.4 / §4。

### [DONE] M2-1 `map`：`ServiceEvent → SessionUpdate` + `ContentBlock → UserInput` + stop reason

**上下文**：

- 纯函数映射（`docs/ACP.md` §4），全无 IO，集中在 `map` 模块，便于离线单测。映射表：
  - `ServiceEvent::TextDelta{text}` → `SessionUpdate::AgentMessageChunk(text.into())`。
  - `ServiceEvent::ToolStarted{trace}` → `SessionUpdate::ToolCall(map_tool_call(trace))`。
  - `ServiceEvent::ToolFinished{trace}` → `SessionUpdate::ToolCallUpdate(..)`（终态 + 结果，用
    `trace.status:ToolStatusWire`/`output`/`message` 填）。
  - `ServiceEvent::DelegationStarted/Progress/Finished/Failed` → `ToolCall`/`ToolCallUpdate`（把委派表示为
    工具；第一版**不**臆造 `Plan` 语义，见 `PLAN.md` R-4），或降级为一条 `AgentMessageChunk` 文本。
  - `InteractionRequested` → **不**走 update（M3 的 `session/request_permission`）。
  - `RunFinished`/`RunError` → **不**发 update（决定 stop reason，见下）。
  - 未知/未支持的 mag 事件 → 保守降级为一条 `AgentMessageChunk` 文本或忽略（记日志），不臆造 ACP 语义
    （`ServiceEvent` 是 `#[non_exhaustive]`）。
- **UserInput**（`ContentBlock → UserInput`）：`Vec<ContentBlock>` → mag `UserInput`。第一版取
  `ContentBlock::Text` 拼接成 `UserInput::text(..)`；`Image`/`Audio`/`ResourceLink`/`Resource` 仅当 §3.1 宣告
  对应 `prompt_capabilities` 才接受——第一版能力未宣告，故按能力协商它们不会到来（映射时忽略或保守）。
- **stop reason**（`docs/ACP.md` §3.4）：正常结束→`StopReason::EndTurn`；被 cancel→`Cancelled`（M4）；
  模型拒绝/错误→`Refusal`；达 loop/token 上限→`MaxTokens`/`MaxTurnRequests`（若 `ServiceEvent` 能区分，否则
  归 `EndTurn`/`Refusal`，见 `PLAN.md` R-5）。

**做什么**：

1. `map`：`service_event_to_session_update(&ServiceEvent) -> Option<SessionUpdate>`（`None` = 不产 update，
   如 `InteractionRequested`/`RunFinished`/`RunError`）；辅助 `map_tool_call(&ToolTrace) -> ToolCall`、
   `map_tool_call_update(&ToolTrace) -> ToolCallUpdate`。
2. `map`：`content_blocks_to_user_input(&[ContentBlock]) -> UserInput`（第一版取 Text 拼接）。
3. `map`：`run_terminal_to_stop_reason(&ServiceEvent) -> Option<StopReason>`（`RunFinished`→`EndTurn`、
   `RunError`→`Refusal`；其余 `None`）。
4. 全部 pub 函数带 rustdoc；每条映射 + 边界（未知变体保守降级、空 ContentBlock、多 Text 拼接）单测。

**验证条件**：

- 聚焦测试：`cargo test -p mag-acp map::`，覆盖 TextDelta/Tool*/Delegation/未知事件降级、ContentBlock 拼接与
  非文本忽略、stop reason 映射。
- 完整验证序列 1–5；clippy / doc 无警告。

**完成记录**（本次调用）：

- 在 `crates/mag-acp/src/map.rs` 落地纯函数映射（全无 IO，均带 rustdoc）：
  - `service_event_to_session_update(&ServiceEvent) -> Option<SessionUpdate>`：
    `TextDelta→AgentMessageChunk`、`ToolStarted→ToolCall`、`ToolFinished→ToolCallUpdate`；
    委派表示为工具（`PLAN.md` R-4，**不**臆造 `Plan`）——`DelegationStarted→ToolCall(InProgress)`、
    `DelegationFinished→ToolCallUpdate(Completed)`、`DelegationFailed→ToolCallUpdate(Failed)`、
    `DelegationMessage→AgentMessageChunk`（降级为文本）；`InteractionRequested`/`RunFinished`/`RunError`/
    `SessionCreated`/`RunStarted`/`LocalAgentsProbed` + 未来变体 `_` → `None`。
  - `pub map_tool_call(&ToolTrace)->ToolCall` / `pub map_tool_call_update(&ToolTrace)->ToolCallUpdate`
    （call_id→ToolCallId、name→title、input→raw_input、output→raw_output、message→content 文本；
    私有 `tool_status_to_acp`：`Started→InProgress`、`Finished→Completed`、`Denied/Cancelled/Failed→Failed`、
    未来变体→`InProgress`）。委派无 call_id，用 `delegate:{delegate}` 命名空间 id（与真实 UUID 不冲突）。
  - `content_blocks_to_user_input(&[ContentBlock])->UserInput`：仅取 `Text` 换行拼接；
    `Image`/`Audio`/`ResourceLink`/`Resource` 及未来变体忽略（能力未宣告，按协商不会到来）。
  - `run_terminal_to_stop_reason(&ServiceEvent)->Option<StopReason>`：`RunFinished→EndTurn`、
    `RunError→Refusal`、其余 `None`（`MaxTokens`/`MaxTurnRequests` 需 service 侧区分，`PLAN.md` R-5，本版不猜）。
- `crates/mag-acp/Cargo.toml` 加 `serde_json`（dev-dependency，构造 tool input/output `Value`；仍在允许边界内）。
- 16 个 `map::` 单测覆盖：TextDelta、Tool{Started,Finished}、Denied/Cancelled/Failed 状态、
  Delegation{Started,Finished,Failed,Message}、非 update 事件→`None`、ContentBlock 多 Text 拼接+非文本忽略+空、
  stop reason 三分支。
- 验证序列 1–5 全绿：`cargo fmt --all -- --check` 干净；`cargo test -p mag-acp map::` 16 passed；
  `cargo clippy --all-targets -- -D warnings` 无警告；`cargo test --workspace` 全绿（mag-acp 16 + 其余 93，0 fail）；
  `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace` 无警告（修正 public→private intra-doc link）。
- 无阻塞缺口、无 workaround、未插前置任务；未映射变体保守降级，无臆造 ACP 语义。
- 下一个未完成任务：M2-2（`session/prompt` handler 泵）。

### [DONE] M2-2 `session/prompt` handler 的泵（`subscribe` → `send_message` → loop → `PromptResponse`）

**上下文**：

- mag-acp 最关键的一段（`docs/ACP.md` §3.4）：ACP 的 `session/prompt` 是一个**请求**，handler 必须在其内部
  运行到本轮 turn 结束、返回 `PromptResponse{stop_reason}`；而 mag 的一轮是**异步事件流**。泵的结构：
  1. `let sid = acp_session_id_to_mag(&req.session_id)?;`
  2. `let input = content_blocks_to_user_input(&req.prompt);`（M2-1）
  3. **先 `subscribe` 再 `send_message`**（防竞态丢事件）：`let mut events = service.subscribe(Some(sid));`
     然后 `service.send_message(sid, input).await?;`
  4. 泵 loop：`events.next().await` → 用 M2-1 映射产 `session/update`（`cx.send_notification(
     SessionNotification::new(req.session_id.clone(), update))?`）；`InteractionRequested` 本任务先占位
     （M3 接 `bridge_permission`）；`RunFinished`→`break EndTurn`；`RunError`→`break Refusal`；流结束
     （`None`）→`break EndTurn`。
  5. `responder.respond(PromptResponse::new(stop))`。
- 泵**只针对本轮**：靠 `RunFinished`/`RunError` 跳出。多个并发 prompt（不同会话）各自泵各自的流
  （`subscribe(Some(sid))` 按会话过滤），互不干扰。

**做什么**：

1. handler：`session/prompt` request handler，按上文结构实现泵；`InteractionRequested` 分支本任务留 TODO
   占位（记日志或忽略，M3 接管），其余 `ServiceEvent` 走 M2-1 映射。
2. handler 级测试：注入 scripted `Arc<dyn MagService>`（脚本化 `subscribe` 事件流：TextDelta×N →
   ToolStarted → ToolFinished → RunFinished；并记录 `send_message` 调用），驱动 `session/prompt` handler，
   断言：泵出的 `session/update` 序列与脚本一致、`RunFinished` 使 handler 返回 `EndTurn`、`send_message` 在
   `subscribe` 之后被调用。
3. 覆盖 `RunError → Refusal`、空流 → `EndTurn` 两条边界。

**验证条件**：

- 聚焦测试：`cargo test -p mag-acp prompt`（handler 级泵，scripted service），1 分钟内绿。
- 完整验证序列 1–5；clippy / doc 无警告。

**完成记录**（本次调用）：

- `crates/mag-acp/src/handlers.rs` 新增 `session_prompt(service, req, responder, cx)` handler，按
  `docs/ACP.md` §3.4 泵伪码实现：
  1. `map::acp_session_id_to_mag(&req.session_id)` —— 解析失败经
     `Error::into_internal_error` 回客户端内部错误（沿用 `session_new` 的错误约定）。
  2. `map::content_blocks_to_user_input(&req.prompt)`（M2-1）。
  3. **先 `subscribe(Some(sid))` 再 `send_message`**（防竞态丢事件）；`send_message` 失败同样回内部错误。
  4. 泵 loop（`futures::StreamExt::next`）：`None`（流结束）→ `break EndTurn`；
     `map::run_terminal_to_stop_reason(&ev)` = `Some(stop)` → `break stop`
     （`RunFinished→EndTurn`、`RunError→Refusal`）；`InteractionRequested` → 显式占位 `continue`
     （注释标 `TODO(M3)`，M3 接 `bridge_permission`，本任务不臆造权限语义）；其余事件经
     `map::service_event_to_session_update(&ev)`，`Some(update)` 时
     `cx.send_notification(SessionNotification::new(req.session_id.clone(), update))?`。
  5. `responder.respond(PromptResponse::new(stop))`。
- `crates/mag-acp/src/lib.rs`：注册 `session/prompt` handler（`service` 拆成 `service_for_new` /
  `service_for_prompt` 两份 `Arc`，各自 move 进闭包并按调用 `Arc::clone`）；更新 crate 级 & `serve` rustdoc
  反映 M2-2。
- `crates/mag-acp/Cargo.toml`：`futures` 由 dev-dependency 提升为常规依赖（handler 需 `StreamExt::next()`；
  `subscribe` 的 `BoxStream` 本就来自 `futures`，仍在 `mag-service` + `agent-client-protocol` + `futures`
  依赖边界内，未引入 mag-core/agent-lib）。
- `crates/mag-acp/tests/prompt.rs`（新测试模块，与 M1 的 `e2e.rs` 分离）：`ScriptedService` 脚本化
  `subscribe` 事件流 + 记录 `subscribe`/`send_message` 调用顺序 + 记录映射后的 `UserInput`；经真实 ACP client
  over in-memory pipe 驱动 `session/prompt`，客户端注册 `on_receive_notification::<SessionNotification>` 收集
  `session/update`。3 个测试：
  - `prompt_pump_streams_updates_and_ends_on_run_finished`：TextDelta×2 → ToolStarted → ToolFinished →
    RunFinished ⇒ 4 条 update 序列一致（AgentMessageChunk×2 + ToolCall(InProgress) + ToolCallUpdate(Completed)）、
    stop=`EndTurn`、`call_log==["subscribe","send_message"]`（subscribe 先于 send_message）、
    recorded input==`UserInput::text("please read")`。
  - `prompt_pump_maps_run_error_to_refusal`：TextDelta → RunError ⇒ 1 条 update、stop=`Refusal`。
  - `prompt_pump_treats_empty_stream_as_end_turn`：空流 ⇒ 0 update、stop=`EndTurn`、仍 subscribe 先于 send_message。
  （测试名带 `prompt_` 前缀，故 `cargo test -p mag-acp prompt` 按名过滤即可命中。）
- 验证序列 1–5 全绿：`cargo fmt --all -- --check` 干净；`cargo clippy --all-targets -- -D warnings` 无警告；
  `cargo test -p mag-acp prompt` 3 passed（<0.01s）；`cargo test --workspace` 全绿（mag-acp 16+2+3=21，
  workspace 合计 112，0 fail）；`RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace` 无警告。
- 无阻塞缺口、无 workaround、未插前置任务；`InteractionRequested` 仅显式占位（M2-1 映射本就返回 `None`，
  M3 将接管），无臆造 ACP 语义。
- 下一个未完成任务：M2-R（Review：M2 泵 + 类型映射）。

### [DONE] M2-R Review：M2 泵 + 类型映射

**上下文**：核对 M2 对 `docs/ACP.md` §3.4/§4 的完整性；真实任务，不得跳过。

**做什么**：

1. 对照 §3.4 泵伪码：确认 `subscribe`→`send_message` 顺序、每类 `ServiceEvent` 的处理、stop reason 跳出逻辑。
2. 对照 §4 映射表：确认每个已映射 `ServiceEvent` 变体正确、未映射变体保守降级、无臆造 ACP 语义。
3. 确认并发多会话泵互不干扰的假设成立（`subscribe(Some(sid))` 过滤）。
4. 确认无未调度失败测试；跑完整验证序列 1–5。

**验证条件**：完整验证序列 1–5 全绿；review 结论 + 对照表 + 缺口汇总写入完成记录。

**完成记录**（本次调用）：

- **结论**：M2 实现（`crates/mag-acp/src/{map.rs,handlers.rs,lib.rs}` + `tests/prompt.rs`）与
  `docs/ACP.md` §3.4/§4 一致。无需改代码——review 逐条核实源码 vs 规范 vs `mag-service` 真相定义
  （`crates/mag-service/src/{service.rs,lib.rs}`），未发现未调度失败测试、workaround 或臆造 ACP 语义。

- **§3.4 泵伪码对照**（`handlers.rs::session_prompt`）：
  | 伪码要点 | 实现 | 结论 |
  |---|---|---|
  | 先 `subscribe(Some(sid))` 再 `send_message`（防竞态） | line 123 subscribe 早于 line 124 send_message | ✓ |
  | `content_blocks_to_user_input(req.prompt)` | line 119 | ✓ |
  | `TextDelta/Tool*/Delegation*` → `session/update` | 经 `service_event_to_session_update` + `send_notification`（line 150-153） | ✓ |
  | `InteractionRequested` → 桥接权限 | 占位 `continue` + `TODO(M3)`（line 143-146），**已由 M3-1/M3-2 调度** | ✓（已调度） |
  | `RunFinished`→`EndTurn`、`RunError`→`Refusal` | `run_terminal_to_stop_reason` 跳出（line 137-139） | ✓ |
  | 流结束 `None`→`EndTurn` | line 130-134 | ✓ |
  | `responder.respond(PromptResponse::new(stop))` | line 156 | ✓ |
  - 错误处理**优于**伪码裸 `?`：无效 session id / `send_message` 失败经
    `respond_with_error(Error::into_internal_error(..))` 正确回客户端（line 112-127），而非不回响应即失败。

- **§4 映射表对照**（`map.rs`；ServiceEvent 实际 13 变体，`service.rs:186-279`，全覆盖）：
  | `ServiceEvent` | 映射 | 核实 |
  |---|---|---|
  | `TextDelta` | `AgentMessageChunk` | ✓ |
  | `ToolStarted` | `ToolCall`（call_id/name/status/raw_input） | ✓ |
  | `ToolFinished` | `ToolCallUpdate`（终态+raw_output+message→content） | ✓ |
  | `DelegationStarted` | `ToolCall(InProgress)`，id=`delegate:{name}`（与 UUID call_id 不冲突） | ✓ |
  | `DelegationFinished` | `ToolCallUpdate(Completed)`（output→content） | ✓ |
  | `DelegationFailed` | `ToolCallUpdate(Failed)`（message→content） | ✓ |
  | `DelegationMessage` | `AgentMessageChunk`（降级文本，不臆造 Plan，PLAN.md R-4） | ✓ |
  | `InteractionRequested`/`RunFinished`/`RunError`/`SessionCreated`/`RunStarted`/`LocalAgentsProbed` | `None` | ✓ |
  | 未来变体 `_`（`#[non_exhaustive]`） | `None`（保守忽略，不臆造） | ✓ |
  - §4 表中的 `DelegationProgress` 在真实 `ServiceEvent` 中**不存在**（仅规范表的假设变体），无需映射——非缺口。
  - 字段访问全部与源定义一致：`ToolTrace{call_id,name,input,output,status,message}`、
    `DelegationTrace{delegate,task,output,message}`、`DelegationMessageWire{text}`（`lib.rs:351-418`）。
  - `ToolStatusWire` 5 变体全覆盖：`Started→InProgress`、`Finished→Completed`、
    `Denied/Cancelled/Failed→Failed`、未来变体 `_→InProgress`（最不武断的非终态，不伪造终态）。
  - `content_blocks_to_user_input`：仅 `Text` 换行拼接，`Image/Audio/ResourceLink/Resource` + 未来变体忽略
    （能力未宣告，按协商不会到来）；空切片→空 `UserInput`。
  - `run_terminal_to_stop_reason`：仅 `RunFinished→EndTurn`、`RunError→Refusal`，其余 `None`；
    `MaxTokens`/`MaxTurnRequests` 需 service 侧区分，规范内保守留白（PLAN.md R-5）。

- **并发多会话互不干扰**：handler 传 `subscribe(Some(session_id))` 按会话过滤（`handlers.rs:123`），
  假设成立（依赖 mag-service 契约）；每个 prompt 泵各自订阅、各自靠本轮终态跳出，break 后 `events` 流被 drop。

- **缺口汇总**（均**已调度**，无未调度项）：
  1. `InteractionRequested` 仅占位 `continue`：真实 service 下会使本轮在 gate 暂停、泵在 `events.next()` 等待，
     直到 M3 `bridge_permission` 接管——已由 **M3-1/M3-2** 显式调度（line 606/641）。M2 测试无该事件故不卡住。
  2. `MaxTokens`/`MaxTurnRequests` 未产出——需 service 侧新增区分信号，PLAN.md R-5 已记为后续。

- **验证序列 1–5 全绿**：`cargo fmt --all -- --check` 干净；`cargo test -p mag-acp map::` 16 passed +
  `cargo test -p mag-acp prompt` 3 passed；`cargo clippy --all-targets -- -D warnings` 无警告；
  `cargo test --workspace` 全绿（合计 112：mag-acp 16+2+3=21、mag-core 48+2=50、mag-service 12、
  mag-sources 10、mag-tools 6+13=19，0 fail）；`RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace` 无警告。

- 本任务纯 review，无代码改动；仅本文件 + `memory/claude_plan.md` 更新。PLAN.md 不改（无阶段计划变更）。
- 下一个未完成任务：M3-1（`map`：`InteractionKindWire → RequestPermissionRequest` + outcome）。

---

## Milestone M3 — 审批桥接（`InteractionRequested` ↔ `session/request_permission`）

目标：把 mag 的审批往返（interface 无关的 `InteractionRequested` / `respond_interaction`）映射到 ACP 的
`session/request_permission`，复用同一 gate，审批逻辑（mag-core）一行不改。对应 `docs/ACP.md` §5。

### [DONE] M3-1 `map`：`InteractionKindWire → RequestPermissionRequest` + outcome → `InteractionResponseWire`

**上下文**：

- 纯函数映射（`docs/ACP.md` §5「映射细节」），无 IO。
- **options**：mag 审批语义（approve/deny）→ `Vec<PermissionOption>`：至少 `AllowOnce`(approve) 与
  `RejectOnce`(deny)；若 mag 支持"始终允许某工具"，加 `AllowAlways`/`RejectAlways`。每个 option 带稳定
  `PermissionOptionId`，`kind` 用 `PermissionOptionKind::{AllowOnce,AllowAlways,RejectOnce,RejectAlways}`。
- **tool_call**：`RequestPermissionRequest.tool_call: ToolCallUpdate` 用 mag 富化的审批信息填充——
  `InteractionKindWire::Approval{call_id, requirement}` 与 `Permission{action_id,actor,category,risk,summary,
  subject,reason}` 携带的字段（工具名/理由/输入摘要）让 client 渲染有意义的权限框。
- **outcome → response**（`RequestPermissionOutcome`）：
  - `Selected{option_id}` → 按 option_id 判 approve/deny，组 `InteractionResponseWire::Approval{step_id,
    call_id, decision:ApprovalDecisionWire::{Approve|Deny}, message}`（或对 `Permission` kind 组
    `InteractionResponseWire::Permission{action_id, decision:PermissionDecisionWire::{Approve|Deny{reason}}}`）。
  - `Cancelled` → 保守映射为 deny/cancel（`ApprovalDecisionWire::Cancel` 或 `Deny`；`PermissionDecisionWire::
    Cancel`），唤醒 driver 以取消收尾。
- `InteractionKind::Permission`（本地 agent / 特权动作）与工具审批 `Approval` 走**同一** `session/
  request_permission` 通道，只是 tool_call/options 的填充来源不同（`docs/ACP.md` §5）。

**做什么**：

1. `map`：`interaction_to_permission_request(sid: &acp::SessionId, kind: &InteractionKindWire) ->
   RequestPermissionRequest`（组 tool_call + options + 稳定 option ids），带 rustdoc + 单测（覆盖 `Approval`
   与 `Permission` 两支）。
2. `map`：`outcome_to_interaction_response(kind: &InteractionKindWire, outcome: RequestPermissionOutcome) ->
   InteractionResponseWire`，含 `Selected`(approve/deny) 与 `Cancelled`(保守 deny/cancel) 分支，带单测。
3. 用稳定常量或枚举定义 `PermissionOptionId` 与 approve/deny 的对应，避免魔法字符串漂移。

**验证条件**：

- 聚焦测试：`cargo test -p mag-acp permission_map`（或 `map::` 过滤），覆盖 Approval/Permission 两 kind 的
  request 组装、Selected(approve/deny)/Cancelled 三种 outcome 回译。
- 完整验证序列 1–5；clippy / doc 无警告。

**完成记录**（本次调用）：

- **实现**（纯函数，`crates/mag-acp/src/map.rs`，均带 rustdoc，`#![warn(missing_docs)]` 无警告）：
  - `pub fn interaction_to_permission_request(sid: &acp::SessionId, kind: &InteractionKindWire) ->
    acp::RequestPermissionRequest`：组 `tool_call` + 固定 options。
  - `pub fn outcome_to_interaction_response(kind: &InteractionKindWire, outcome:
    acp::RequestPermissionOutcome) -> InteractionResponseWire`：Selected/Cancelled 回译。
  - 稳定 option id 常量（避免魔法串漂移）：`pub const PERMISSION_OPTION_ALLOW="mag:allow"` /
    `PERMISSION_OPTION_REJECT="mag:reject"`；私有 helper：`permission_options`、`option_id_to_choice`、
    `permission_category_to_tool_kind`、`approval_tool_call`、`permission_tool_call`、`interaction_tool_call`、
    `outcome_to_decision`、`decision_to_approval`、`decision_to_permission`、`placeholder_step_id`。
- **options**：只给一次性 `AllowOnce`(approve)+`RejectOnce`(deny)。mag wire 无「始终允许」持久语义，
  故不宣告 `AllowAlways`/`RejectAlways`（如实宣告，`docs/ACP.md` §7）；每 option 带稳定 `PermissionOptionId`。
- **tool_call 富化**（`RequestPermissionRequest.tool_call: ToolCallUpdate`）：
  - `Approval{call_id, requirement}` → id=call_id（对齐已流式的 `ToolCall`）、status=Pending、通用 title、
    `requirement` 的 reason(若有)入 content。冻结 `Approval` 变体**只有** call_id+requirement（无 tool_name/input），
    只用现有字段，不臆造（§5 提到的 tool_name/input 尚未进 wire 类型）。
  - `Permission{action_id,category,summary,subject,reason,..}` → id=action_id、status=Pending、
    kind=category→ToolKind（Shell→Execute 等，未知/未来→Other）、title=summary、raw_input=subject、reason 入 content。
  - `Approval`/`Permission` 走**同一** `session/request_permission` 通道，仅富化来源不同。
- **outcome→response**：先归约 `Decision{Approve,Deny,Cancel}`（Selected(allow)→Approve、Selected(其它/未知 id)→
  Deny（fail-safe，未知 id 绝不批准，§6）、Cancelled/未来 outcome→Cancel），再按 kind 组**家族匹配**的 response
  （mag-core `interaction_response_from_wire` 要求家族一致，否则 Err）：Approval→`InteractionResponseWire::Approval`
  （Cancel 带 model-visible message；step_id/call_id 是占位——mag 从存储 interaction 重建，回显真实 call_id、
  step_id 用 `StepIdWire::new(*call_id.as_uuid())` 复用 UUID 作惰性占位，避免引入 `uuid` 依赖破坏边界）、
  Permission→`InteractionResponseWire::Permission`（keyed by action_id）。
- **mag-unused 家族**：`Question`/`Choice` 在 mag-core 注释为 facade 不产出（`engine/approval.rs:243`），
  故 request 用通用 pending box 保 total、response 用惰性默认（`Answer("")`/`Choice{index:0}`，镜像 mag-core 自身
  取消处理），未来 `_` kind → 惰性 `Answer("")`。非缺口、非未调度失败：这些路径运行时不触达。
- **依赖边界**：`serde_json` 由 dev-dep 提升为 dep（边界明确允许 serde/serde_json）；仍只依赖
  `mag-service` + `agent-client-protocol`(+serde_json/futures)，未碰 mag-core/agent-lib/tauri/axum。冻结 `MagService`
  契约一字未改。
- **验证序列 1–5 全绿**：`cargo fmt --all -- --check` 干净；`cargo test -p mag-acp map::` 22 passed（16 旧+6 新：
  options 稳定性、Approval/Permission request 组装、Approval/Permission 的 Selected(approve/deny)/Cancelled 回译、
  未知 option id fail-safe deny）；`cargo clippy --all-targets -- -D warnings` 无警告；`cargo test --workspace`
  全绿（118：mag-acp 22+2+3=27、mag-core 48+2、mag-service 12、mag-sources 10、mag-tools 6+13，0 fail）；
  `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace` 无警告。
- PLAN.md 不改（无阶段计划变更）。下一个未完成任务：M3-2（`bridge_permission` 接入泵）。

### [DONE] M3-2 `bridge_permission` 接入泵

**上下文**：

- 把 M2-2 泵里 `InteractionRequested` 的占位换成真正的桥接（`docs/ACP.md` §5 伪码）：
  1. `let req = interaction_to_permission_request(&acp_sid, &kind);`（M3-1）
  2. `let resp = cx.send_request(req).block_task().await?;`（向 client 发 `session/request_permission` 并
     **await** 用户决定——异步暂停点）
  3. `let mag_resp = outcome_to_interaction_response(&kind, resp.outcome);`（M3-1）
  4. `service.respond_interaction(sid, request_id, mag_resp).await?;`（唤醒 service 侧被暂停的 driver）
- **审批是异步暂停点**（`docs/ACP.md` §5、`PLAN.md` 约束）：必须真正 `await`；测试须断言 driver 在 client
  回 outcome 前不前进。cancel 期间的挂起权限收尾在 M4-1 处理。

**做什么**：

1. 实现 `bridge_permission(service, cx, sid, acp_sid, request_id, kind)`，接入 M2-2 泵的
   `InteractionRequested` 分支。
2. handler 级测试：scripted service 脚本化 `subscribe` 流在中途产 `InteractionRequested`；用一个可控的 fake
   ACP client（内存管道）在收到 `session/request_permission` 后回 `Selected{approve}` / `Selected{deny}` /
   `Cancelled`，断言：driver 在 client 回 outcome 前不前进（暂停语义）、outcome 正确回灌
   `respond_interaction`、三条路径都覆盖。

**验证条件**：

- 聚焦测试：`cargo test -p mag-acp permission_bridge`（approve/deny/cancel 三路径 + 暂停语义），1 分钟内绿。
- 完整验证序列 1–5；clippy / doc 无警告。

**完成记录**（本次调用）：

- **实现**（`crates/mag-acp/src/handlers.rs`，均带 rustdoc）：
  - `async fn bridge_permission(service,&connection,session_id,acp_sid,request_id,kind)`：按 `docs/ACP.md`
    §5 伪码——`map::interaction_to_permission_request` 组请求 → `connection.send_request(req).block_task().await?`
    **await** client 决定 → `map::outcome_to_interaction_response(&kind, resp.outcome)` 回译 →
    `service.respond_interaction(sid, request_id, mag_resp).await`（`ServiceError`→internal_error）唤醒暂停 driver。
  - 泵 `InteractionRequested` 分支：占位 `continue` 换为解构 `{request_id,kind,..}` 调 `bridge_permission`；
    失败经 `responder.respond_with_error` 收尾。Approval/Permission 走**同一**通道（§5/§6），未在任何路径绕过审批。
- **关键修复（阻塞并入本任务，非绕过）**：ACP handler 回调跑在**单任务 event loop** 上并阻塞其继续收报文
  （acp `Builder` 文档 line 586；`SentRequest::block_task` 明确警告「在 handler 内直接用会死锁连接」）。M2-2
  的 `session/prompt` 泵原是**内联**在 handler 里跑——只发 notification（fire-and-forget）时无碍，但审批需
  **收**入站 `session/request_permission` 响应，内联 `block_task().await` 会死锁（event loop 被泵占住，无法投递
  响应）。修复：`session_prompt` 校验 session id 后把整泵 `connection.spawn(..)` 到 event loop **之外**运行
  （延迟经 `responder` 应答 prompt），泵抽出为 `run_prompt_pump`。这是审批往返正确落地的前提，故并入本任务而非
  papering over；同时为 M4（prompt 期间收 `session/cancel`）预留了空闲 event loop。M2 三条 prompt 测试仍全绿，
  证明行为兼容。
- **handler 级测试**（`crates/mag-acp/tests/permission_bridge.rs`，3 条，multi_thread，<1s）：scripted
  `BridgeService` 的 `subscribe` 先放「before」TextDelta + `InteractionRequested`，随后**阻塞**——post 事件
  （「after」哨兵 + `RunFinished`）压在 sender 里，直到 `respond_interaction` 被调用才释放（忠实建模 mag 审批
  异步暂停）。fake ACP client 注册 `session/request_permission` 请求 handler，回 `Selected{allow}` /
  `Selected{reject}` / `Cancelled` 三路径。断言：**暂停语义**（收到权限请求时「after」哨兵尚未流出）、
  `respond_interaction` 收到家族正确的 `InteractionResponseWire::Approval`（Approve/Deny/Cancel(带 message)、
  call_id/request_id/session 对齐）、driver 被唤醒、turn 以 `EndTurn` 收尾且 before/after 更新最终都到达。
- **依赖边界**：仅用现有 `mag-service` + `agent-client-protocol`(+futures)；未碰 mag-core/agent-lib/tauri/axum；
  冻结 `MagService` 契约一字未改。
- **验证序列 1–5 全绿**：`cargo fmt --all -- --check` 干净；`cargo test -p mag-acp permission_bridge` 3 passed；
  `cargo clippy --all-targets -- -D warnings` 无警告；`cargo test --workspace` 全绿（121：mag-acp 22+2+3+3=30、
  mag-core 48+2、mag-service 12、mag-sources 10、mag-tools 6+13，0 fail）；
  `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace` 无警告。
- PLAN.md 不改（无阶段计划变更）。下一个未完成任务：M3-R（Review：M3 审批桥接）。

### [DONE] M3-R Review：M3 审批桥接

**上下文**：核对 M3 对 `docs/ACP.md` §5/§6 的完整性；真实任务，不得跳过。

**做什么**：

1. 对照 §5：确认 request 组装（options/tool_call/稳定 id）、outcome 回译、`Approval` 与 `Permission` 走同一
   通道、异步暂停语义被显式测试。
2. 对照 §6 安全：确认 shell 等特权工具审批不可省（走同一 gate），mag-acp 未在任何路径绕过审批。
3. 确认无未调度失败测试；跑完整验证序列 1–5。

**验证条件**：完整验证序列 1–5 全绿；review 结论 + 对照表 + 缺口汇总写入完成记录。

**完成记录**（本次调用）：

- **结论**：M3（M3-1 映射 + M3-2 泵接入）**忠实实现** `docs/ACP.md` §5/§6，无未调度失败测试、无绕过审批
  路径、无 papering over。就地复核了 `crates/mag-acp/src/map.rs`、`crates/mag-acp/src/handlers.rs`、
  `crates/mag-acp/tests/permission_bridge.rs` 及 `map` 单测。本任务仅复核 + 文档，未改任何编译产物。

- **§5 对照表**（要点 → 实现/证据）：

  | §5 要求 | 实现 | 证据 |
  | --- | --- | --- |
  | options：≥ `AllowOnce`(approve)+`RejectOnce`(deny)，稳定 `PermissionOptionId`，不虚报 always | `permission_options()` 只给 once-scoped allow/reject；稳定常量 `PERMISSION_OPTION_ALLOW/REJECT`；mag wire 无持久语义故**不宣告** `AllowAlways/RejectAlways`（§7 如实宣告） | `map.rs:412` + 单测 `permission_options_offer_stable_once_scoped_allow_and_reject`（断言 kind + 不含 always） |
  | tool_call 富化（Approval 支） | `approval_tool_call`：id=call_id（对齐已流式 `ToolCall`）、status=Pending、reason 入 content | `map.rs:472` + 单测 `approval_request_addresses_tool_call_and_carries_reason` |
  | tool_call 富化（Permission 支） | `permission_tool_call`：id=action_id、category→ToolKind、summary→title、subject→raw_input、reason→content | `map.rs:499` + 单测 `permission_request_enriches_tool_call_from_action` |
  | outcome→response：`Selected` 判 approve/deny | `outcome_to_decision` + `option_id_to_choice`：仅精确 `ALLOW` id→Approve，其余→Deny | 单测 `approval_outcomes_map_to_matching_decisions`、`permission_outcomes_map_to_matching_decisions` |
  | outcome→response：`Cancelled`→保守 deny/cancel | `Cancelled`（及未来 `_` outcome）→`Decision::Cancel`；Approval 组 `ApprovalDecisionWire::Cancel`（带 model-visible message）、Permission 组 `PermissionDecisionWire::Cancel` | 单测同上（cancel 分支）+ handler `approval_bridge_maps_cancelled_outcome_to_cancel` |
  | `Approval` 与 `Permission` 走**同一**通道 | `interaction_to_permission_request` 对两 kind 都产 `RequestPermissionRequest`；`bridge_permission` 对 kind 泛化，仅富化来源不同 | `map.rs:562`、`handlers.rs:241` |
  | 异步暂停被**显式测试** | `bridge_permission` 真正 `connection.send_request(req).block_task().await?` 后才 `respond_interaction`；泵 `spawn` 出 event loop 避免 `block_task` 死锁 | `handlers.rs:135/252`；测试断言 `paused_before_decision`（收到权限请求时 after 哨兵尚未流出）+ `resumed` |

- **§6 安全对照**：

  | §6 要求 | 实现 | 证据 |
  | --- | --- | --- |
  | shell 等特权工具审批不可省，走同一 gate | `Approval`/`Permission` 均只经 `session/request_permission`；泵 `InteractionRequested` 分支唯一出口是 `bridge_permission`，无旁路 | `handlers.rs:181-198`（无绕过分支）；`permission_category_to_tool_kind`：`Shell→Execute` |
  | 未知/取消 fail-safe 绝不批准 | 未知 option id、`Cancelled`、未来 outcome 一律**不**映射为 Approve | 单测 `unknown_option_id_denies_fail_safe`（Deny）+ `outcome_to_decision` 默认臂 |
  | 凭据不经 ACP、不写日志 | mag-acp 不触碰凭据；仅消费 `Arc<dyn MagService>`，依赖边界内无 `CredentialStore` | 依赖边界（`Cargo.toml` 仅 `mag-service`+acp+futures+serde_json） |

- **缺口汇总**（均**非阻塞、非未调度失败**）：
  1. §5 文案设想 `Approval` 携 `tool_name`/`input` 摘要（agent-lib M7-3，见 `DESIGN.md` §9），但冻结 wire
     `InteractionKindWire::Approval` 目前**只有** `call_id + requirement`。M3-1 已如实只用现有字段、不臆造
     （`approval_tool_call` 注释明示）。待 agent-lib M7-3 富化 wire 后可回填 tool_call 字段——属**上游 wire 演进**，
     不在 mag-acp 范围内，不新增前置任务。
  2. handler 级 3 条测试只跑 `Approval` 家族；`Permission` 家族的 request 组装与三态回译由 `map` 单测覆盖
     （`bridge_permission` 对 kind 泛化，桥接逻辑与家族无关）。判定为**充分覆盖**，不补任务。
  3. §5「cancel 期间挂起权限收尾」（`await` 中收 `session/cancel`）**明确调度在 M4-1**（本节上下文与 M4-1
     标题均注明），非本任务范围、非未调度缺口。

- **验证序列 1–5 全绿**（本次独立复核，非复用旧绿）：`cargo fmt --all -- --check` 干净；聚焦
  `cargo test -p mag-acp 'map::'` 22 passed + `--test permission_bridge` 3 passed；
  `cargo clippy --all-targets -- -D warnings` 无警告；`cargo test --workspace` 全绿（121：mag-acp 22+2+3+3=30、
  mag-core 48+2、mag-service 12、mag-sources 10、mag-tools 6+13，0 fail）；
  `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace` 无警告。
- PLAN.md 不改（无阶段计划变更）。M3 里程碑全部 `[DONE]`。下一个未完成任务：M4-1（`session/cancel`
  notification + 挂起权限的 cancel 收尾）。

---

## Milestone M4 — cancel + `session/load` + 能力宣告收口

目标：接 `session/cancel` notification（含挂起权限的 cancel 收尾），接 `session/load` → `resume_session`，
并按 mag-core 恢复能力实际就绪度收口 `load_session` / `prompt` 能力宣告。对应 `docs/ACP.md` §3.5 / §3.3 / §7。

### [DONE] M4-0 agent-lib 升级适配：`RunErrorKind` / `SessionBudget` 契约扩展 + 官方取消入口（前置任务）

**上下文**：

- 这是 M4-1 的**前置契约扩展**任务（按通用规则回 service 主干**向后兼容**加字段，只加字段不改既有语义）。
  agent-lib 一批修复性更新（M4–M9）带来了两项 mag-acp 需要的主干能力：
  1. **stop reason 区分**（`PLAN.md` R-5）：`session/cancel` 后 ACP 规范**强制**返回 `StopReason::Cancelled`，
     而冻结契约里 `RunError{id,message}` 无法区分 cancel / loop 上限 / 一般错误；
  2. **per-run 预算**：agent-lib M6 暴露 `BudgetLimits`，mag 侧需要 wire 承载。
- 同时把 mag 自建的取消通道（`CancelToken` + `arm_cancel`）切换为 agent-lib 官方
  `facade::CancelHandle` / `Agent::stream_with_cancel`（M5-4），取消令牌经 `RunContext` 传播到
  `IpcApproval::fulfill`，挂起审批在取消时以保守 deny 立即收尾。
- **向后兼容硬约束**：`RunError.kind` 与 `SessionConfig.budget` 均带 `#[serde(default)]`，旧持久化/旧
  客户端反序列化为缺省值（`RunErrorKind::Other` / `None`）。

**做什么**：

1. `mag-service`：`Event`/`ServiceEvent::RunError` 加 `kind: RunErrorKind`（`#[serde(default)]`，
   `#[non_exhaustive]` 枚举 `{Other,Cancelled,LoopLimitExceeded,BudgetExhausted}`）；`SessionConfig` 加
   `budget: Option<SessionBudget>`；新增 `SessionBudget{max_steps,max_tokens,max_cost_micros,
   max_wall_time_secs}`（全 `Option<u64>`）。
2. `mag-core`：`SessionDriver::new`/`restore` 把 `SessionBudget` 映射为 `BudgetLimits` 接线到 facade
   `.budget(..)`；`run_turn` 改走 `stream_with_cancel`，从 `FacadeError` 变体映射 `RunErrorKind`；
   删除自建 `CancelToken` 与 `IpcApproval::arm_cancel`（改用 `ctx.cancellation()`）。
3. `mag-core` mutex poison 策略对齐 agent-lib M9-1：`approval.rs`/`session.rs`/`persistence.rs` 的
   `.lock().expect(..)` 统一改 `lock_recovering`（`PoisonError::into_inner`）。
4. `mag-acp`：`run_terminal_to_stop_reason` 按 `kind` 映射（`Cancelled→Cancelled`、
   `LoopLimitExceeded→MaxTurnRequests`、其余→`Refusal`）。
5. 测试：budget 耗尽与 loop 上限各一条 engine 级测试断言结构化 `kind`；重写 `cancel_while_paused` 测试走
   `stream_with_cancel`；ACP map 测试补齐四种 kind 的映射断言。

**验证条件**：

- 聚焦测试：`cargo test -p mag-core exhausted` + `cargo test -p mag-acp map::`（1 分钟内绿）。
- 完整验证序列 1–5；clippy / doc 无警告。
- 向后兼容：旧 JSON（无 `kind`/`budget` 键）反序列化为缺省值。

**完成记录（M4-0）**：

- **agent-lib 升级影响面复核**：全 workspace 仅 1 处编译错误（`ToolCall` 新增 `extra` 字段，
  `mag-tools/tests/builtin_tools.rs` 已补 `extra: Default::default()`）；deny 语义收窄（M5-2）、取消契约
  （M4-5）、解析容忍（M7）均与 mag 既有设计兼容。
- **契约扩展（向后兼容）**：`mag-service` 新增 `RunErrorKind`（`#[non_exhaustive]`，serde default=`Other`）
  并加到 `Event`/`ServiceEvent::RunError`；新增 `SessionBudget` 并加到 `SessionConfig.budget`。旧 JSON 无新键
  反序列化为缺省值，已序列化数据不回写多余键。
- **官方取消入口**：删除 mag 自建 `CancelToken`（约 50 行）与 `IpcApproval::arm_cancel`；session actor 持
  `facade::CancelHandle`，`run_turn` 走 `stream_with_cancel`；facade 将取消令牌经 `RunContext` 传播到
  `IpcApproval::fulfill`，挂起审批 select 在 `ctx.cancellation().cancelled()` 上，取消时以保守
  deny/cancel 收尾、driver 不悬挂（M4-1 的 service 侧前提已就绪）。
- **budget 接线**：`SessionBudget` → `BudgetLimits`（wall time 秒→`Duration`），`new`/`restore` 均接线；
  预算逐顶层 run 重置，超限 → `FacadeError::BudgetExhausted` → `RunErrorKind::BudgetExhausted`。
- **poison 策略**：14 处 `.lock().expect(..)` 改 `lock_recovering`（`approval.rs`×3（另有 `arm_cancel` 的
  1 处随该机制删除）、`session.rs`×4、`persistence.rs`×7），对齐 agent-lib M9-1。
- **ACP stop reason**：`run_terminal_to_stop_reason` 按 `kind` 映射——`Cancelled→StopReason::Cancelled`
  （**修复了 ACP 规范违规**：`session/cancel` 此前误归 `Refusal`）、`LoopLimitExceeded→MaxTurnRequests`、
  `BudgetExhausted→Refusal`（ACP 无预算变体）。**残留**：`MaxTokens` 仍不产出（无独立 token 上限信号）。
- **测试**：新增 `engine::chat::exhausted_budget_surfaces_structured_run_error`（1-token 预算被首轮响应击穿）
  与 `exhausted_loop_limit_surfaces_structured_run_error`（10 轮 tool-use 撞 per-turn 上限）；
  `cancel_while_paused_resolves_without_running_the_tool` 重写为 `stream_with_cancel` 路径；
  `stop_reason_maps_terminal_events_only` 补 `Cancelled`/`LoopLimitExceeded`/`BudgetExhausted` 三臂；
  `engine.rs`/`e2e_offline.rs` 的 cancel 断言加强为 `kind == Cancelled`。
- **文档同步**：`docs/ACP.md` §3.4 伪代码与 stop reason 映射表按 `RunErrorKind` 更新；`docs/DESIGN.md`
  §3.3 审批伪代码的取消令牌来源改为 `ctx.cancellation()`；`PLAN.md` 契约清单（`ServiceEvent`/
  `SessionConfig`/`RunErrorKind`/`SessionBudget`）与 R-5（标大部消解）已更新。
- **验证（全绿）**：`cargo fmt --all` 干净；`cargo clippy --workspace --all-targets` 无警告；
  `cargo test --workspace` 全通过（123：mag-acp 22+2+3+3、mag-core 50+2、mag-service 12、mag-sources 10、
  mag-tools 6+13，0 fail）。
- 下一个未完成任务：M4-1（`session/cancel` notification + 挂起权限的 cancel 收尾）。

### [DONE] M4-1 `session/cancel` notification + 挂起权限的 cancel 收尾

**依赖**：M4-0（`RunErrorKind::Cancelled` 结构化分类 + 官方取消入口，已 `[DONE]`）。

**上下文**：

- `session/cancel` 是**通知**（无响应，`docs/ACP.md` §3.5）：handler 用 `on_receive_notification(closure,
  on_receive_notification!())`，请求类型 `CancelNotification{session_id}`。收到后调
  `service.cancel(sid).await`，令该会话正在跑的一轮干净终止。
- 效果沿 M2-2 的泵传导：`cancel` 使 service 侧结束本轮，泵收到终态后 `session/prompt` handler 返回
  `PromptResponse{stop_reason: Cancelled}`（acp 规范要求 cancel 后必须返回 `Cancelled`）。
  **M4-0 已铺平识别路径**：被 cancel 的本轮以 `RunError{kind: RunErrorKind::Cancelled}` 收尾，
  `run_terminal_to_stop_reason` 已将其映射为 `StopReason::Cancelled`——本任务**无需**再设本轮 cancel
  标志，接上 notification handler 即可传导。
- **挂起权限的 cancel 收尾**（`docs/ACP.md` §3.5/§5）：若 cancel 时有挂起的 `session/request_permission`
  （M3-2 的 `await` 中），需让其以 cancel 收尾——依赖 client 回 `Cancelled` outcome，或本地放弃并
  `respond_interaction` 一个 deny/cancel，确保 service 侧 driver 不悬挂。注意 M4-0 后 service 侧挂起审批
  已能随取消自行收尾（`ctx.cancellation()`），mag-acp 侧只需保证自己的 `await` 不悬挂并避免迟到的
  `respond_interaction` 误报（service 对未知 request_id 返回 `InteractionNotFound`，桥接应容忍）。

**做什么**：

1. handler：`session/cancel` notification handler → 解析 sid → `service.cancel(sid).await`（M4-0 后泵经
   `RunErrorKind::Cancelled` 自然以 `Cancelled` 收尾，无需 cancel 标志）。
2. 挂起权限 cancel 收尾：cancel 时若有挂起 `bridge_permission`，令其 `respond_interaction` 一个保守
   deny/cancel（或依赖 client `Cancelled` outcome）；迟到的 respond 遇 `InteractionNotFound` 应静默容忍
   （M4-0 后 service 侧可能已自行收尾）。
3. handler 级测试：scripted service，run 中途收 `session/cancel`，断言本轮以 `Cancelled` 收尾（事件带
   `kind: RunErrorKind::Cancelled`）；再测 cancel 落在挂起权限期间时两侧都干净收尾（无悬挂、无死等、
   迟到 respond 不报错）。

**验证条件**：

- 聚焦测试：`cargo test -p mag-acp cancel`（本轮 Cancelled 收尾 + 挂起权限 cancel 收尾），1 分钟内绿，
  无卡死。
- 完整验证序列 1–5；clippy / doc 无警告。

**完成记录（M4-1）**：

- **`CancelTracker`（`crates/mag-acp/src/handlers.rs`）**：跨 handler 的取消信号共享状态——每会话一个
  `tokio::sync::watch::Sender<bool>`；`cancel` 翻标志并唤醒所有挂起桥接，晚到的桥接经
  `watch::Receiver::borrow` 立即观察到 `true`（`changed` 只看未来翻转）。`serve` 持一份，clone 分发给
  `session/prompt` 与 `session/cancel` handler。
- **`session/cancel` handler（`session_cancel`）**：经 `on_receive_notification` 注册（`CancelNotification`）；
  解析 sid 失败静默忽略（notification 无错误通道），先翻 tracker（唤醒挂起桥接，让其在 driver 仍停着时
  答出保守 cancel），再 `service.cancel(sid).await`（错误吞掉，cancel 是 best-effort）。M4-0 后 run 以
  `RunError{kind: Cancelled}` 收尾，泵经 `run_terminal_to_stop_reason` 自然回 `StopReason::Cancelled`，
  **未设任何本轮 cancel 标志**。
- **挂起权限 cancel 收尾（`bridge_permission`）**：`tokio::select!` 竞速 client outcome vs
  `wait_for_cancel`（tracker）；cancel 胜出时按 `RequestPermissionOutcome::Cancelled` 走既有
  `outcome_to_interaction_response` 保守回译（Approval→Cancel 带 model-visible message）。
  `respond_interaction` 遇 `ServiceError::InteractionNotFound` **静默容忍**（M4-0 后 service 侧挂起审批
  可能已随 `ctx.cancellation()` 自行收尾，迟到响应合法地找不到目标），其余错误仍上报。
- **依赖变化**：`tokio`（`sync` feature）由 dev-dependency 提升为常规依赖（`watch`/`select!`；在
  PLAN.md 允许的依赖边界内）。`mag-acp` 仍不依赖 `mag-core`/`agent-lib`。
- **测试（`crates/mag-acp/tests/cancel.rs`，2 条，均 <0.01s）**：
  - `cancel_ends_prompt_turn_with_cancelled_stop_reason`：scripted service 流一条 TextDelta 后静默，
    client 收首条 update 后发 `CancelNotification`；断言 stop=`Cancelled`、`MagService::cancel` 以正确
    sid 被调、无 interaction 被误答。
  - `cancel_during_pending_permission_wraps_up_both_sides`：service 发 `InteractionRequested` 后静默，
    fake client 收 `session/request_permission` 后**永不应答**（responder 移到 event loop 外的 pending
    task，避免阻塞 client loop 造成测试假死）；service 的 `respond_interaction` 故意回
    `InteractionNotFound` 模拟 M4-0 后的自行收尾竞态。断言：stop=`Cancelled`（无悬挂、无误报）、
    桥接仍尝试以 `ApprovalDecisionWire::Cancel` 答出保守响应、`cancel` 被调。
- **文档**：crate 级 rustdoc 与 `serve` 注册清单更新为含 `session/cancel`；`handlers.rs` 相关 rustdoc
  补齐（`CancelTracker`/`session_cancel`/`wait_for_cancel`/桥接的 cancel 分支）。
- **验证（全绿）**：1) `cargo fmt --all -- --check` 干净；2) `cargo test -p mag-acp cancel` 2 passed；
  3) `cargo clippy --all-targets -- -D warnings` 无警告；4) `cargo test --workspace` 全通过（125：
  mag-acp 22+2+3+3+2=32、mag-core 50+2、mag-service 12、mag-sources 10、mag-tools 6+13，0 fail）；
  5) `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace` 无警告。
- **未做的取舍**：tracker 的 watch channel 随会话常驻（每会话一个小 channel，无清理），注释已注明；
  ACP 规范鼓励 client 在 cancel 时自行回 `Cancelled` outcome（该路径 M3 已覆盖），本任务覆盖的是
  client 不应答的兜底。
- 下一个未完成任务：M4-2（`session/load` → `resume_session` + 能力宣告收口）。

### [DONE] M4-2 `session/load` → `resume_session` + 能力宣告收口

**上下文**：

- `session/load`（`docs/ACP.md` §3.3）：**仅当** `initialize` 宣告了 `load_session`。`LoadSessionRequest
  {session_id}` 带 ACP `SessionId`；反查 mag `SessionId`（`acp_session_id_to_mag`），调
  `service.resume_session(id).await`，回 `LoadSessionResponse`。
- **恢复能力就绪度**（`docs/ACP.md` §3.3/§7、`docs/DESIGN.md` §3.6）：需审批会话的恢复已完全可用
  （agent-lib M7-F1 已补齐 restore 的 `interaction_handler` 注入口）。`load_session` 是否宣告为 `true` 仅取决于
  mag-core 恢复能力的**实际就绪度**（service 主干 C4）——本任务据此收口 `agent_capabilities()`：若 mag-core
  `resume_session` 确可用则置 `true` 并注册 `session/load` handler；否则保持 `false` 且不注册。
- 顺带收口 `prompt_capabilities` / `session_capabilities` / `mcp_capabilities` 到与实现一致的最终值
  （`docs/ACP.md` §7 如实宣告）。

**做什么**：

1. `map`：调整 `agent_capabilities()`，`load_session` 依 mag-core 恢复就绪度取值（第一版结合 C4 现状定；若
   就绪则 `true`）；其余能力位收口到与实现一致。更新单测断言。
2. handler：`session/load` request handler → `service.resume_session(..)`，回 `LoadSessionResponse`（仅当宣告
   `load_session` 时注册）。
3. e2e：内存管道跑 `initialize`（断言 `load_session` 位）→ `session/new` → `session/load`（断言 fake service
   收到 `resume_session`）。
4. 若发现 mag-core 恢复能力不足以支撑宣告的语义（缺口），**不 papering over**：在本文件插最小前置任务
   （回主干补 C4 恢复缺口）并让本任务显式依赖它。

**验证条件**：

- 聚焦测试：`cargo test -p mag-acp session_load`（能力位断言 + load 往返），1 分钟内绿。
- 完整验证序列 1–5；clippy / doc 无警告。

**完成记录（M4-2）**：

- **恢复就绪度核实**：mag-core `resume_session`（`crates/mag-core/src/engine.rs:156`）完整实现——
  从持久层读回 config + 最新 committed snapshot、幂等（已 live 则 no-op）、重建 session actor；
  `tests/e2e_offline.rs` 有跨“进程重启”的恢复测试覆盖。判定就绪，按 `docs/ACP.md` §3.3/§7 宣告
  `load_session = true`，**无需插主干前置任务**。
- **能力宣告收口（`map::agent_capabilities`）**：`load_session` 翻为 `true`；其余位保持与实现一致的
  最终值——`prompt_capabilities.image/audio/embedded_context` 全 `false`（多模态第一版不做）、
  `mcp_capabilities`/`session_capabilities`/`auth` 保持未宣告默认、无 `auth_methods`。单测由
  `agent_capabilities_are_conservative` 更名为 `agent_capabilities_match_implementation` 并翻转断言；
  e2e `initialize_round_trips_over_in_memory_pipe` 同步断言 `load_session == true`。
- **`session_load` handler（`handlers.rs`）**：`acp_session_id_to_mag` 解析（非法 id 回内部错误）→
  `service.resume_session(sid).await` → 回空 `LoadSessionResponse::new()`（mag 无 session modes /
  config options 可报）。请求的 `cwd`/`mcp_servers` 不重复应用——会话按持久化的配置恢复（rustdoc 注明）。
  已在 `serve` 注册（`service_for_load` 一份 `Arc`）。
- **e2e（`tests/e2e.rs`）**：`FakeService` 增加 `recorded_resume`；新增
  `session_load_round_trips_over_in_memory_pipe`：真实 ACP client 走 `initialize`（断言宣告位）→
  `session/new` → `session/load`，断言 fake service 以对创建的同一 mag `SessionId` 收到
  `resume_session`（<0.01s）。
- **验证（全绿）**：1) `cargo fmt --all -- --check` 干净；2) `cargo test -p mag-acp` 33 passed
  （map 22、e2e 3、prompt 3、permission_bridge 3、cancel 2）；3) `cargo clippy --all-targets --
  -D warnings` 无警告；4) `cargo test --workspace` 全通过（126：mag-acp 33、mag-core 50+2、
  mag-service 12、mag-sources 10、mag-tools 6+13，0 fail）；5) `RUSTDOCFLAGS="-D warnings" cargo doc
  --no-deps --workspace` 无警告。
- 下一个未完成任务：M4-R（Review：M4 cancel + load + 能力宣告）。

### [DONE] M4-R Review：M4 cancel + load + 能力宣告

**上下文**：核对 M4 对 `docs/ACP.md` §3.5/§3.3/§7 的完整性；真实任务，不得跳过。

**做什么**：

1. 对照 §3.5：cancel 后本轮必返回 `Cancelled`、挂起权限被干净收尾（无悬挂）。
2. 对照 §3.3/§7：`load_session` 宣告与 mag-core 实际恢复能力一致；`session/load` 仅在宣告时注册。
3. 对照 §7：所有能力位如实宣告，无假装支持。
4. 确认无未调度失败测试；跑完整验证序列 1–5。

**验证条件**：完整验证序列 1–5 全绿；review 结论 + 对照表 + 缺口汇总写入完成记录。

**完成记录（M4-R）**：

对照 `docs/ACP.md` §3.5/§3.3/§7 逐条核对 M4（M4-0 契约扩展、M4-1 cancel、M4-2 load）成果。
**发现并修复 1 个真实缺陷**（见下「review 发现」），修复后全序列绿。

- **§3.5 `session/cancel` 对照表**：

  | §3.5 要求 | 实现 | 证据 |
  | --- | --- | --- |
  | notification（无响应），`on_receive_notification` 注册 | `serve` 注册 `CancelNotification` → `session_cancel` | `lib.rs` + `handlers.rs::session_cancel` |
  | 收到后 `service.cancel(sid)`，令本轮干净终止 | 先翻 `CancelTracker` 再 `service.cancel`；无效 sid / service 错误静默（notification 无错误通道） | `handlers.rs::session_cancel` |
  | 本轮终态 → `PromptResponse{stop_reason: Cancelled}`（规范强制） | 终态 `RunError{kind: RunErrorKind::Cancelled}`（M4-0）→ `run_terminal_to_stop_reason` → `Cancelled`，无需本轮 cancel 标志 | `map.rs` + 测试 `cancel_ends_prompt_turn_with_cancelled_stop_reason` |
  | 挂起 `session/request_permission` 以 cancel 收尾 | `bridge_permission` `select!` 竞速 tracker；cancel 胜出按 `Cancelled` outcome 保守回译；`InteractionNotFound` 静默容忍（M4-0 后 service 侧可自行收尾） | `handlers.rs::bridge_permission` + 测试 `cancel_during_pending_permission_wraps_up_both_sides`（client 永不应答 + service 回 NotFound 双重刁难下仍 `Cancelled` 收尾） |

- **§3.3 `session/load` 对照表**：

  | §3.3 要求 | 实现 | 证据 |
  | --- | --- | --- |
  | 仅当宣告 `load_session` 才可用 | `agent_capabilities().load_session == true` 且 handler 已注册（两者同源同改，无“宣告了没接”或反之） | `map.rs` + `lib.rs` |
  | 反查 mag `SessionId` → `resume_session` → `LoadSessionResponse` | `session_load`：解析失败 / service 错误回内部错误；成功回空响应（无 modes/config options） | `handlers.rs::session_load` |
  | 宣告取决于 mag-core 恢复实际就绪度 | `engine.rs::resume_session` 完整实现（config+snapshot 恢复、幂等），`e2e_offline.rs` 跨重启恢复测试兜底 | M4-2 完成记录的就绪度核实 |

- **§7 能力如实宣告对照**：`load_session=true`（有 handler + mag-core 恢复支撑）；多模态 prompt 位全
  `false`（未实现）；`mcp/session/auth` 未宣告；`InitializeResponse` 回传协商版本（M1 既有）。无假装支持。

- **review 发现（已修复，非绕过）**：`CancelTracker` 的 watch 标志**取消后不复位**——若某轮在审批挂起期间
  被 cancel（channel 已建、标志已翻），同一会话下一轮 prompt 再遇审批时，`wait_for_cancel` 经
  `borrow()` 立即读到残留的 `true`，桥接被“幻影取消”瞬时答出 Cancel，该轮卡死（无终态事件）。
  修复：`run_prompt_pump` 启动时 `cancels.reset(session_id)`——新 turn 从未取消状态开始（语义对齐
  mag-core“无活动 run 时 cancel 是 no-op”）。回归测试 `cancel_does_not_leak_into_the_next_turn`
  （turn 1 审批挂起中被 cancel → turn 2 审批被 approve 须 `EndTurn` + `Approve`）：**已验证无修复时必红**
  （临时注释 reset 后 10s 超时失败），修复后稳定绿。另注意初版回归测试曾因 turn 1 无审批（channel 未建）
  而未真正复现——已修正为两轮都在审批挂起的场景。

- **缺口汇总**：无未调度缺口。已知可接受取舍：tracker channel 随会话常驻（每会话一个小 channel，注释已
  注明）；“cancel 恰好落在 turn 启动前”的窗口语义与 mag-core 一致（no-op）。

- **验证序列 1–5 全绿**：`cargo fmt --all -- --check` 干净；`cargo test -p mag-acp cancel` 3 passed；
  `cargo clippy --all-targets -- -D warnings` 无警告（修掉一处 collapsible_if）；`cargo test --workspace`
  全通过（127：mag-acp 22+3+3+3+3=34、mag-core 50+2、mag-service 12、mag-sources 10、mag-tools 6+13，
  0 fail）；`RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace` 无警告。
- M4 里程碑全部 `[DONE]`。下一个未完成任务：M5-1（协议级全回合 e2e）。

---

## Milestone M5 — 协议级 e2e + 收官验收

目标：用 acp crate 自身 client 经内存管道驱动 mag-acp 跑全回合（含流式 + 权限 + cancel），service 侧注入
fake LLM，全程离线；真实 Zed 联调 `#[ignore]`。最后收官验收 mag-acp interface。对应 `docs/ACP.md` §9。

### [DONE] M5-1 协议级全回合 e2e（`initialize → session/new → session/prompt`（流式+权限）`→ session/cancel`）

**上下文**：

- 协议级 e2e（`docs/ACP.md` §9）：用 acp crate 自身 client 端（或 agent-lib 的 ACP client）经内存管道
  （复用 M1-2 的 `tokio::io::duplex` 夹具）驱动 mag-acp，service 侧注入一个 fake `Arc<dyn MagService>`
  （脚本化 `subscribe` 事件流 + fake LLM 语义），跑完整回合：
  `initialize → session/new → session/prompt`（含流式 `AgentMessageChunk` + 一次 `session/request_permission`
  往返）`→ session/cancel`，断言 client 侧观察到的 `session/update` 序列、权限往返、stop reason 全部正确。
- 真实 Zed 联调（spawn `mag --acp` 子进程）为 `#[ignore]` / 手动，缺环境干净跳过（绿）。

**做什么**：

1. `tests/e2e_acp.rs`：内存管道全回合测试（一个脚本覆盖流式 + 权限 approve + 后续 cancel）。断言 client 看到
   的 update 序列、`RequestPermissionRequest` 内容（tool_call/options）、`PromptResponse.stop_reason`。
2. 增补边界回合：纯对话（无工具/无权限）→ `EndTurn`；prompt 中途 cancel → `Cancelled`。
3. 一个 `#[ignore]` 的真实 Zed 联调骨架测试（spawn `mag --acp`），带注释说明手动运行方式。

**验证条件**：

- 聚焦测试：`cargo test -p mag-acp --test e2e_acp`（全回合 + 边界），每个用例 1 分钟内绿，无卡死；`#[ignore]`
  的 Zed 测试默认跳过。
- 完整验证序列 1–5；clippy / doc 无警告。

**完成记录（M5-1）**：

- **`tests/e2e_acp.rs`（新）**：真实 acp client 经 `Channel::duplex` 内存管道驱动 `mag_acp::serve`，service 侧
  注入脚本化 `RoundService`（`initial` 事件立即流式、`on_respond` 事件在 `respond_interaction` 后释放、
  `on_cancel` 事件在 `cancel` 后释放；全程记录 `create_session`/`respond_interaction`/`cancel`）。每个用例都
  走完整 `initialize`（断言 `load_session` 宣告位）`→ session/new → session/prompt`：
  - `full_round_streams_permission_and_completes`：TextDelta → `InteractionRequested` →（client 断言
    `RequestPermissionRequest` 的 tool_call id 与 once-scoped allow/reject options 后 approve）→ TextDelta →
    `RunFinished`；断言 stop=`EndTurn`、client 观测文本 `draft final`（权限暂停两侧的事件序正确）、
    `respond_interaction` 收到 `Approve`、无 cancel。
  - `plain_conversation_ends_with_end_turn`：纯对话边界——两条 TextDelta + `RunFinished`，stop=`EndTurn`，
    无 interaction、无 cancel。
  - `mid_prompt_cancel_ends_with_cancelled`：流一条后静默，client 收首条 update 即发 `session/cancel`；
    断言 stop=`Cancelled`、`MagService::cancel` 以正确 sid 被调。
  三个用例均 <0.01s，无卡死。
- **Zed 联调骨架**：`zed_integration_manual_handshake`（`#[ignore]`）——spawn `mag --acp` 子进程，stdio 上发
  `initialize` 并断言响应含 `agentCapabilities`；无 `mag` 二进制时干净跳过；rustdoc 注明手动运行方式
  （`cargo test -p mag-acp --test e2e_acp zed_integration -- --ignored`）。tokio dev-deps 补 `process` /
  `io-util` feature（仅 dev）。
- **验证（全绿）**：1) `cargo fmt --all -- --check` 干净；2) `cargo test -p mag-acp --test e2e_acp`
  3 passed + 1 ignored；3) `cargo clippy --all-targets -- -D warnings` 无警告（修掉一处 clone_on_copy）；
  4) `cargo test --workspace` 全通过（130：mag-acp 22+3+3+3+3+3=37、mag-core 50+2、mag-service 12、
  mag-sources 10、mag-tools 6+13，0 fail，1 ignored）；5) `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
  --workspace` 无警告。
- 下一个未完成任务：M5-R（Review：mag-acp 整体验收）。

### [DONE] M5-R Review：mag-acp 整体验收

**上下文**：mag-acp interface 收官验收；对照 `docs/ACP.md` 全文逐节核对；真实任务，不得跳过。

**做什么**：

1. 逐节对照 `docs/ACP.md` §1–§9：方法↔类型、泵、审批桥接、类型映射、安全边界、能力宣告、双向不冲突、测试
   策略——确认全部实现或明确记录为后续（并有对应 `TODO.md` 追踪）。
2. `MagService` 方法 vs mag-acp 映射对照表：确认 ACP 只用子集、未映射方法（`list_sessions`/`delete_session`/
   `probe_local_agents` 等）有意留待 GUI/web，无遗漏 ACP 必需映射。
3. 依赖边界终检：`mag-acp` 只依赖 `mag-service` + acp crate；`mag` bin 是唯一装配点；`MagService` 契约未改。
4. 确认无未调度失败测试；跑完整验证序列 1–5（含 `cargo test --workspace` ≤30min）。
5. 汇总遗留缺口 / 后续 interface 交接点（web / Tauri）。

**验证条件**：

- 完整验证序列 1–5 全绿；`e2e_acp` 稳定通过；无未调度失败测试。
- 逐节对照表 + `MagService` 映射对照表 + 依赖边界终检 + 缺口汇总写入完成记录。
- 放行判据：M1–M5 全 `[DONE]`、验证序列全绿、`e2e_acp` 稳定通过。

**完成记录（M5-R）**：

**放行判据达成**：M1–M5 全部 `[DONE]`、验证序列 1–5 全绿、`e2e_acp` 稳定通过（3 passed + 1 ignored）。
本任务为收官 review，未改编译产物。

- **`docs/ACP.md` 逐节对照**：

  | 节 | 要求 | 状态 | 证据 |
  | --- | --- | --- | --- |
  | §0 定位与边界 | agent(server) 角色、stdio JSON-RPC、纯翻译器、不经 Command/Event | ✓ | `serve` 仅 agent 端；bin 用 `Stdio`；mag-acp 只消费 `MagService`/`ServiceEvent` |
  | §1 acp crate 形状 | role-marker + builder + typed-handler、`connect_to` run loop | ✓ | M1-R 就地核对记录 |
  | §2 整体结构 | handlers / pump / permission bridge / map 分层 | ✓ | `src/{lib,handlers,map}.rs` 与图一致 |
  | §3 请求映射 | initialize / new / load / prompt / cancel 五方法 | ✓ | 全部注册于 `serve`（M1–M4） |
  | §4 类型映射 | SessionId / UserInput / ServiceEvent→SessionUpdate / stop reason 纯函数 | ✓ | `map.rs`（M2-R 全 13 变体核实；stop reason M4-0 升级为 kind 结构化） |
  | §5 审批桥接 | 异步暂停点、outcome 回译、两家族同通道 | ✓ | M3-R 对照表 |
  | §6 安全边界 | cwd→worktree、特权工具必过 gate、凭据不经 ACP | ✓ | M1-3 worktree 落位；审批无旁路；依赖边界无 CredentialStore |
  | §7 能力协商 | 如实宣告、版本回传、不启 unstable v2 | ✓ | `agent_capabilities`（load_session 有恢复支撑）；`agent-client-protocol = "1"` 默认 features |
  | §8 双向不冲突 | mag-acp 不涉 external-acp（client 方向） | ✓ | 依赖边界无 agent-lib |
  | §9 测试策略 | 纯函数单测 + handler 级 + 协议级 e2e + Zed `#[ignore]` | ✓ | map 22、prompt 3、permission_bridge 3、cancel 3、e2e 3、e2e_acp 3、zed 1(ignored) |

- **`MagService` 方法 vs ACP 映射对照表**：

  | `MagService` 方法 | ACP 映射 | 状态 |
  | --- | --- | --- |
  | `create_session` | `session/new` | ✓ M1-4 |
  | `resume_session` | `session/load` | ✓ M4-2 |
  | `send_message` | `session/prompt` | ✓ M2-2 |
  | `cancel` | `session/cancel` | ✓ M4-1 |
  | `respond_interaction` | `session/request_permission` outcome | ✓ M3-2 |
  | `subscribe` | prompt 泵 → `session/update` | ✓ M2-2 |
  | `list_sessions` / `delete_session` / `list_sources` / `probe_local_agents` | **有意不映射** | 留待 GUI/web interface（ACP 无需） |

- **依赖边界终检**：`crates/mag-acp/Cargo.toml` 常规依赖 = `mag-service` + `agent-client-protocol` +
  `futures` + `serde_json` + `tokio(sync)`（全部在 PLAN.md 允许清单内）；无 `mag-core`/`agent-lib`/
  tauri/axum；无 unstable feature。`mag` bin 是唯一同时见 mag-core 与 mag-acp 的装配点（`main.rs`）。
  `MagService` 契约在冻结后仅经两次**向后兼容**扩展（M1-3 `cwd`、M4-0 `RunError.kind`/`SessionConfig.budget`，
  均以显式前置任务留痕），无破坏性变更。

- **缺口汇总 / 后续交接点**（均非阻塞、均有归属）：
  1. `MaxTokens` stop reason 不产出（service 侧无独立 token 上限信号；token 预算耗尽归
     `BudgetExhausted→Refusal`）——`PLAN.md` R-5 残留项。
  2. 审批 `Approval` wire 只有 `call_id + requirement`，`tool_name`/`input` 富化待 agent-lib 上游 wire 演进
     （M3-R 缺口 1）。
  3. `session/load` 不回放历史消息（client 自行决定何时刷新）——后续可按需加历史回放。
  4. 后续 interface 交接点：`list_sessions`/`delete_session`/`probe_local_agents` 等未映射方法留给 web /
     Tauri；`mag --acp` 的 provider/model 装配随来源配置（mag-sources）接入。
  5. 真实 Zed 联调为手动 `#[ignore]`（`zed_integration_manual_handshake`），需本机环境。

- **验证序列 1–5 全绿**（收官复核）：`cargo fmt --all -- --check` 干净；`cargo clippy --all-targets --
  -D warnings` 无警告；`cargo test --workspace` 130 passed / 0 failed / 1 ignored（mag-acp 37、mag-core 52、
  mag-service 12、mag-sources 10、mag-tools 19）；`RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace`
  无警告；`cargo test -p mag-acp --test e2e_acp` 稳定通过。

**mag-acp interface 收官：M1–M5 全部 `[DONE]`，放行判据全部达成。**
