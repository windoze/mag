# TODO：mag-cli 落地任务单（最小 CLI 验证原型 + 运行时配置系统）

> 依据 [`PLAN.md`](PLAN.md) 与**唯一设计输入** [`docs/CLI.md`](docs/CLI.md)（决策 D1–D6、§5 service 主干
> 前置改动清单、§5A agent-lib 前置需求——A/B 类已在 agent-lib 侧全部完成）。
> **范围：`docs/CLI.md` §5 的 service 主干前置改动（M1–M5）+ `mag-cli` crate 本身（M6）。**
> 既有计划归档：[`docs/archive/2026-07-19-mag-service/`](docs/archive/2026-07-19-mag-service/)、
> [`docs/archive/2026-07-20-mag-acp/`](docs/archive/2026-07-20-mag-acp/)。

## 通用执行规则

- **一次一个任务**：每次只执行「首个标题带 `[TODO]` 的任务」。完成后把 `[TODO]` 改为 `[DONE]`，在任务末尾
  补「完成记录」，提交并推送，然后继续下一个任务（本计划按用户指令连续推进，不停顿等待）。
- **完成的定义**：只有标题带 `[DONE]` 才算完成。仅填完成记录而标题仍 `[TODO]`，按未完成处理。review 任务
  （`M<n>-R`、`F-R`）是真实任务，不得跳过。
- **编号**：任务按实现顺序编号 `M<里程碑>-<序号>`（如 `M1-1` = milestone 1 第一个任务）；每个里程碑末尾有
  独立 review 任务 `M<n>-R`；全部里程碑完成后有一次全计划 review `F-R`。
- **依赖边界（硬约束）**：
  - `mag-config`（新 crate）：纯数据 crate，**只依赖** serde/toml/thiserror 等通用 crate；**不得**依赖
    agent-lib / mag-core / mag-service。DTO↔DO 双向转换在本 crate 内完成。
  - `mag-service`：不依赖 agent-lib / mag-core（现状保持）；对契约只做**向后兼容新增**（方法/变体/字段），
    不改既有方法签名与事件语义；新增事件变体/字段必须 `#[serde(default)]` 或新增变体（`#[non_exhaustive]`
    已就位）。
  - `mag-core`：可依赖 mag-config / mag-service / mag-sources / mag-tools / agent-lib；M4 起 agent-lib
    依赖开 `external-acp` feature。
  - `mag-cli`（新 crate）：**只依赖** `mag-service`（+ rustyline/tokio/futures/serde/serde_json）；
    **不得**依赖 `mag-core` / `agent-lib` / `mag-config`，面对 `Arc<dyn MagService>`。装配
    `mag-core::Engine` 注入的是上层 bin（`crates/mag`）。
- **不改已冻结语义**：若发现前置缺口（缺事件/缺字段/缺方法），在本文件正确依赖位置插最小前置任务
  （按向后兼容方式加），让被阻塞任务显式依赖它，然后提交并继续。
- **离线测试纪律**：所有测试必须离线——LLM 用 fake `LlmClient`；interface 级注入 scripted
  `Arc<dyn MagService>`；CLI e2e 用管道驱动 stdin/stdout；external ACP 用本地 fake ACP 进程脚本
  （`docs/CLI.md` §6）。不依赖网络 / 真实凭据 / 真实 LLM / 真实 ACP agent。每个测试须 1 分钟内完成，卡住
  即为 bug，须立刻修。真实联调一律 `#[ignore]`，缺环境干净跳过（绿），不输出 secret。
- **配置 secret 纪律**：配置文件只存引用（`{env=...}` / `{keyring=...}`），任何测试/日志不输出解析后的
  secret 值（`docs/CLI.md` §4.1）。
- **默认完整验证序列**（任务另有放宽以任务为准）：
  1. `cargo fmt --all -- --check`
  2. 聚焦测试（任务给出精确过滤名）
  3. `cargo clippy --all-targets -- -D warnings`
  4. `cargo test --workspace`
  5. `cargo doc --no-deps --workspace`
- **公开 API 必须带 rustdoc**（crate 开 `#![warn(missing_docs)]`）。
- 环境：cargo 不在默认 PATH，每个 shell 先 `export PATH="$HOME/.cargo/bin:$PATH"`。

### 复用锚点（各任务通用，避免反复翻库）

- **`MagService` trait**（`crates/mag-service/src/service.rs`，object-safe，`Arc<dyn MagService>`）：
  `create_session(SessionConfig)->SessionId`、`list_sessions()->Vec<SessionInfo>`、`resume_session(SessionId)`、
  `delete_session(SessionId)`、`send_message(SessionId,UserInput)->RunId`、`cancel(SessionId)`、
  `respond_interaction(SessionId,RequestId,InteractionResponseWire)`、
  `subscribe(Option<SessionId>)->BoxStream<'static,ServiceEvent>`、`list_sources()`、`probe_local_agents()`；
  async 方法返回 `Result<_,ServiceError>`。`ServiceError`（`#[non_exhaustive]`）现有变体：
  `SessionNotFound/InteractionNotFound/InvalidInput/Unsupported/Backend`。
