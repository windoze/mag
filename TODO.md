# TODO：mag-web 落地任务单（单用户 web UI，与 desktop 共用 UI 基础设施）

> 依据 [`PLAN.md`](PLAN.md) 与**唯一设计输入** [`docs/WEB.md`](docs/WEB.md)（决策 D1–D8、§2
> REST+SSE 协议、§3 service 前置清单；开放问题 2026-07-20 全部拍板）。
> **范围：`docs/WEB.md` §3 的 service 前置（W1）+ `mag-web` crate（W2）+ `ui/` 前端 monorepo
> （W3–W5）。** desktop（Tauri 壳）不在本单。
> 既有计划归档：[`docs/archive/2026-07-19-mag-service/`](docs/archive/2026-07-19-mag-service/)、
> [`docs/archive/2026-07-20-mag-acp/`](docs/archive/2026-07-20-mag-acp/)、
> [`docs/archive/2026-07-20-mag-cli/`](docs/archive/2026-07-20-mag-cli/)。

## 通用执行规则

- **一次一个任务**：每次只执行「首个标题带 `[TODO]` 的任务」。完成后把 `[TODO]` 改为 `[DONE]`，在任务
  末尾补「完成记录」，提交并推送，然后继续下一个任务。
- **完成的定义**：只有标题带 `[DONE]` 才算完成。review 任务（`W<n>-R`、`F-R`）是真实任务，不得跳过。
- **编号**：任务按实现顺序编号 `W<里程碑>-<序号>`；每个里程碑末尾有独立 review 任务 `W<n>-R`；
  全部里程碑完成后有一次全计划 review `F-R`。
- **依赖边界（硬约束）**：
  - `mag-web`（新 crate）：**只依赖** `mag-service` + axum + tokio + futures + serde/serde_json
    （+ rust-embed 视需要）；**不得**依赖 `mag-core` / `agent-lib` / `mag-config`，面对
    `Arc<dyn MagService>`。装配 `mag-core::Engine` 注入的是上层 bin（`crates/mag`）。
  - `mag-service`：不依赖 agent-lib / mag-core（现状保持）；对契约只做**向后兼容新增**；ts-rs
    依赖必须 feature-gated（`ts-export`），默认构建不引入。
  - `ui/`：`@mag/app-web` 只依赖 `@mag/ui` + `@mag/client`；`@mag/ui` 不感知传输；`@mag/client`
    不感知渲染；`@mag/protocol` 只含 ts-rs 生成物，**禁止手写 wire 类型**。
- **不改已冻结语义**：若发现前置缺口，在本文件正确依赖位置插最小前置任务（向后兼容方式加），
  让被阻塞任务显式依赖它，然后提交并继续。
- **离线测试纪律**：Rust 测试全离线——scripted `Arc<dyn MagService>`、fake LlmClient、axum 内存
  请求（`tower::ServiceExt::oneshot`）、回环端口 SSE 客户端。前端 vitest 用 JSON fixture（scripted
  Event 流）驱动，不起真浏览器/真 server。不依赖网络/真实凭据/真实 LLM。每个测试须 1 分钟内完成，
  卡住即为 bug。真实浏览器联调一律 `#[ignore]`，缺环境干净跳过（绿）。前端依赖安装（pnpm install）
  允许网络，装完后测试离线。
- **secret 纪律**：配置 DTO 中 secret 只以 `{env=...}`/`{keyring=...}` 引用形态出现；任何 API 响应、
  测试、日志不输出解析后的值。
- **默认完整验证序列**（任务另有放宽以任务为准）：
  1. `cargo fmt --all -- --check`
  2. 聚焦测试（任务给出精确过滤名）
  3. `cargo clippy --all-targets -- -D warnings`
  4. `cargo test --workspace`
  5. `cargo doc --no-deps --workspace`
  前端相关任务追加：6. `pnpm -r test` 7. `pnpm -r build`（在 `ui/` 下）
- **公开 API 必须带 rustdoc**（crate 开 `#![warn(missing_docs)]`）；TS 公开接口带 TSDoc。
- 环境：cargo 不在默认 PATH，每个 shell 先 `export PATH="$HOME/.cargo/bin:$PATH"`。

