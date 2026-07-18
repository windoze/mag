# Claude 执行计划 — M1-3 前置契约缺口：`SessionConfig.cwd` 承载

## 定位
`TODO.md` 首个未完成任务原为 **M1-3**（`session/new` handler → `create_session`，cwd → `SessionConfig`）。
M1-1 / M1-2 均已 `[DONE]` 且提交，工作区干净。

## 关键发现（就地核实）——契约缺口，阻塞 M1-3
- ACP `NewSessionRequest.cwd: PathBuf`（绝对路径，**必填**）= 会话工作根
  （acp schema v1.4.0 `agent.rs`；`docs/ACP.md` §3.2/§6）。
- mag 内建工具（read_file/shell/grep/list_dir，`crates/mag-tools`）**全部**相对 facade `Agent` 的
  **worktree** 执行并受 `safe_join` 约束（`docs/DESIGN.md` §3.2）。
- 缺口：
  1. `mag_service::SessionConfig`（`crates/mag-service/src/lib.rs:276`）只有
     `provider/model/tool_profile/routing`，**无 cwd 字段**。
  2. `mag-core::SessionDriver::new`（`crates/mag-core/src/driver.rs:75`）建 facade `Agent` 时**从不**
     调 `.worktree(..)`；agent-lib 默认回落 `WorktreeRef::new(".")`（`agent-lib/src/facade/agent.rs:1295`）。
  3. `MagService::create_session(SessionConfig)` 是唯一入口，cwd 只能经 `SessionConfig` 流入。
- 结论：ACP 传入 cwd 现在**无处承载、会被丢弃**，工具将在 mag 进程 cwd 而非客户端目录执行 →
  违反 ACP §3.2/§6。这正是 M1-3 上下文预设的契约缺口触发条件
  （“若 `SessionConfig` 确实无处承载 cwd 而 mag-core 又需要它”）。

## 决策（依据 TODO.md 通用规则 line 22-24 + PLAN.md line 36-37）
不在 mag-acp 侧丢 cwd、不臆造语义。回 service 主干**向后兼容**加字段，并在 `TODO.md` 正确依赖位置
插入最小前置任务，让被阻塞任务显式依赖它，然后**提交并停止**。

## 本次动作
1. `TODO.md`：
   - 新增前置任务 **M1-3**：`SessionConfig.cwd: Option<PathBuf>`（`#[serde(default)]` 向后兼容）+
     `SessionDriver::new` 在 `Some` 时 `.worktree(WorktreeRef::new(cwd))`。
   - 原 `session/new` handler 任务顺延为 **M1-4**，显式依赖 M1-3（cwd → `SessionConfig.cwd`）。
   - **M1-R** review 范围补：cwd→worktree 落位核对。
2. `memory/claude_plan.md`：本文件。
3. 提交并停止（下次调用执行新 M1-3 trunk 修复）。

## 可行性已核实（供下次 M1-3 实现参考）
- facade builder `Agent::builder().worktree(WorktreeRef)`（`facade/agent.rs:1086`）存在。
- `WorktreeRef` 经 `agent_lib::agent::{WorktreeRef}`（`agent/mod.rs:92`）/facade 可从 mag-core 达。
- worktree 序列化进 `AgentSpec`（`agent/spec.rs:60/332`），`self.agent.snapshot()` → `AgentSnapshot`
  内 `agent_state` 保留 spec；mag-core 测试可经快照 JSON 断言 worktree 路径（可观测）。
- SessionConfig 被 `persistence.rs` 持久化；`#[serde(default)]` 保旧快照（无 cwd）反序列化为 `None`。

## 未改动
- 不动 mag-acp（依赖边界保持）。本次仅编辑 TODO.md + memory；无代码改动，故不跑测试套件。
- PLAN.md 不改：line 37 已预置“回主干加字段 + TODO 插前置任务”流程，无阶段计划变更；
  §mag-service 契约清单（line ~105）待 M1-3 真正落地字段时一并订正。

---

# 更新（本次调用）：实现 M1-3 trunk 修复

前一次调用已把 M1-3 作为前置任务插入 `TODO.md`（当前首个未完成任务）。本次**真正落地**：