- **`ServiceEvent`**（同文件，`#[serde(tag="type",rename_all="snake_case")]`，`#[non_exhaustive]`）：
  `SessionCreated{id,config}`、`RunStarted{id,run_id}`、`RunFinished{id,output:RunOutput}`、
  `RunError{id,message,kind:RunErrorKind}`、`TextDelta{id,text}`、`ToolStarted{id,trace:ToolTrace}`、
  `ToolFinished{id,trace}`、`InteractionRequested{id,request_id:RequestId,kind:InteractionKindWire}`、
  `DelegationStarted/Finished/Failed{id,trace:DelegationTrace}`、`DelegationMessage{id,message}`、
  `LocalAgentsProbed{available}`；`ServiceEvent::session_id()->Option<SessionId>`。
- **wire 类型**（`crates/mag-service/src/lib.rs`）：`SessionId/RunId/RequestId`（`transparent` 包 `Uuid`）；
  `UserInput{text,attachments}`；`SessionConfig{provider,model,tool_profile,cwd,routing:RoutingMode,
  budget:Option<SessionBudget>}`；`RunErrorKind::{Other,Cancelled,LoopLimitExceeded,BudgetExhausted}`；
  `ToolTrace{run_id,call_id,name,input,output,status:ToolStatusWire,message}`；
  `InteractionKindWire{Approval{call_id,requirement},Question{prompt},Choice{prompt,options},
  Permission{action_id,actor,category,risk,summary,subject,reason}}`；
  `InteractionResponseWire{Approval{step_id,call_id,decision,message},Answer{text},Choice{index},
  Permission{action_id,decision}}`；`ApprovalDecisionWire::{Approve,Deny,Timeout,Cancel}`。
- **mag-core driver**（`crates/mag-core/src/driver.rs`）：`run_turn` 用
  `agent.stream_with_cancel(text, cancel)` 消费事件流；cancel 已实现（CancelHandle 模式，pivot 旁路同构）；
  `Engine` 现有构造器 `new/with_llm_client/with_llm_client_and_tools/with_persistence`
  （`crates/mag-core/src/engine.rs`）。
- **agent-lib 已就绪能力**（mag 直接可用，无需再改 agent-lib）：
  `AgentRunStream::interject()`（pivot，仅 step 边界窗口接受，InvalidState 失败无副作用、盲重试安全）；
  子 agent 交互带 `InteractionOrigin{delegate,depth}` 路由到父级注入 handler；
  `Agent::reconfigure(ReconfigRequest)`（Idle 时准入，skill 变体报 Config 错）；`Agent::worker()`；
  `ManagedExternalAgent::acp(binary,args)` + `default_external_session_handler`
  （behind `external-acp` feature，默认关）；cancel 可抢占阻塞批（2s 宽限后 detach）。
- **配置文件设计**（`docs/CLI.md` §4，决策 D4）：TOML 于 `~/.config/mag/config.toml`（示例见 §4.2）；
  secret 只存引用；生效时机决策 D2（会话创建钉住 `Arc<ConfigSnapshot>` + `apply_config` 到 turn 边界，
  审批策略下一 run 生效）；turn-complete 通用通知/回调机制见 §4.5。

---

## Milestone M1 — pivot 能力（`docs/CLI.md` §3.2，决策 D1）

目标：`MagService` 暴露两层 pivot 语义的第一层（`pivot_message`，无 in-progress turn 则 `NotPivotable`），
mag-core driver 落地 pivot 队列旁路（agent-lib `interject()`）。第二层「不能 pivot 自动转 send_message」
在 CLI 侧（M6）实现。

### M1-1 [TODO] mag-service：pivot 契约新增

- **上下文**：`docs/CLI.md` §3.2（决策 D1 第一层）。只加不改。
- **实现要求**：
  - `MagService` 新增方法 `async fn pivot_message(&self, id: SessionId, input: UserInput)
    -> Result<(), ServiceError>`（带默认实现返回 `Err(ServiceError::Unsupported(..))`，保持下游实现者
    向后兼容；或按既有惯例不加默认实现——以 crate 内现有方法风格为准，二选一并在完成记录注明）。
  - `ServiceError` 新增变体 `NotPivotable { id: SessionId, reason: String }`（`#[non_exhaustive]` 下直接加）。
  - `ServiceEvent` 新增变体 `PivotQueued{id}`、`PivotApplied{id}`、`PivotDropped{id,reason}`；
    `ServiceEvent::session_id()` 覆盖新变体。
  - rustdoc 引用 `docs/CLI.md` §3.2 说明两层语义：本方法只负责第一层（turn 中途注入 pivot；无
    in-progress turn 返回 `NotPivotable`），第二层（调用方回落 `send_message`）由调用方实现。