### 复用锚点（各任务通用，避免反复翻库）

- **REST 路由表**（`docs/WEB.md` §2.1，权威）：`/api/sessions`（GET 列表/POST 创建）、
  `/api/sessions/{id}/resume|history|messages|pivot|cancel`（POST/GET 见表）、
  `/api/sessions/{id}/interactions/{rid}`（POST）、`/api/sources`（GET）/probe（POST）、
  `/api/config`（GET/PUT）/reload|apply（POST）、`/api/events`（GET SSE）。
- **错误投影**（§2.3）：SessionNotFound/InteractionNotFound→404、NotPivotable→409、
  InvalidInput/Config→400、Unsupported→501、Backend→500；body `{kind, message}`。
- **SSE**（§2.2）：`event: <snake_case type>` + `data: <Event JSON>`；~15s comment heartbeat；
  单连接全量事件（`subscribe(None)`）；慢消费者有界队列溢出即断连；不做事件重放（前端重连后
  拉全量对齐）。
- **auth**（§4）：默认生成 token；`--token <t>` 外部提供；`--no-auth` 关闭（非 loopback 忽略）；
  所有 `/api` 请求 `Authorization: Bearer <token>`；前端 SSE 用 fetch+ReadableStream（EventSource
  不能设 header）。
- **`MagService` trait / `ServiceEvent` / wire 类型**：见
  [`docs/archive/2026-07-20-mag-cli/TODO.md`](docs/archive/2026-07-20-mag-cli/TODO.md) 复用锚点节
  （含 M1 pivot、M2 origin、M3 配置四方法 + ConfigChanged、M4 delegation 事件、M5 ask_user 后的
  完整现状）。
- **前端栈**（Q2 拍板）：React + TypeScript + Vite + Tailwind + shadcn/ui + vitest + Storybook；
  pnpm workspace；状态层自管于 `@mag/client`（不引 Redux）。
- **UI 信息架构**（`docs/WEB.md` §5）：左栏会话导航 / 主区 thread view / 右栏 delegate 栏 /
  底部 composer；交互卡四形态；origin 徽标 `[from <delegate>@depth<n>]`；composer pivot 两层语义
  （409 回落）。

---

## Milestone W1 — service 前置 + ts-rs 管线（`docs/WEB.md` §3/§2.4）

目标：补齐 web 所需的契约面（只加不改），并打通 Rust→TS 类型生成管线。

### W1-1 [DONE] mag-service：Command 补 pivot/配置变体 + `ServiceError::kind`

- **上下文**：`docs/WEB.md` §3 P1/P2。M1/M3 加了 trait 方法与 Event 变体，wire `Command` 未同步。
- **实现要求**：
  - wire `Command`（`crates/mag-service/src/lib.rs`）新增：`PivotMessage{session_id, text}`、
    `GetConfig`、`UpdateConfig{config: ConfigDto}`、`ReloadConfig`、`ApplyConfig`（serde 惯例与现有
    变体一致；`ConfigDto` 已在 mag-service re-export）。
  - `ServiceError` 新增 `kind(&self) -> &'static str`（变体 snake_case tag，供 REST 错误投影；
    手写 match 或 serde 均可，取最小改动）。
  - roundtrip 单测覆盖五个新变体；`kind()` 全变体单测。
  - mag-core Engine 若消费 Command（dispatcher 路径）需同步处理新变体——检查既有 Command 消费点
    并补齐；无消费点则在完成记录说明。
- **验证条件**：`cargo test -p mag-service`；默认验证序列全过。

完成记录（2026-07-21）：

- `Command` 新增 `pivot_message`、`get_config`、`update_config`、`reload_config`、`apply_config` wire 变体，并补 roundtrip/tag 稳定性测试。
- `ServiceError::kind()` 返回与 serde tag 一致的 snake_case kind，覆盖全部 7 个错误变体。
- 已检查 `Command` 消费点：仓内无 `mag_service::Command` dispatcher 消费点；搜索命中均为 `mag-core` 内部 `SessionCommand` 或标准库 `Command`，无需同步处理。
- 验证通过：`cargo fmt --all`、`cargo test -p mag-service`、`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`、`cargo doc --no-deps --workspace`。