## 已核实锚点
- `agent_lib::agent::WorktreeRef`（`spec.rs:95`，`#[serde(transparent)]` 包 `PathBuf`）经
  `agent/mod.rs:92` 导出；`Agent::builder().worktree(WorktreeRef)`（`facade/agent.rs:1086`）存在。
- 快照可观测：`agent.snapshot()` → `AgentSnapshot{ agent_state: AgentStateSnapshot(transparent Value) }`；
  `AgentStateRecord.spec: AgentSpec`，`AgentSpec.worktree`（直接字段）→ JSON 路径
  `snapshot["agent_state"]["spec"]["worktree"]` = path 字符串。`WorktreeRef` transparent→纯字符串。
- 默认回落 `WorktreeRef::new(".")`（`facade/agent.rs:1295`）。

## 落地步骤
1. `mag-service/src/lib.rs`：`SessionConfig` 加 `pub cwd: Option<PathBuf>`
   （`#[serde(default, skip_serializing_if="Option::is_none")]`，rustdoc）。补 `config()` 测试字面量。
2. `mag-service/src/service.rs:410` `config()` 字面量补 `cwd: None`。
3. `mag-core/src/driver.rs`：`SessionDriver::new` 在 `config.cwd = Some(p)` 时
   `.worktree(WorktreeRef::new(p.clone()))`；导入 `agent_lib::agent::WorktreeRef`。
4. 补 `mag-core` 全部 `SessionConfig{..}` 字面量（engine.rs ×5、persistence.rs、tests/e2e_offline.rs）`cwd: None`。
5. 测试：
   - `mag-service`：serde round-trip 含 `cwd: Some(path)`；旧 JSON 无 cwd 键 → `None`（向后兼容）。
   - `mag-core` driver：`new(cwd=Some(p))` 快照 worktree==p；`cwd=None` → worktree=="."。
6. `PLAN.md` §mag-service 契约清单补 `cwd`。
7. 验证序列 1–5，提交，停止。

## 完成状态：M1-3 已 [DONE]（本次）
- 代码：`SessionConfig.cwd` 落地 + `SessionDriver::new` worktree 落位 + 全部字面量补 `cwd`。
- 测试：mag-service session_config 3 passed；mag-core worktree 2 passed。
- 验证序列 1–5 全绿（fmt / focused / clippy -D warnings / workspace / doc）。
- PLAN.md 契约清单已补 cwd；TODO.md M1-3 标题改 [DONE] + 完成记录。
- 下一个未完成任务：M1-4（`session/new` handler → create_session，依赖 M1-3，已就绪）。

---

# 更新（本次调用）：实现 M1-4 `session/new` handler → `create_session`

首个未完成任务 = **M1-4**（TODO.md:305）。依赖 M1-3 已 `[DONE]`（`SessionConfig.cwd` 就绪）。

## 已核实锚点
- `acp::NewSessionRequest{ cwd: PathBuf（必填绝对路径）, additional_directories, mcp_servers, meta }`
  （schema v1 `agent.rs:1011`）；`NewSessionResponse::new(session_id: impl Into<SessionId>)`（`agent.rs:1115`）。
- `mag_service::SessionConfig{ provider, model, tool_profile:Option, cwd:Option<PathBuf>, routing }`。
- `ServiceError: std::error::Error` → `acp::Error::into_internal_error(err)` 可转协议错误。
- `map::mag_session_id_to_acp` / `acp_session_id_to_mag` 已存在（M1-1）。
- builder 按请求类型分发 handler；新增第三个 `on_receive_request`（`NewSessionRequest`）。service 经
  async 闭包 move 捕获 + 每次 `Arc::clone` 供多次调用。
- 默认 provider/model：`"openai"` / `"gpt-5-codex"`（与 service.rs 测试 fixture 一致）；tool_profile=None、
  routing=RoutingMode::default()。cwd = Some(req.cwd.clone())（ACP 绝对路径，直接承载，不丢弃）。

## 落地步骤
1. `map.rs`：`pub fn new_session_request_to_config(req: &NewSessionRequest) -> SessionConfig`
   + `DEFAULT_PROVIDER`/`DEFAULT_MODEL` 常量 + rustdoc + 单测（默认值 + cwd 承载）。