- **验证条件**：`cargo test -p mag-service`；序列化/反序列化 roundtrip 单测覆盖三个新事件变体与新错误
  变体；默认验证序列全过。

### M1-2 [TODO] mag-core：driver pivot 队列旁路

- **上下文**：`docs/CLI.md` §3.2；agent-lib `AgentRunStream::interject()` 锚点（仅 step 边界窗口接受，
  InvalidState 失败无副作用、盲重试安全）。CancelHandle 同构模式见 driver.rs 现有 cancel 实现。
- **实现要求**：
  - Engine 实现 `pivot_message`：会话无 in-progress run → `Err(NotPivotable)`；有则入 pivot 队列并发
    `PivotQueued`。
  - driver `run_turn` 事件循环每次 poll 后 drain pivot 队列尝试 `stream.interject()`；`InvalidState`
    留队下次重试；接受则发 `PivotApplied`。
  - run 结束（含 cancel/出错）时队列仍有未落地 pivot → 逐条发 `PivotDropped{id,reason}`（reason 区分
    run 正常结束 / cancelled / error）。
  - pivot 文本作为 user message 注入对话历史（agent-lib interject 语义），与 `send_message` 的持久化路径
    对齐（消息须进 session 存储，重启可见）。
- **验证条件**：聚焦测试：fake LLM 下 (a) run 进行中 pivot 被接受且 `PivotQueued`→`PivotApplied` 顺序正确、
  pivot 文本进入后续 LLM 请求上下文；(b) 无 run 时 pivot 返回 `NotPivotable`；(c) run 恰好结束时 pivot 未
  落地发 `PivotDropped`；(d) pivot 后 session 持久化包含 pivot 消息。默认验证序列全过。

### M1-R [TODO] M1 review

- **实现要求**：通读 M1 全部 diff，对照 `docs/CLI.md` §3.2 检查：契约只加不改、事件顺序正确、pivot 与
  cancel 竞态（cancel 抢占后 pivot 必须 `PivotDropped` 而非丢失）、持久化对齐、rustdoc 完整。发现问题直接
  修复并补测试。
- **验证条件**：默认验证序列全过；完成记录列出 review 结论（发现/修复项）。

---

## Milestone M2 — 交互归因（`docs/CLI.md` §3.3，决策 D5）

目标：子 agent（delegate）产生的 `InteractionRequested` 统一 pop 到 root 会话事件流，并标注来源；
GUI/web/CLI 无需感知多个会话通道。

### M2-1 [TODO] mag-service：`InteractionOrigin` wire 类型 + 事件字段

- **上下文**：`docs/CLI.md` §3.3；agent-lib 已提供 `InteractionOrigin{delegate:Option<String>,
  depth:usize}`（子 agent 交互路由到父级注入 handler 时携带）。
- **实现要求**：
  - 新增 wire 类型 `InteractionOrigin{delegate:Option<String>, depth:u32}`（serde 完整，rustdoc 注明
    `None` = root 会话自身产生）。
  - `ServiceEvent::InteractionRequested` 新增字段 `#[serde(default)] origin: InteractionOrigin`
    （default = root，向后兼容旧事件流与消费者）。
- **验证条件**：序列化兼容单测（无 origin 字段的旧 JSON 可反序列化、新 JSON 含 origin）；默认验证序列全过。

### M2-2 [TODO] mag-core：子 agent 交互 origin 映射

- **上下文**：`docs/CLI.md` §3.3；mag-core 把 agent-lib 路由来的子 agent 交互（approval / question /
  choice / permission）统一经 root 会话的 `ServiceEvent::InteractionRequested` 发出。
- **实现要求**：
  - driver/approval 路径把 agent-lib `InteractionOrigin` 映射到 wire `InteractionOrigin`；
    root 会话自身交互 origin 为 default。
  - `respond_interaction` 按 `RequestId` 回灌时正确路由回发起交互的 delegate（RequestId 唯一性保证在
    mag-core 侧，不因多 delegate 并发交互而串号）。
  - 事件渲染不丢信息：origin 有 delegate 时 trace/日志含 delegate 名与 depth。
- **验证条件**：聚焦测试：两级 delegate 场景（fake LLM 驱动 subagent 触发审批），断言 root 订阅者收到
  一条 `InteractionRequested` 且 `origin.delegate == Some(..)`、`depth == 1`；`respond_interaction` 后
  正确的 delegate 恢复执行；无 delegate 时 origin 为 default。默认验证序列全过。