### W1-2 [DONE] `get_session_history` + `HistoryEntry`（决策 D5，Q1 拍板）

- **上下文**：`docs/WEB.md` §3 P3。web thread view 必须能全量渲染历史（含 tool call 记录）；
  不用事件重放。
- **实现要求**：
  - mag-service：新增 `HistoryEntry`（`#[non_exhaustive]` 枚举，serde tag）：`UserMessage{text,
    attachments}`、`AssistantMessage{text}`、`ToolCall{trace: ToolTrace}`、`Delegation{trace:
    DelegationTrace}`；`MagService` 新增 `get_session_history(id) -> Result<Vec<HistoryEntry>,
    ServiceError>`（风格同既有方法；DummyService/Engine/测试 fake 同步）。
  - mag-core Engine 实现：从持久化快照/会话存储还原历史，按对话顺序输出；tool trace 取终态；
    delegation trace 还原生命周期终态（已知限制：agent-lib DelegationTrace 只有
    `{delegate,status,usage}`，如实映射）。
  - Command 枚举补 `GetSessionHistory{id}`（Tauri 管道 parity）。
- **验证条件**：聚焦测试：fake LLM 跑一轮含工具调用与委派的对话 → 持久化 → resume/新 Engine
  读取 history，断言变体顺序与内容（user/assistant/tool 终态/delegation）；serde roundtrip。
  默认验证序列全过。

完成记录（2026-07-21）：

- `mag-service` 新增 `HistoryEntry` wire 枚举、`MagService::get_session_history`、`Command::GetSessionHistory{id}`，并补 serde/tag roundtrip 测试。
- `DelegationTrace` 向后兼容新增 `status: DelegationStatusWire` 与可选 `usage`，用于 history 单变体表达委派终态；既有事件映射同步填充 started/finished/failed。
- `mag-core` 新增快照历史投影：通过 `Conversation::restore(snapshot.supervisor)` 读取 committed turns，按对话顺序输出 user/assistant/tool terminal/delegation terminal；普通 tool trace 从 tool-use/tool-result/pairing 还原 input/output/status，delegation 从 `ask_<delegate>` 调用还原终态、task/output/message。
- `Engine::get_session_history` 从持久化 session/snapshot 读取历史；未知 session 返回 `SessionNotFound`，尚无 committed snapshot 的 session 返回空历史。测试 fake/DummyService 已同步新 trait 方法。
- 已检查 `Command` 消费点：仓内仍无 `mag_service::Command` dispatcher 消费点；无需同步执行分发。
- 验证通过：`cargo test -p mag-service`、`cargo test -p mag-core get_session_history_restores_messages_tools_and_delegations_after_restart`、`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`、`cargo doc --no-deps --workspace`。

### W1-3 [DONE] `SessionInfo` 增强（P4）

- **上下文**：`docs/WEB.md` §3 P4。左栏需要标题/时间/状态。
- **实现要求**：`SessionInfo` 补 `#[serde(default)]` 字段：`title`（首条 user message 截断，无则
  空/None）、`last_active_at`（unix 秒或既有时间类型惯例）、`status`（新 wire 枚举
  `SessionStatusWire::{Idle,Running,AwaitingInteraction}`，`#[serde(default)]` = Idle）；mag-core
  在 list_sessions 填充真实值（Engine 内有 run/交互状态）；旧 JSON 兼容单测。
- **验证条件**：`cargo test -p mag-service` + mag-core 聚焦（running/awaiting 状态断言）；
  默认验证序列全过。

完成记录（2026-07-21）：