2. `handlers.rs`：`session_new(service, request, responder, connection)`：`create_session(cfg)`→map id→
   `NewSessionResponse`；错误 `into_internal_error`。
3. `lib.rs`：注册 `session/new` handler，闭包捕获 `service`；移除 `let _ = &service;`。
4. `tests/e2e.rs`：录制型 fake（Arc<Mutex<Option<SessionConfig>>>）记录 config；扩展夹具跑
   `initialize → session/new`，断言 `config.cwd == Some(req.cwd)` 且回的 ACP SessionId 经
   `acp_session_id_to_mag` 解回同一 mag SessionId。
5. 验证序列 1–5：fmt / `cargo test -p mag-acp session_new` / clippy -D warnings / workspace / doc。
6. TODO.md M1-4 标 [DONE] + 完成记录（注明 cwd 经 SessionConfig.cwd 承载）。提交并停止。

## 完成状态：M1-4 已 [DONE]（本次）
- map: `new_session_request_to_config` + DEFAULT_PROVIDER/MODEL 常量 + 单测。
- handler: `session_new`（create_session → 映射 SessionId → NewSessionResponse；错误 into_internal_error）。
- lib: 注册 session/new handler，闭包捕获 service。
- e2e: 录制型 FakeService + `session_new_round_trips_over_in_memory_pipe`（cwd 承载 + id 往返）。
- 验证序列 1–5 全绿（fmt / mag-acp 6 tests / clippy -D warnings / workspace / doc）。
- TODO.md M1-4 标 [DONE] + 完成记录。PLAN.md 无需改（契约清单 M1-3 已含 cwd；里程碑表已含 session/new）。
- 下一个未完成任务：M1-R（M1 review）。

---

# 更新（本次调用）：执行 M1-R Review（M1 crate 骨架 + initialize + session/new）

首个未完成任务 = **M1-R**（TODO.md:366，review 任务，真实任务不得跳过）。

## 核对结论（对照 docs/ACP.md §1/§2/§3.1/§3.2）
- §1 方法↔类型 + handler 注册：initialize / session/new / authenticate 均经
  `Agent.builder().on_receive_request(closure, on_receive_request!())` 如实注册；
  `connect_to(transport)` 常驻 run loop。✔
- 依赖边界：mag-acp Cargo.toml 仅 mag-service + agent-client-protocol，无 mag-core/agent-lib；
  唯一装配点 mag bin（Engine::new → Arc<dyn MagService>）。✔
- 能力宣告保守（load_session/image/audio/embedded_context 全 false，auth_methods 空）。✔
- M1-3 契约缺口：SessionConfig.cwd（serde 向后兼容）→ new_session_request_to_config →
  SessionDriver::new .worktree(WorktreeRef)；e2e 端到端证明 cwd 未丢弃 + id 往返。✔

## 本次代码改动
- 仅一处 doc 微修：`crates/mag-acp/src/lib.rs` `serve` rustdoc 原漏列 session/new，已补正
  （M1-4 加了 handler 但函数级注释未同步）。无逻辑改动。

## 验证结果（完整序列 1–5 全绿）
1. fmt --check 干净；2. `cargo test -p mag-acp` 6 passed；3. clippy -D warnings 无警告；
4. `cargo test --workspace` 97 passed / 0 failed / 0 ignored；5. cargo doc -D warnings 无缺 doc。

## 状态：M1-R 已 [DONE]
- TODO.md 标题改 [DONE] + 写入完成记录（对照表 + 缺口汇总 + 验证结果）。
- 缺口汇总：无阻塞缺口，未插前置任务；后续项（M2 泵/映射、M3 审批、M4 cancel/load、M4-2 能力收口）已调度。
- PLAN.md 无需改（无阶段计划变更）。
- 下一个未完成任务：M2-1（map: ServiceEvent → SessionUpdate + ContentBlock → UserInput + stop reason）。

---

# 更新（本次调用）：实现 M2-1 `map`（ServiceEvent → SessionUpdate + ContentBlock → UserInput + stop reason）

首个未完成任务 = **M2-1**（TODO.md:449，纯函数映射，全无 IO，集中在 `map` 模块）。