### M2-R [TODO] M2 review

- **实现要求**：对照 `docs/CLI.md` §3.3 检查：向后兼容（旧消费者不感知 origin 仍可用）、多 delegate
  并发交互不串号、rustdoc 完整。发现问题直接修复并补测试。
- **验证条件**：默认验证序列全过；完成记录列出 review 结论。

---

## Milestone M3 — 运行时配置系统（`docs/CLI.md` §4，决策 D2/D4，重点）

目标：配置不是一次读入的只读快照——DTO（配置文件/任意来源）↔ DO（程序实际使用的 `Arc` 对象树）双向
转换；运行时可更新（`update_config` / 文件 watch `reload`），在合适时机生效（会话创建钉住 +
`apply_config` 到 turn 边界）；同一套系统后续直接服务 GUI/web。

### M3-1 [TODO] 新 crate `mag-config`：DTO + TOML 读写 + 校验

- **上下文**：`docs/CLI.md` §4.1/§4.2（TOML 示例在 §4.2 代码块）。DTO 对应配置文件或任何可能的配置
  来源（决策 D4：不与「配置文件」绑死）。
- **实现要求**：
  - 新建 `crates/mag-config`，加入 workspace members；`#![warn(missing_docs)]`。
  - DTO serde 类型覆盖 §4.2 全部配置面：`[llm.<name>]`（provider/model/api_key 引用/endpoint/参数表）、
    `[tools]`（profile、逐工具开关/策略）、`[[agent]]`（name、role、llm 引用、tools 引用、system prompt、
    budget）、`[[external_agent]]`（name、kind=acp、command/args/env、capabilities）、`[session]`
    （默认 budget、persist 路径）、`[approval]`（默认策略、超时）等——以 §4.2 示例为最小完备集，
    字段命名与示例一致；全部字段 `Option`/`Default` 友好（部分配置合法）。
  - secret 字段只接受引用形态（`{env=VAR}` / `{keyring=NAME}`，字符串 mini-DSL 按 §4.2），提供
    `SecretRef` 解析类型（解析出值是后续 DO/运行时的事，DTO 层只保引用）。
  - TOML 读写：`parse_str`/`to_string_pretty`/`load(path)`/`save_atomic(path)`（写临时文件 + rename，
    保证 watch 端不读到半截文件）；行级校验错误（带 line/col 与字段路径，`ConfigError` 枚举）。
  - 轮次 trip 单测：§4.2 示例 TOML → DTO → TOML 语义等价。
- **验证条件**：聚焦测试 `cargo test -p mag-config`；默认验证序列全过。

### M3-2 [TODO] mag-config：DTO ↔ DO 双向转换

- **上下文**：`docs/CLI.md` §4.2（决策 D4）。DO 对应程序实际使用的配置对象；由于动态生效需求，DO 不是
  简单嵌套 struct，而是**引用计数的关联对象树/森林**——会话钉住 `Arc<ConfigSnapshot>` 后，后续
  `update_config` 产生新快照，旧快照对存活会话保持不变。
- **实现要求**：
  - DO 类型：`ConfigSnapshot`（`Arc` 内部不可变对象树：`LlmConfig`/`ToolConfig`/`AgentConfig`/
    `ExternalAgentConfig`/`SessionDefaults`/`ApprovalConfig` 等节点均 `Arc` 共享，revision: u64）；
    节点提供访问器；`ConfigSnapshot` 整树 `Clone`（廉价，`Arc` 拷贝）。
  - 双向转换：`ConfigDto -> ConfigSnapshot`（校验+归一化：引用解析为结构、默认填充、交叉引用检查——
    agent 引用的 llm/tools 名必须存在，报带路径的 `ConfigError`）；`ConfigSnapshot -> ConfigDto`
    （无损回写，secret 引用形态保持引用不物化）。
  - 转换测试：DTO→DO→DTO roundtrip 等价；非法交叉引用报错路径正确。
- **验证条件**：聚焦测试 `cargo test -p mag-config`；默认验证序列全过。

### M3-3 [TODO] mag-core：`ConfigService`