- `SessionInfo` 向后兼容新增 `title: Option<String>`、`last_active_at: Option<u64>`、`status: SessionStatusWire`，旧 JSON 缺字段默认解码为 `None`/`Idle`；新增 `SessionStatusWire::{Idle,Running,AwaitingInteraction}` snake_case wire 枚举。
- `mag-core` 的 `list_sessions` 以持久化 session 列表为源，填充创建/提交时间作为 `last_active_at`；从最新 committed snapshot 的首条 user message 派生标题；对 live actor 叠加 running/awaiting interaction 状态和未提交首条 user message 标题。
- `SessionManager` 新增只读 runtime metadata 投影，actor 在 run start/terminal/interaction response 时维护活动时间与运行状态，approval pending map 用于判定 `AwaitingInteraction`。
- 测试覆盖：`mag-service` 旧 `SessionInfo` JSON 兼容与新字段 roundtrip；`mag-core` running 状态、awaiting interaction 状态、持久化标题/活动时间断言。
- 验证通过：`cargo fmt --all`、`cargo test -p mag-service`、`cargo test -p mag-core list_sessions_reports_running_status_and_live_title`、`cargo test -p mag-core gated_tool_pauses_then_runs_after_approve`、`cargo test -p mag-core committed_run_persists_a_snapshot_to_the_store`、`cargo clippy --all-targets -- -D warnings`、`cargo fmt --all -- --check`、`cargo test --workspace`、`cargo doc --no-deps --workspace`。

### W1-4 [DONE] ts-rs 生成管线 + `@mag/protocol`

- **上下文**：`docs/WEB.md` §2.4（Q3 拍板 ts-rs）。
- **实现要求**：
  - mag-service（及 mag-config DTO 类型）加 feature-gated `ts-rs` 依赖与 `#[derive(TS)]`（feature
    名如 `ts-export`，默认构建不引入；先验证 `cargo fetch` 可得 ts-rs，离线不可得则记录降级：
    手写生成脚本解析 serde JSON schema 或暂缓自动门禁、类型人工维护 + roundtrip 校验）。
  - 导出机制：`cargo test -p mag-service --features ts-export export_ts`（TS::export_all 惯例）
    或 xtask，产物写入 `ui/packages/protocol/src/`（新建 pnpm 包骨架：`package.json` +
    `tsconfig.json` + `src/index.ts` re-export）。
  - 门禁：脚本/CI 说明「重新生成 + `git diff --exit-code`」；本任务提交首份生成物。
  - 覆盖 §2.4 列出的全部 wire 类型（Command/Event/SessionConfig/UserInput/Interaction*/History*/
    SessionInfo/SourceInfo/ConfigDto 等）。
- **验证条件**：默认验证序列全过；生成物存在且与 Rust roundtrip 测试互证（任一 wire 类型的
  JSON fixture 同时被 Rust serde 与 TS 类型接受——TS 侧可做最小编译期校验）。

完成记录（2026-07-21）：

- `mag-config` 与 `mag-service` 新增默认关闭的 `ts-export` feature；`ts-rs` 依赖仅在该 feature 下启用，默认构建不引入。
- `ConfigDto`/嵌套 DTO、`SecretRef`、`Command`/`Event`/`ServiceEvent`、session/history/interaction/tool/delegation/source/error 等协议类型新增 feature-gated `TS` 派生；ID 新类型导出为 TS `string`，JSON `u64` wire 字段导出为 TS `number`。
- 新增 `cargo test -p mag-service --features ts-export export_ts` 导出测试，重建 `ui/packages/protocol/src/generated/` 与 generated `src/index.ts`；`@mag/protocol` 包骨架含 `package.json`、`tsconfig.json`、README 与漂移门禁说明。
- 首份生成物已提交；`SecretRef` 仅导出 `{ env: string } | { keyring: string }` 引用形态，不导出 secret 值。
- 新增 `update-config-command.json` fixture：Rust 侧通过 `Command` serde roundtrip 校验，TS 侧通过 `satisfies`/编译期 fixture 校验协议类型可接受同形 JSON payload。
- 验证通过：`cargo fetch`、`cargo fmt --all`、`cargo fmt --all -- --check`、`cargo test -p mag-service --features ts-export export_ts`、`cargo test -p mag-service update_config_protocol_fixture_round_trips`、`cargo clippy --all-targets -- -D warnings`、`cargo clippy -p mag-service --features ts-export --all-targets -- -D warnings`、`cargo test --workspace`、`cargo doc --no-deps --workspace`、`npx --yes -p typescript@5.9.2 tsc --noEmit -p ui/packages/protocol/tsconfig.json`。

### W1-R [DONE] W1 review