## 已核实锚点（acp schema v1.4.0）
- `SessionUpdate`（client.rs:99，非穷举）：`AgentMessageChunk(ContentChunk)`、`ToolCall(ToolCall)`、
  `ToolCallUpdate(ToolCallUpdate)` 等。
- `ContentChunk::new(ContentBlock)`；`ContentBlock: From<T: Into<String>>`（content.rs:118 → Text）。
- `ToolCall::new(id: impl Into<ToolCallId>, title)`，builder：`.kind(ToolKind)`, `.status(ToolCallStatus)`,
  `.raw_input(impl IntoOption<Value>)`, `.raw_output(..)`, `.content(Vec<ToolCallContent>)`。
- `ToolCallUpdate::new(id, ToolCallUpdateFields)`；`ToolCallUpdateFields::new().status(..).content(..).raw_output(..)`。
- `ToolCallStatus{Pending,InProgress,Completed,Failed}`（非默认 Pending）；`ToolKind`（默认 Other）。
- `ToolCallContent: From<T: Into<ContentBlock>>`（tool_call.rs:520）→ 文本内容。
- `StopReason{EndTurn,MaxTokens,MaxTurnRequests,Refusal,Cancelled}`（非穷举）。
- `IntoOption<T> for Option<T>` 存在 → `Option<Value>` 可直接喂 `.raw_input(..)`。

## mag-service 侧（非穷举需 catch-all `_`）
- `ServiceEvent`：TextDelta/ToolStarted/ToolFinished/Delegation{Started,Finished,Failed,Message}/
  RunFinished/RunError/InteractionRequested/SessionCreated/RunStarted/LocalAgentsProbed。
- `ToolTrace{run_id,call_id:ToolCallIdWire,name,input:Option<Value>,output:Option<Value>,status:ToolStatusWire,message}`。
- `ToolStatusWire{Started,Finished,Denied,Cancelled,Failed}`（非穷举）。
- `DelegationTrace{run_id,delegate,task,output,message}`（无 call_id → 用 `delegate:{delegate}` 命名空间作 id，
  与真实 UUID call_id 不冲突）。
- `ContentBlock`（非穷举）→ 仅取 Text 拼接（换行连接）；Image/Audio/ResourceLink/Resource 忽略（能力未宣告）。

## 落地映射
1. `service_event_to_session_update(&ServiceEvent) -> Option<SessionUpdate>`：
   - TextDelta{text} → AgentMessageChunk(text)
   - ToolStarted{trace} → ToolCall(map_tool_call)
   - ToolFinished{trace} → ToolCallUpdate(map_tool_call_update)
   - DelegationStarted{trace} → ToolCall（id=delegate:{d}, status=InProgress）
   - DelegationFinished{trace} → ToolCallUpdate（status=Completed, output→content/raw_output）
   - DelegationFailed{trace} → ToolCallUpdate（status=Failed, message→content）
   - DelegationMessage{message} → AgentMessageChunk(message.text)（降级为文本）
   - InteractionRequested / RunFinished / RunError / SessionCreated / RunStarted / LocalAgentsProbed / `_` → None
2. `map_tool_call(&ToolTrace) -> ToolCall`（pub）：id=call_id.to_string(), title=name,
   status=tool_status_to_acp(status), raw_input=input.clone()。
3. `map_tool_call_update(&ToolTrace) -> ToolCallUpdate`（pub）：id 同上，
   fields.status=tool_status_to_acp, raw_output=output.clone(), message→content 文本。
4. `content_blocks_to_user_input(&[ContentBlock]) -> UserInput`：Text 换行拼接，非文本忽略。
5. `run_terminal_to_stop_reason(&ServiceEvent) -> Option<StopReason>`：RunFinished→EndTurn、RunError→Refusal、其余 None。
6. 私有辅助 `agent_message_chunk`、`tool_status_to_acp`。

## 测试（`cargo test -p mag-acp map::`）
TextDelta/ToolStarted/ToolFinished(状态映射)/Delegation{Started,Finished,Failed,Message}/未映射事件→None、
ContentBlock 单/多 Text 拼接 + 非文本忽略 + 空、stop reason 三分支。dev-dep 加 serde_json（构造 input Value）。