- **上下文**：`docs/CLI.md` §4.3。
- **实现要求**：
  - mag-core 新增 `config` 模块：`ConfigService` 持有 `RwLock<Arc<ConfigSnapshot>>` + 配置文件路径 +
    revision 计数；`current() -> Arc<ConfigSnapshot>`（廉价克隆）。
  - `update(dto) -> Result<Arc<ConfigSnapshot>, ConfigError>`：DTO→DO 转换 → 原子写配置文件
    （write-through，save_atomic）→ 替换 RwLock 内快照 + revision+1 → 发 `ConfigChanged`（经 M3-4 的
    事件通道）。写盘失败则不换快照（内存与文件一致优先，报错）。
  - `reload() -> Result<Arc<ConfigSnapshot>, ConfigError>`：重读文件 → 校验 → 换快照 → 发事件；
    文件损坏时保留当前快照并报错（不崩）。
  - 文件 watch（`notify` crate；若离线环境引入失败，降级为仅显式 `reload`，在完成记录注明）：去抖
    （~300ms）+ 自身 write-through 写入去重（记录自写 revision/mtime 指纹，避免自写触发 reload 回环）。
  - watch 触发的自动 reload 失败只记 warn 日志，不发错误事件、不换快照。
- **验证条件**：聚焦测试（用 tempdir 配置文件）：update 后 current() 返回新快照且文件落盘；reload 拾取
  外部手改；损坏文件 reload 报错且快照不变；自写不触发回环（若 watch 启用）。默认验证序列全过。

### M3-4 [TODO] mag-service：配置方法 + `ConfigChanged` 事件

- **上下文**：`docs/CLI.md` §4.3/§5。wire 形态：配置内容本身用 DTO 的 JSON 形态（serde 已就位），
  不在 mag-service 重复定义一套配置 wire 类型——`mag-service` 依赖 `mag-config` 取 DTO 类型（纯数据
  crate，依赖方向合法：service 层定义契约，config DTO 是契约数据）。
- **实现要求**：
  - `MagService` 新增四方法（风格与 M1-1 决策一致）：
    `get_config() -> ConfigDto`、`update_config(ConfigDto) -> Result<(), ServiceError>`、
    `reload_config() -> Result<(), ServiceError>`、`apply_config() -> Result<(), ServiceError>`。
    rustdoc 注明生效时机语义（决策 D2）：update/reload 立即换快照但**不影响已钉住会话**；
    `apply_config` 请求把当前快照在**各会话下一个 turn 边界**应用（经 turn-complete 机制，M3-5）。
  - `ServiceEvent` 新增变体 `ConfigChanged{revision: u64}`（全局事件，`session_id() -> None`）。
  - `ServiceError` 如需新增 `Config{message}` 变体承载配置错误。
- **验证条件**：`cargo test -p mag-service`；事件序列化 roundtrip；默认验证序列全过。

### M3-5 [TODO] mag-core：turn-complete 通知/回调机制 + `apply_config`

- **上下文**：`docs/CLI.md` §4.5（决策 D2 附带）：通用「turn complete」通知/回调机制——不光能
  apply config，还能做其他功能（如弹桌面通知）。
- **实现要求**：
  - mag-core 定义内部 trait（如 `TurnCompleteListener: Send + Sync`），`on_turn_complete(session_id,
    TurnSummary)`；Engine 持有 `Vec<Arc<dyn TurnCompleteListener>>`，driver 在每次 run 终态
    （finished/error/cancelled）后逐一调用（listener  panic/错误隔离，不影响主流程）。
  - 第一消费者：`ConfigApplier`——`apply_config()` 置 pending 标记；turn complete 时若 pending，对
    该会话执行 reconfigure（agent-lib `Agent::reconfigure(ReconfigRequest)`：llm 参数/工具集/审批策略/
    budget 等可变项；skill 变体等不可变项按 agent-lib 语义报 Config 错→记 warn 不换）；审批策略类变更
    下一 run 自然生效。无 in-progress run 的会话立即应用（Idle 准入）。
  - listener 机制留出注册口（Engine builder / `add_turn_complete_listener`），供后续桌面通知等使用。
- **验证条件**：聚焦测试：(a) `apply_config` 后进行中会话在 turn 结束边界被 reconfigure（fake agent
  断言 reconfigure 调用时机在 run 终态之后）；(b) Idle 会话立即应用；(c) listener 异常不影响后续
  listener 与主流程；(d) 不可变项变更记 warn 且不中断。默认验证序列全过。

### M3-6 [TODO] `Engine::from_config` + bin 读配置

- **上下文**：`docs/CLI.md` §4.3/§4.6；Engine 现有构造器在 `crates/mag-core/src/engine.rs`；
  bin 在 `crates/mag/src/main.rs`（现有 `--acp` 装配路径）。
- **实现要求**：
  - `Engine::from_config(config_service: Arc<ConfigService>, ..) -> Result<Engine, EngineError>` 装配
    构造器：按当前快照装配 LLM client（含 secret 引用解析——env/keyring 读取在此层做，失败报带引用名
    的错误、不输出值）、tool registry、sources registry（含 external agent slot 注册，供 M4 使用）、
    persistence、审批策略。
  - bin：新增 `--config <path>`（默认 `~/.config/mag/config.toml`）；启动读配置 → 建 ConfigService →
    `Engine::from_config`；`mag --acp` 同走配置装配（mag-acp 行为不变）。
  - 配置文件不存在：用内置默认配置（空 sources、无 external agent、默认审批策略）启动并记 info，
    不报错退出（验证原型友好）。