- **实现要求**：对照 `docs/WEB.md` §3 P1–P4 与 §2.4 逐项核查：契约只加不改（mag-acp/mag-cli 不受
  影响）；HistoryEntry 粒度（含 tool call）；SessionInfo 兼容；ts-rs 管线无手写漂移。发现问题直接
  修复并补测试。
- **验证条件**：默认验证序列全过；完成记录列出 review 结论。

**完成记录（2026-07-21）**：通读 W1-1..W1-4 全部 diff（`149dffb`/`6a1803d`/`137ec20`+`7677250`/
`a168f58`）逐项核查，结论 **W1 放行进入 W2，无 bug 级发现、无需修复项**：

- **P1** ✅：Command 五变体（PivotMessage/GetConfig/UpdateConfig/ReloadConfig/ApplyConfig）serde 惯例
  一致；`ServiceError::kind()` 7 变体全 snake_case 且与 serde tag 互证；仓内无 `mag_service::Command`
  dispatcher 消费点需同步。
- **P3** ✅：`HistoryEntry` `#[non_exhaustive]` 四变体（含 ToolCall/Delegation，Q1 拍板）；Engine
  实现未知 session→SessionNotFound、无快照→空；全链路测试（工具+委派→持久化→跨 Engine 重启还原）
  覆盖顺序/终态/内容。
- **P4** ✅：SessionInfo 三字段全 `#[serde(default)]`，旧 JSON 兼容测试在；list_sessions 填真实值，
  running/awaiting 断言在；时间戳毫秒一致。
- **ts-rs** ✅：feature-gated（默认构建 0 引用，cargo tree 实测）；45 个生成文件覆盖 §2.4 全部 wire
  类型，零手写；重生成零漂移（实测）；fixture 双侧互证 + `tsc --noEmit` 通过。
- **只加不改** ✅：mag-acp/mag-cli 的改动全部是编译适配（新字段默认值/新 trait 方法 stub），两
  interface 测试原样全过。
- **门禁**：fmt/clippy(`-D warnings`)/workspace 29 套件全 ok/doc 全过（唯一 ignored 为既有真二进制
  e2e 骨架）。

**记录在案的偏差（不阻塞，随 W2/W5 跟踪）**：①history `UserMessage.attachments` 恒空——引擎尚不
消费附件，W3 thread view 不应预期附件还原；②`DelegationTrace` 既有事件新增 `status`/`usage` 字段
（兼容方向安全，序列化形状变化知会）；③history/标题投影对受损快照硬错误（单条坏记录可拖垮
`GET /api/sessions`）且标题派生为全量投影——建议 W2/W5 加固（投影容错 + commit 时物化 title）；
④DelegationStatusWire 无 Cancelled/Denied 终态（折叠为 Failed，表达力取舍）；⑤ts-rs 漂移门禁尚无
CI 承载（引入 CI 时列为第一批）；⑥tool 输出为 agent-lib ContentBlock 裸 JSON，wire 稳定输出形态
后续固化。

---

## Milestone W2 — mag-web crate（`docs/WEB.md` §1/§2/§4）

目标：纯协议翻译器——REST 命令面 + SSE 事件面 + token auth + 静态资源 + bin 装配，协议级 e2e 全链路。

### W2-1 [TODO] crate 骨架 + REST 路由 + 错误投影

- **上下文**：`docs/WEB.md` §2.1/§2.3；依赖边界见通用执行规则。
- **实现要求**：
  - 新建 `crates/mag-web`，加入 workspace members；`#![warn(missing_docs)]`。
  - `serve(service: Arc<dyn MagService>, opts)` 入口（opts：host/port/token 策略/静态资源路径）。
  - 实现 §2.1 路由表全部命令路由（`/api/events` 与静态资源除外，后续任务）：请求体反序列化 →
    `MagService` 方法 → JSON 响应；路由层不含任何 agent 逻辑。
  - 错误投影按 §2.3 表：HTTP 状态码 + `{kind, message}`（kind 用 W1-1 的 `ServiceError::kind()`）；
    非 ServiceError 内部错误 500 通用体。
- **验证条件**：聚焦测试：`tower::ServiceExt::oneshot` 内存请求驱动，scripted service 断言每路由的
  方法调用映射、成功响应体、五类错误投影。默认验证序列全过。