## 验证序列 1–5 后提交并停止。

## 完成状态：M2-1 已 [DONE]（本次）
- map.rs 落地 5 个纯函数（+2 私有辅助），全带 rustdoc；委派表示为工具（delegate:{d} 命名空间 id），
  未映射/未来变体保守降级，无臆造 ACP 语义。
- Cargo.toml 加 serde_json dev-dep（构造 tool input/output Value）。
- 16 个 map:: 单测全绿。验证序列 1–5 全绿（fmt/focused/clippy -D warnings/workspace/doc -D warnings）。
  （doc 修正：public 函数 rustdoc 不得链私有 tool_status_to_acp，改为纯 code span。）
- TODO.md M2-1 标 [DONE] + 完成记录。PLAN.md 无需改（无阶段计划变更；R-4/R-5 已预置本版决策）。
- 下一个未完成任务：M2-2（session/prompt handler 泵）。

---

# 更新（本次调用）：实现 M2-2 —— `session/prompt` handler 泵

## 定位
`TODO.md` 首个未完成任务 = **M2-2**（session/prompt handler 泵）。M2-1 已 [DONE] 且提交，工作区干净。
M2-1 已提供全部纯函数映射（`service_event_to_session_update` / `content_blocks_to_user_input` /
`run_terminal_to_stop_reason` / `map_tool_call*`）。本任务只做 handler 层的“泵”接线 + handler 级测试。

## 就地核实（无契约缺口，可直接实现）
- `MagService::subscribe(Some(sid)) -> BoxStream<'static, ServiceEvent>`（service.rs:123）——按会话过滤。
- `MagService::send_message(sid, input) -> Result<RunId, ServiceError>`（service.rs:83）。
- ACP handler 形状：`(service, req: PromptRequest, responder: Responder<PromptResponse>, cx: ConnectionTo<Client>)`。
- 出站通知：`cx.send_notification(acp::SessionNotification::new(session_id, update))?`
  （SessionNotification => "session/update"，schema v1）。
- `PromptResponse::new(stop_reason)`。错误经 `agent_client_protocol::Error::into_internal_error(err)`（沿用 session_new）。

## 实现
1. `crates/mag-acp/Cargo.toml`：`futures` 从 dev-dependency 提升为常规依赖（handler 需 `StreamExt::next()`）。
2. `handlers.rs` 新增 `session_prompt(service, req, responder, cx)`：
   - `sid = acp_session_id_to_mag(&req.session_id)?`；解析失败 → respond_with_error(内部错误)。
   - `input = content_blocks_to_user_input(&req.prompt)`（M2-1）。
   - **先 subscribe 再 send_message**（防竞态丢事件）：`let mut events = service.subscribe(Some(sid));`
     然后 `service.send_message(sid, input).await`；失败 → respond_with_error。
   - 泵 loop（`events.next().await`）：
     * `None`（流结束）→ break `EndTurn`。
     * `run_terminal_to_stop_reason(&ev)` = Some(stop) → break stop（RunFinished→EndTurn / RunError→Refusal）。
     * `InteractionRequested` → 本任务占位（M3 接 bridge_permission），continue（留显式分支+注释）。
     * 其余 → `service_event_to_session_update(&ev)`，Some(update) 时 `cx.send_notification(...)?`。
   - `responder.respond(PromptResponse::new(stop))`。
3. `lib.rs`：注册 `session_prompt` handler（再 clone 一份 service）。
4. handler 级测试（tests/e2e.rs 或新 tests/prompt.rs）：ScriptedService（脚本化 subscribe 事件流 +
   记录 send_message 是否在 subscribe 之后调用），驱动真实 ACP client `session/prompt`，断言：
   泵出的 session/update 序列与脚本一致、RunFinished→EndTurn、RunError→Refusal、空流→EndTurn。

## 验证序列 1–5：fmt → clippy -D warnings → focused(`-p mag-acp prompt`) → workspace → doc -D warnings。

## 完成状态：M2-2 已 [DONE]（本次）
- handlers.rs 新增 `session_prompt` 泵；lib.rs 注册 handler + 更新 rustdoc；Cargo.toml `futures` 提升为常规依赖。
- tests/prompt.rs 新增 3 个 handler 级泵测试（ScriptedService + 真实 ACP client over pipe + 通知收集）：
  全序列→EndTurn（含 subscribe 先于 send_message + input 映射断言）、RunError→Refusal、空流→EndTurn。