- **验证条件**：聚焦测试：`Engine::from_config` 用样例配置装配成功；缺 secret env var 报带引用名的错；
  bin 级 smoke（`--config` 指向 tempdir 样例，`--help`/启动路径不崩，可用 `#[ignore]` 或假 LLM 注入点）。
  默认验证序列全过。

### M3-R [TODO] M3 review

- **实现要求**：对照 `docs/CLI.md` §4 全节检查：DTO↔DO 双向无损、快照隔离（update 不影响已钉住会话）、
  write-through 原子性、watch 回环防护、turn-complete 边界语义、secret 不物化不输出、GUI/web 可用性
  （四方法 + 事件足以驱动配置 UI）。发现问题直接修复并补测试。
- **验证条件**：默认验证序列全过；完成记录列出 review 结论。

---

## Milestone M4 — delegation 接线（`docs/CLI.md` §5 P7，决策 D3）

目标：external ACP agent 是本程序核心功能（决策 D3：尽早动手）。model-routed `ask_<name>` 委派两条来源
全部落地：local LLM subagent（agent-lib `Agent::worker()`）与 external ACP agent
（`ManagedExternalAgent::acp`）。

### M4-1 [TODO] mag-core：local LLM subagent 委派 + `Delegation*` 事件映射

- **上下文**：`docs/CLI.md` §5 P7；agent-lib `Agent::worker()`、`ask_<name>` model-routed 委派机制；
  `ServiceEvent::DelegationStarted/Finished/Failed/DelegationMessage` wire 变体已在契约中（本任务把它们
  真正接通）。
- **实现要求**：
  - mag-core 新增 delegate 装配：按配置 `[[agent]]` 条目为会话主 agent 注册 worker delegate
    （agent-lib worker 语义），tool surface 出现 `ask_<name>`。
  - driver 把 agent-lib 委派生命周期事件映射到 `Delegation*` wire 事件（trace 含 delegate 名、输入摘要、
    输出/失败原因）；`DelegationMessage` 映射中间消息。
  - 子 agent 交互经 M2 origin 归因 pop 到 root。
- **验证条件**：聚焦测试：fake LLM 下主 agent 调 `ask_researcher` → `DelegationStarted/Finished` 顺序正确、
  trace 内容完整；delegate 内触发审批 → root 收到带 origin 的 `InteractionRequested`。默认验证序列全过。

### M4-2 [TODO] mag-core：external ACP agent 委派（决策 D3，核心）

- **上下文**：`docs/CLI.md` §5 P7 + 决策 D3；agent-lib `ManagedExternalAgent::acp(binary,args)` +
  `default_external_session_handler`（behind `external-acp` feature）；mag-sources 已有 ACP slot
  （`crates/mag-sources/src/registry.rs`）。
- **实现要求**：
  - mag-core 的 agent-lib 依赖开 `external-acp` feature。
  - 按配置 `[[external_agent]]`（kind=acp、command/args/env）经 mag-sources ACP slot 注册
    `ManagedExternalAgent::acp(..)` + `default_external_session_handler`；name 进入 tool surface
    （`ask_<name>`），`list_sources()` 如实反映（kind/available/capabilities）。
  - external agent 启动失败/探测失败：`SourceInfo.available=false` + 原因记日志，不阻塞 Engine 启动；
    委派调用时失败映射 `DelegationFailed`。
  - 会话生命周期对齐：会话结束/删除时 external session 清扫（agent-lib 未 committed 自动清扫已就位）。
- **验证条件**：聚焦测试用**本地 fake ACP 进程**（脚本实现 ACP initialize/session/prompt 最小协议，
  `docs/CLI.md` §6）：(a) `ask_<ext>` 委派全生命周期事件正确；(b) fake 进程崩溃映射 `DelegationFailed`；
  (c) `list_sources` 反映可用性；(d) 会话结束清扫（断言 fake 进程收到结束/退出）。真实 claude-code-acp
  等联调 `#[ignore]`。默认验证序列全过。

### M4-3 [TODO] 委派审批 + restore 重注册 delegate

- **上下文**：`docs/CLI.md` §5 P7；`ApprovalPolicy::ask_tool("ask_<name>")` 走 IpcApproval
  （`crates/mag-core/src/approval.rs`）；**已知陷阱**：restore 必须重注册全部 delegate，否则审批策略静默
  回落 auto_allow。