### W2-2 [TODO] SSE 事件面

- **上下文**：`docs/WEB.md` §2.2。
- **实现要求**：`GET /api/events` → `text/event-stream`：`subscribe(None)` 事件流 → 帧
  （`event: <type>` + `data: <Event JSON>` + 单调 `id:`）；~15s comment heartbeat（tokio interval
  select）；每连接有界队列，慢消费者溢出即断连（记日志）；多连接各自独立订阅；连接断开清理订阅
  任务。auth 中间件接入点预留（W2-3 启用）。
- **验证条件**：聚焦测试：回环端口起 server，reqwest/自写 SSE 客户端收帧——事件帧格式、heartbeat、
  两个连接都收到广播、scripted service 停发后连接清理。默认验证序列全过。

### W2-3 [TODO] token auth + 静态资源

- **上下文**：`docs/WEB.md` §4（Q6 拍板）与 §10 D4（Q4：debug 目录/release 嵌入）。
- **实现要求**：
  - auth 中间件（axum layer）：`/api/**` 全部要求 `Authorization: Bearer <token>`；静态资源与 `/`
    不要求（token 由前端 fragment 取出后带在 API 请求上）。
  - token 策略三态：默认生成（`Uuid`/随机）；`--token <t>` 外部提供；`--no-auth` 关闭；**非
    loopback host 时忽略 `--no-auth` 并打印警告**。
  - 静态资源：`/` 与 SPA 资源——`cfg(debug_assertions)` 读 `ui/apps/web/dist/` 目录（不存在时返回
    友好占位页提示先 `pnpm build`），release 用 `rust-embed` 嵌入；SPA fallback（非 /api 路径回
    index.html）。
- **验证条件**：聚焦测试：正确/错误/缺失 token 三态；`--no-auth` 直通；非 loopback + `--no-auth`
  被忽略；占位页/静态文件 200；默认验证序列全过。

### W2-4 [TODO] bin `mag --web` + 协议级 e2e

- **上下文**：`docs/WEB.md` §1.3；bin 现有 `--acp`/`--config`（crates/mag/src/main.rs）。
- **实现要求**：
  - bin 新增 `mag --web [--host] [--port] [--token <t>] [--no-auth]`：读配置 → ConfigService →
    `Engine::from_config` → `mag_web::serve`；启动打印访问 URL（含 `#t=<token>`，token 由外部
    提供时打印不含 token 的 URL + 提示）。
  - 协议级 e2e（全离线）：fake LLM 装配 Engine + 回环端口起 server，HTTP 客户端跑全链路——
    建会话 → 发消息收 SSE 流式事件 → 工具审批（收 interaction_requested → POST 响应 → run 继续）
    → pivot（run 中 POST pivot → applied）→ cancel → GET history（含 tool call）→ config
    GET/reload/apply → sources。可分多个测试。
  - mag-acp/CLI 路径回归不破。
- **验证条件**：上述 e2e 全绿；默认验证序列全过。

### W2-R [TODO] W2 review

- **实现要求**：对照 `docs/WEB.md` §2 全节与 §4 检查：路由表与 §2.1 逐条一致；错误投影完整；SSE
  heartbeat/背压/清理；auth 三态 + 非 loopback 强制；依赖边界（`cargo tree -p mag-web` 无
  mag-core/agent-lib/mag-config）；secret 不物化。发现问题直接修复并补测试。
- **验证条件**：默认验证序列全过；完成记录列出 review 结论。

---

## Milestone W3 — 前端核心（`docs/WEB.md` §6 + §5.1/§5.2/§5.4）

目标：`ui/` monorepo 立起来，`@mag/client` 状态层 + `@mag/ui` 核心组件 + app-web 壳打通
「会话列表 + 流式对话 + 审批交互」闭环。

### W3-1 [TODO] `ui/` monorepo 脚手架

