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