- **实现要求**：
  - 委派调用默认走审批：`ask_<name>` 按审批策略（配置 `[approval]` 段）经 IpcApproval 向 root 会话发
    `InteractionRequested`（Approval kind，origin 归因 M2）；批准才执行。
  - `resume_session` 恢复路径重注册会话全部 delegate（local + external），恢复后审批策略与 tool surface
    与新会话一致；补回归测试防静默回落。
- **验证条件**：聚焦测试：(a) 委派触发审批，deny 时 `DelegationFailed`/工具拒绝路径正确；(b) resume 后
  `ask_<name>` 仍出现在 tool surface 且审批策略生效（断言不回落 auto_allow）；(c) approve 后委派正常
  执行。默认验证序列全过。

### M4-R [TODO] M4 review

- **实现要求**：对照 `docs/CLI.md` §5 P7 与决策 D3 检查：两条来源行为一致（事件、审批、origin）、
  external 生命周期清扫、restore 完备性、feature gating 正确（不开 feature 时编译过、external 配置报
  明确错误）。发现问题直接修复并补测试。
- **验证条件**：默认验证序列全过；完成记录列出 review 结论。

---

## Milestone M5 — ask_user 工具（`docs/CLI.md` §5 P6，决策 D6）

目标：AskUserQuestion 式通用交互作为普通 plugin 先行（GUI 阶段再详细设计）。

### M5-1 [TODO] mag-tools：`ask_user` ToolPlugin

- **上下文**：`docs/CLI.md` §5 P6 + 决策 D6；`ToolPlugin` trait 在
  `crates/mag-tools/src/plugin.rs`（执行时细读签名）；交互桥复用 mag 侧审批/交互注入路径
  （`InteractionKindWire::Question/Choice`）。
- **实现要求**：
  - 新 `ask_user` 工具：输入 `{question: String, options: Option<Vec<String>>}`；有 options →
    `Choice{prompt,options}` 交互，响应 `Choice{index}`；无 → `Question{prompt}`，响应 `Answer{text}`。
  - 阻塞式 handler + 闭包捕获交互桥（与 approval 注入同路径）；`select!` `ToolContext::cancel`——cancel
    时立即返回取消（agent-lib 阻塞批抢占已就位），不悬挂。
  - 注册进 tool registry（tool profile 可控开关）；工具描述写清「向用户提问并等待回答」。
- **验证条件**：聚焦测试：fake LLM 触发 `ask_user` → 订阅者收到 `Question/Choice` 交互 →
  `respond_interaction` 回答成为工具输出进入后续上下文；cancel 中途打断工具立即返回 Cancelled。
  默认验证序列全过。

### M5-R [TODO] M5 review

- **实现要求**：对照 `docs/CLI.md` §5 P6 检查：交互桥复用一致、cancel 语义、tool profile 开关、rustdoc。
  发现问题直接修复并补测试。
- **验证条件**：默认验证序列全过；完成记录列出 review 结论。

---

## Milestone M6 — mag-cli crate（`docs/CLI.md` §1/§2）

目标：最小 CLI 验证原型——「符合 GUI/web 使用模式」风格的最小实现，验证端到端管线。易用性/美观不考虑。

### M6-1 [TODO] `mag-cli` 骨架：双任务 REPL + 基本对话渲染

- **上下文**：`docs/CLI.md` §1.1（crate 与依赖边界）/§1.2（双任务结构）。**硬约束**：只依赖
  `mag-service`（+ rustyline/tokio/futures/serde/serde_json）。
- **实现要求**：
  - 新建 `crates/mag-cli`，加入 workspace members；`#![warn(missing_docs)]`。
  - `Cli::run(service: Arc<dyn MagService>, opts)` 入口：input 任务（rustyline 读行）+ render 任务
    （`subscribe` 事件流渲染到 stdout）；两任务经 channel 协调； rustyline 与流式输出共享 stdout 的
    最小互斥（渲染时暂停 prompt 回显即可，不做高级 TUI）。
  - 基本对话：输入行 → `send_message`；`TextDelta` 流式原样写 stdout；`RunFinished` 换行 +
    可选 usage 摘要；`RunError` 打印 kind + message。
  - 会话生命周期：启动 `create_session`（`/new` 重建）；`/quit` 退出（delete 与否按 §2 命令面）。
- **验证条件**：聚焦测试：scripted `Arc<dyn MagService>` + 管道 stdin/stdout e2e——输入两行消息断言
  stdout 含流式文本与 finish 摘要。默认验证序列全过。

### M6-2 [TODO] PromptCoordinator：交互提示（审批 + Question/Choice）