- 验证序列 1–5 全绿（fmt/clippy -D warnings/`test -p mag-acp prompt` 3 passed/workspace 112 passed 0 fail/doc -D warnings）。
- TODO.md M2-2 标 [DONE] + 完成记录。PLAN.md 无需改（无阶段计划变更；§3.4 泵结构与 R-4/R-5 已预置）。
- InteractionRequested 仅显式占位（TODO(M3)），无臆造 ACP 语义；无 workaround、无前置任务。
- 下一个未完成任务：M2-R（Review）。

---

# 更新（本次调用）：M2-R Review — M2 泵 + 类型映射

## 定位
`TODO.md` 首个未完成任务 = **M2-R**（line 586，`[TODO]`）。M2-1 / M2-2 均 `[DONE]` 且已提交，工作区干净。
这是 review 任务（真实任务，不得跳过）：核对 M2 对 `docs/ACP.md` §3.4/§4 的完整性。

## Review 方法（就地核实源码 vs 规范）
被审文件：`crates/mag-acp/src/{map.rs,handlers.rs,lib.rs}` + `tests/prompt.rs`。
源码真相：`crates/mag-service/src/{service.rs,lib.rs}` 的 `ServiceEvent`/`ToolTrace`/`DelegationTrace`/
`DelegationMessageWire`/`ToolStatusWire` 定义。

### 1) §3.4 泵伪码对照（handlers.rs::session_prompt）
- subscribe 先于 send_message ✓（line 123 先 subscribe，line 124 才 send_message；防竞态）。
- 逐事件处理 ✓：terminal → run_terminal_to_stop_reason 跳出；InteractionRequested → 占位 continue（M3）；
  其余 → service_event_to_session_update → send_notification。
- stop reason 跳出 ✓：RunFinished→EndTurn、RunError→Refusal、流尽 None→EndTurn。
- 错误处理**优于**伪码：send_message/session-id 失败经 respond_with_error(into_internal_error) 正确回客户端，
  而非伪码的裸 `?`（后者会不回响应直接失败）。非缺口。

### 2) §4 映射表对照（map.rs::service_event_to_session_update）
- ServiceEvent 实际 13 变体（service.rs:186-279），map 全覆盖：
  TextDelta→AgentMessageChunk、ToolStarted→ToolCall、ToolFinished→ToolCallUpdate、
  Delegation{Started→ToolCall(InProgress), Finished→Completed, Failed→Failed, Message→AgentMessageChunk}、
  InteractionRequested/RunFinished/RunError/SessionCreated/RunStarted/LocalAgentsProbed→None；`_`→None（non_exhaustive）。
- §4 表里的 `DelegationProgress` 在真实 `ServiceEvent` 中**不存在**（仅规范表的假设变体）；无需映射。非缺口。
- 字段访问全部与源定义一致：ToolTrace{call_id,name,input,output,status,message}、
  DelegationTrace{delegate,task,output,message}、DelegationMessageWire{text}。
- ToolStatusWire 5 变体全覆盖 + non_exhaustive `_`→InProgress（最不武断的非终态）。无臆造 ACP 语义。
- content_blocks_to_user_input：仅 Text 换行拼接，非文本忽略（能力未宣告）；stop reason 仅 RunFinished/RunError。

### 3) 并发多会话泵互不干扰
- handler 传 `subscribe(Some(session_id))` 按会话过滤（handlers.rs:123）；假设成立（依赖 mag-service 契约）。

### 4) 已知且**已调度**缺口（非未调度失败）
- InteractionRequested 在 M2 仅占位 continue：真实 service 下会使本轮暂停、泵在 events.next() 上等待，
  直到 M3 的 bridge_permission 接管。**已由 M3-1/M3-2 显式调度**（TODO.md line 606/641），非未调度缺口。
  M2 测试无该事件，故不会卡住。
- MaxTokens/MaxTurnRequests 未产出：需 service 侧区分（PLAN.md R-5），已在规范内保守留白。