- **上下文**：`docs/WEB.md` §6.1；栈 Q2 拍板（React+TS+Vite+Tailwind+shadcn+vitest+Storybook）。
- **实现要求**：pnpm workspace（`ui/pnpm-workspace.yaml`）：`packages/protocol`（接入 W1-4 生成物）、
  `packages/client`、`packages/ui`、`apps/web` 骨架；统一 tsconfig（strict）、eslint/prettier 最小
  配置；各包 `package.json` 依赖方向符合硬约束；`pnpm install` + 各包空 build/test 绿；
  Storybook 骨架在 `@mag/ui`；根 README 简记开发命令（dev/build/test/协议再生成）。
- **验证条件**：`pnpm -r build`、`pnpm -r test` 绿；默认验证序列（cargo 部分不受影响）全过。

### W3-2 [TODO] `@mag/client`：ITransport + HttpSseTransport + SessionStore

- **上下文**：`docs/WEB.md` §6.2（决策 D8：send(Command) 把 REST 映射收敛在 transport 内）。
- **实现要求**：
  - `ITransport`（send/subscribe/kind）；`Command → method+path+body` 映射表（对照 §2.1 全表）。
  - `HttpSseTransport`：fetch POST（Bearer token 注入点）+ fetch+ReadableStream SSE 解析（不用
    EventSource）；错误体 → typed error（kind 字段保留，供 409 pivot 回落）。
  - `SessionStore`：消费 Event 流维护每会话 {消息、工具卡、delegation、pending 交互队列、run 状态、
    pivot 提示}；selector 派生；历史合并（resume/重连先 history 全量替换再叠加增量，RunId/时序
    去重）；断线自动重连 + 全量对齐（list_sessions + 打开会话的 history）。
- **验证条件**：vitest：JSON fixture（scripted Event 流 + history）驱动——流式合并、工具卡状态
  迁移、pending 队列、历史+增量去重、重连对齐、409 typed error。`pnpm -r test` 绿。

### W3-3 [TODO] `@mag/ui` 核心组件 + Storybook

- **上下文**：`docs/WEB.md` §5.2/§5.4/§6.3。
- **实现要求**：纯 props+回调组件：`ThreadView`（消息流 + Markdown 渲染 + 流式增量）、
  `ToolCallCard`（折叠、五状态徽标）、`InteractionCard`（Approval/Question/Choice 三形态 +
  origin 徽标 + 已决只读态）、`Composer`（发送/cancel/pivot 文案切换、pending 提示）、
  `SessionSidebar`（分组列表 + 状态徽标 + New/Sources/Config 入口）。Storybook 覆盖全部视觉态；
  Tailwind + shadcn 落地，设计 token 集中。
- **验证条件**：Storybook 构建绿；组件交互测试（交互卡提交回调载荷正确）；`pnpm -r test` /
  `pnpm -r build` 绿。

### W3-4 [TODO] app-web 壳：对话闭环

- **上下文**：`docs/WEB.md` §5.1/§5.2；W3-2/W3-3 就位。
- **实现要求**：壳装配——fragment token 提取（`#t=`）→ sessionStorage → transport 注入；
  SessionStore 单例；路由（会话视图/Sources/Config 占位）；左栏 + thread view + composer 联通：
  会话列表（含状态徽标）、新建/恢复/删除、流式对话、工具卡、审批交互提交、pivot 两层语义
  （409 回落）、cancel。
- **验证条件**：vitest 壳级测试（mock transport）；与 W2 server 的真实联调脚本（`#[ignore]` 或
  手动说明）；`pnpm -r test`/`pnpm -r build` 绿；默认验证序列全过。

### W3-R [TODO] W3 review

- **实现要求**：对照 `docs/WEB.md` §5/§6 检查：依赖方向（app→ui/client→protocol 单向）；组件不碰
  transport；store 合并逻辑无竞态；token 不进 URL query/日志；Storybook 覆盖度。发现问题直接修复。
- **验证条件**：`pnpm -r test`/`pnpm -r build` 绿 + 默认验证序列；完成记录列出 review 结论。

---

## Milestone W4 — 功能完备（`docs/WEB.md` §5.2–§5.5）

目标：CLI 已验证的全部能力在 web UI 完备呈现。

### W4-1 [TODO] delegation 可视化