- **上下文**：`docs/CLI.md` §1.2/§3.3；单一 pending 交互队列；origin 标注渲染（决策 D5）。
- **实现要求**：
  - PromptCoordinator：`InteractionRequested` 入队（同会话同一时刻只提示一条，其余排队）；render 任务在
    合适时机（当前无流式输出进行中）弹出提示。
  - 审批提示：显示 tool 名/输入摘要/origin（`[from <delegate>@depth<n>]` 前缀），读 y/n（+ always/
    never 若 wire 支持映射）→ `respond_interaction(Approval{decision})`。
  - Question → 读一行文本 → `Answer{text}`；Choice → 编号菜单读数字 → `Choice{index}`。
  - 超时不做（验证原型）；Ctrl-C 在 pending 交互中等价 cancel 决策（`ApprovalDecisionWire::Cancel` /
    对应变体）。
- **验证条件**：e2e：scripted service 发审批/问题/选择交互（含 delegate origin），断言提示文本含 origin
  标注、回答正确回灌、多条交互按序处理。默认验证序列全过。

### M6-3 [TODO] pivot/cancel/会话命令

- **上下文**：`docs/CLI.md` §2/§3.2（决策 D1 第二层在 CLI）。
- **实现要求**：
  - run 进行中输入普通文本 → 先 `pivot_message`，`NotPivotable` 自动回落 `send_message`（两层语义，用户
    无感）；`PivotQueued/Applied/Dropped` 渲染为一行状态提示。
  - Ctrl-C：run 进行中 → `cancel`；pending 交互 → cancel 决策（M6-2）；Idle → 忽略（不退出，退出用
    `/quit`）。
  - slash 命令：`/new`、`/sessions`（列表，标当前）、`/resume <id>`、`/delete <id>`、`/cancel`、
    `/sources`、`/help`、`/quit`——行为按 §2 命令面。
- **验证条件**：e2e：run 中输入触发 pivot（断言先 pivot 后无回落）；scripted `NotPivotable` 时断言回落
  `send_message`；Ctrl-C 路径（用信号或注入点）；各 slash 命令调用正确 service 方法。默认验证序列全过。

### M6-4 [TODO] `/config` 命令

- **上下文**：`docs/CLI.md` §2 + §4（决策 D2 生效时机）。
- **实现要求**：
  - `/config show`：打印 `get_config()` 的 TOML 形态（secret 引用原样显示，不物化）。
  - `/config reload`：`reload_config()`，打印结果（新 revision 或错误）。
  - `/config apply`：`apply_config()`，打印「将在各会话下一 turn 边界生效」语义提示；
    `ConfigChanged{revision}` 事件渲染一行提示。
- **验证条件**：e2e：三个子命令调用正确 service 方法并渲染预期输出；`ConfigChanged` 事件到达时打印。
  默认验证序列全过。

### M6-5 [TODO] bin 装配 + 端到端验证

- **上下文**：`docs/CLI.md` §1.1/§5/§6；bin 在 `crates/mag/src/main.rs`（现有 `--acp`）。
- **实现要求**：
  - bin 子命令：`mag`（默认 CLI）、`mag --resume <id>`、`mag --config <path>`（M3-6）、`mag --acp`
    （保留不变）；CLI 路径：读配置 → ConfigService → `Engine::from_config` → `Cli::run`。
  - 端到端 e2e：fake LLM 装配真实 Engine + mag-cli（管道 stdio），跑通：对话 → ask_user 交互 → 委派
    （local + fake external ACP）→ pivot → cancel → `/config reload` → `/resume` 恢复后继续对话。
    可分多个 e2e 测试，全部离线。
- **验证条件**：上述 e2e 全绿；默认验证序列全过。

### M6-R [TODO] M6 review

- **实现要求**：对照 `docs/CLI.md` §0 目标清单逐项核对验证覆盖：流式对话/工具权限/通用交互/多 agent
  编排（含 external ACP）/协作/持久化恢复/cancel/pivot/配置动态生效。检查依赖边界（`cargo tree -p
  mag-cli` 无 mag-core/agent-lib/mag-config）。发现问题直接修复并补测试。
- **验证条件**：默认验证序列全过；完成记录逐项列出 §0 验证清单结论。

---

## F-R [TODO] 全计划 review

- **实现要求**：全部里程碑完成后，对整轮改动做一次完整 review（可分子代理分块）：对照 `docs/CLI.md`
  全节（含决策 D1–D6）逐条核对；重点：冻结契约只加不改、配置系统 DTO↔DO 与快照隔离、pivot/cancel 竞态、
  delegate restore 完备性、external ACP 生命周期、依赖边界、离线测试纪律、rustdoc 完整性。发现的问题直接
  修复并补测试。
- **验证条件**：默认验证序列全过（含整个 workspace）；完成记录列出 review 发现与修复清单。