## 结论
M2 实现与 §3.4/§4 一致，无未调度失败测试、无 workaround、无臆造 ACP 语义。无需改代码；
仅跑完整验证序列 1–5 并把 review 结论/对照表/缺口汇总写入 M2-R 完成记录、标 `[DONE]`、提交。

## 动作
1. 跑验证序列 1–5（fmt / `cargo test -p mag-acp` / clippy / `cargo test --workspace` / doc）。
2. TODO.md：M2-R 标 `[DONE]` + 完成记录（结论 + 对照表 + 缺口汇总）。
3. 提交并停止。PLAN.md 不改（无阶段计划变更）。

---

# M3-1 `map`：`InteractionKindWire → RequestPermissionRequest` + outcome → `InteractionResponseWire`（进行中）

## 目标
纯函数映射（`docs/ACP.md` §5「映射细节」），无 IO。两个新公开函数 + 稳定 option id 常量，全部单测。

## 真相核对（已读源码）
- `InteractionKindWire`（mag-service lib.rs:424）：`Approval{call_id, requirement}`、`Question{prompt}`、
  `Choice{prompt,options}`、`Permission{action_id,actor,category,risk,summary,subject,reason}`。
  Approval **只有** call_id+requirement（无 tool_name/input）——ACP.md §5 提的 tool_name/input 尚未在冻结
  wire 类型里；不臆造，只用现有字段。
- `InteractionResponseWire`（lib.rs:468）：`Approval{step_id,call_id,decision,message}`、`Answer{text}`、
  `Choice{index}`、`Permission{action_id,decision}`。
- `ApprovalDecisionWire{Approve,Deny,Timeout,Cancel}`；`PermissionDecisionWire{Approve,Deny{reason},Cancel}`。
- mag-core `interaction_response_from_wire`（approval.rs:296）：按 (kind,response) **家族必须匹配**，否则
  Err("family does not match")；Approval 用 decision/message，step_id/call_id **被忽略**（由存储的 interaction
  重建）；Permission 用 decision，action_id 被忽略；Question 需 Answer；Choice 需 Choice。
- Question/Choice 在 mag-core 注释为 **mag-unused**（approval.rs:243）——facade 不产出，故不会到达 bridge。
- `RequestPermissionOutcome`（acp schema v1 client.rs:835）：`Cancelled`、`Selected(SelectedPermissionOutcome{option_id})`。
- `PermissionOptionKind{AllowOnce,AllowAlways,RejectOnce,RejectAlways}`；`PermissionOption::new(id,name,kind)`；
  `PermissionOptionId: From<&'static str>`。

## 设计决策
- **options**：只给 `AllowOnce`(approve)+`RejectOnce`(deny)。mag wire 无「始终允许」持久语义，加 AllowAlways/
  RejectAlways 会臆造 mag 不支持的语义 → 如实只给一次性两项。
- **稳定 option id 常量**（pub，避免魔法字符串漂移）：`PERMISSION_OPTION_ALLOW="mag:allow"`、
  `PERMISSION_OPTION_REJECT="mag:reject"`。分类：仅精确等于 allow id → Approve；其余（含 reject / 未知 id）→ Deny
  （fail-safe，未知 id 绝不批准，§6）。
- **tool_call 富化**：Approval → id=call_id、status=Pending、title=通用「Tool call requires approval」、
  reason(若有)入 content；Permission → id=action_id、status=Pending、kind=category→ToolKind、title=summary、
  raw_input=subject、reason(若有)入 content。
- **step_id 占位**：mag 忽略之。无 uuid 依赖，用 `StepIdWire::new(*call_id.as_uuid())` 复用 call_id 的 UUID 作
  惰性占位（全类型化 API，无魔法串、无 panic）。call_id 则回显 kind 里的真实 call_id。
- **outcome→response**：先归约 Decision{Approve,Deny,Cancel}（Selected(allow)→Approve、Selected(其它)→Deny、
  Cancelled/未来变体→Cancel）；再按 kind 组家族匹配的 response：Approval→Approval、Permission→Permission、
  Question→Answer("")、Choice→Choice(0)（惰性，mag-unused）、未来 `_`→Answer("")（最惰性，家族不符时 driver
  会干净 Err，非挂起）。