- **上下文**：`docs/WEB.md` §5.2/§5.3（origin 归因兑现）。
- **实现要求**：`DelegationCard`（内联：delegate 名/状态/usage）；右栏 delegate 子线程视图
  （origin.delegate 匹配的事件汇聚；工具卡/交互卡带 `[from <delegate>@depth<n>]` 徽标）；右栏可
  折叠；`@mag/client` store 补 delegation 分组 selector。
- **验证条件**：vitest fixture（含两级 delegate 事件流）断言分组与徽标；Storybook 新增视觉态；
  `pnpm -r test`/`pnpm -r build` 绿。

### W4-2 [TODO] pivot/cancel 完备 + run 状态

- **上下文**：`docs/WEB.md` §5.2/§5.4（决策 D6）。
- **实现要求**：`pivot_queued/applied/dropped` 渲染为轻量系统消息；`run_error` 按 `RunErrorKind`
  分类渲染（cancelled/budget/loop 非错误色）；composer 状态机完备（idle/running/pending 交互三态
  文案与按钮）；多会话并行时右栏全局 running 列表。
- **验证条件**：vitest + Storybook 新增态；`pnpm -r test`/`pnpm -r build` 绿。

### W4-3 [TODO] ConfigEditor（文本形态）+ Sources 页

- **上下文**：`docs/WEB.md` §5.5（Q5 拍板：文本形态先行，图形化以后）。
- **实现要求**：`ConfigEditor` 组件：`GET /api/config` → TOML 文本展示/编辑（代码编辑器 textarea，
  语法高亮可选）、保存 `PUT /api/config`、Reload、Apply（按钮旁注明 D2 生效时机）、`config_changed`
  toast；secret 引用原样显示；预留 text/graph mode 插槽（graph 占位「后续版本」）。`SourcesView`：
  表格 + Probe 按钮 + `local_agents_probed` 刷新。
- **验证条件**：vitest（mock transport 断言 PUT 载荷与按钮行为）；Storybook 新增态；
  `pnpm -r test`/`pnpm -r build` 绿。

### W4-R [TODO] W4 review

- **实现要求**：对照 `docs/WEB.md` §5 全节逐项核对 CLI 能力在 web 的呈现覆盖（§0 目标清单）；
  检查 origin 归因、pivot 回落、D2 语义文案。发现问题直接修复。
- **验证条件**：`pnpm -r test`/`pnpm -r build` 绿 + 默认验证序列；完成记录逐项列出 §0 清单结论。

---

## Milestone W5 — e2e 加固（`docs/WEB.md` §8）

### W5-1 [TODO] 端到端加固 + Storybook 补全

- **上下文**：`docs/WEB.md` §8。
- **实现要求**：
  - 协议级 e2e 补场景：SSE 断连重连全量对齐（杀连接→重连→history 对齐无重复无丢失）；多连接
    （双标签模拟）各自收全量事件互不干扰；长 run 中 heartbeat 保活。
  - 前端 e2e（可选 Playwright 或同档——若引入成本高则 scripted store 级覆盖 + 手动联调说明，
    完成记录注明取舍）：建会话→对话→审批→委派→pivot→cancel→config→sources 全路径。
  - Storybook 视觉态补全（错误态/空态/长会话性能基线——大列表虚拟化视需要，不性能过度设计）。
  - 真实浏览器联调 `#[ignore]` 脚本与说明。
- **验证条件**：上述测试全绿；默认验证序列 + `pnpm -r test`/`pnpm -r build` 全绿。

### F-R [TODO] 全计划 review

- **实现要求**：全部里程碑完成后对整轮改动做一次完整 review（可分子代理分块）：对照
  `docs/WEB.md` 全节（决策 D1–D8）逐条核对；重点：冻结契约只加不改、REST 路由表与 §2.1 一致、
  SSE 可靠性（heartbeat/背压/清理）、auth 三态与非 loopback 强制、ts-rs 无手写漂移、依赖边界
  （cargo tree + pnpm 依赖方向）、secret 纪律、离线测试纪律、rustdoc/TSDoc 完整性、§0 目标清单
  逐项达成。发现的问题直接修复并补测试。
- **验证条件**：默认验证序列 + `pnpm -r test`/`pnpm -r build` 全绿；完成记录列出 review 发现与
  修复清单。