## 动作
1. map.rs：加 imports + 常量 + 私有 helper + 两个 pub 函数（带 rustdoc）+ 单测（Approval/Permission 组装 +
   Selected(approve/deny)/Cancelled 回译，覆盖两 kind）。
2. 验证序列：fmt → 聚焦 `cargo test -p mag-acp map::` / `permission` → clippy → `cargo test --workspace` → doc。
3. TODO.md：M3-1 标 `[DONE]` + 完成记录。提交并停止。PLAN.md 不改（无阶段计划变更）。

---

# 更新（本次调用）：实现 M3-2 `bridge_permission` 接入泵

## 定位
`TODO.md` 首个未完成任务 = **M3-2**（line 737，`[TODO]`）。前置 M3-1（纯函数映射）已 `[DONE]`。
工作区干净，HEAD=0474c84（M3-1）。

## 就地核实（可行性）
- 泵在 `handlers.rs::session_prompt`：`InteractionRequested` 分支当前是占位 `continue`（line 143-146）。
- M3-1 已提供 `map::interaction_to_permission_request(&acp::SessionId,&InteractionKindWire)->RequestPermissionRequest`
  与 `map::outcome_to_interaction_response(&InteractionKindWire, RequestPermissionOutcome)->InteractionResponseWire`。
- acp API：`connection.send_request(req).block_task().await? -> RequestPermissionResponse`（schema v1
  `client.rs:792`，字段 `pub outcome: RequestPermissionOutcome`）。`ConnectionTo<Client>` 有 `send_request`
  （`jsonrpc.rs:2286`）与 `send_notification`。
- `ServiceEvent::InteractionRequested{id,request_id:RequestId,kind:InteractionKindWire}`。
- `MagService::respond_interaction(SessionId,RequestId,InteractionResponseWire)->Result<(),ServiceError>`。
- `request.session_id: acp::SessionId`（= `agent_client_protocol::schema::v1::SessionId`，与 map 的
  `acp` 别名一致）。

## 计划
1. `handlers.rs`：新增 `async fn bridge_permission(service,&connection,session_id,acp_sid,request_id,kind)`
   → 组 request → `send_request().block_task().await?` → `outcome_to_interaction_response` →
   `respond_interaction`（ServiceError→internal_error）。
2. 泵 `InteractionRequested` 分支：destructure `{request_id,kind,..}`，调用 bridge；失败则
   `responder.respond_with_error` 并 return。成功 `continue`。
3. 更新 handler rustdoc（去掉「M3 占位」措辞）。
4. 新增 handler 级测试 `tests/permission_bridge.rs`：scripted service 中途产 `InteractionRequested`，
   fake ACP client 收 `session/request_permission` 回 approve/deny/cancel 三路径；断言暂停语义
   （client 未回 outcome 前 driver 不前进）+ outcome 正确回灌 `respond_interaction`。
5. 验证：fmt → 聚焦 `cargo test -p mag-acp permission_bridge` → clippy -D warnings → `cargo test --workspace`
   → doc。
6. `TODO.md` M3-2 标 `[DONE]` + 完成记录；提交并停止。

## 暂停语义测试思路
scripted subscribe 流：先发若干可观测事件（如 TextDelta），再发 InteractionRequested，之后再发一个
「哨兵」事件（如另一 TextDelta）与 RunFinished。fake client 在收到 request_permission 后先记录「driver
尚未产出哨兵」（因泵阻塞在 await），再回 outcome。断言 respond_interaction 收到正确 wire response，且
哨兵 update 在 outcome 回灌之后才出现（顺序断言）。

## 结果（M3-2 完成）
- 实现 `bridge_permission` + 泵接入；关键修复：`session_prompt` 把泵 `connection.spawn` 到 event loop
  之外（handler 内联 `block_task` 会死锁），泵抽为 `run_prompt_pump`。
- 新增 `tests/permission_bridge.rs` 3 条（approve/deny/cancel + 暂停语义），全绿。
- 验证 1–5 全绿（workspace 121 tests，clippy/doc 无警告）。
- TODO.md：M3-2 → [DONE] + 完成记录。下一个：M3-R。
