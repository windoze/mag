# mag-web 设计文档（interface #3：web UI，与 desktop 共用 UI 基础设施）

> 前置文档：[`DESIGN.md`](DESIGN.md)（全局架构；§3.0 `MagService` trait；§4 Command/Event wire 编码）、
> [`CLI.md`](CLI.md)（interface #2；决策 D1 pivot 两层语义、D2 配置生效时机、D5 交互 origin 归因——
> web 直接继承这些 service 语义）、[`ACP.md`](ACP.md)（interface #1）。
>
> 本文档是 **mag-web 与 mag-desktop 共享 UI 基础设施**的实现级设计。web 先行，desktop（Tauri）随后；
> 两者是同一套前端代码 + 同一份 wire 类型 + 两个薄壳。
>
> 2026-07-20 开放问题拍板（原 §7 全部关闭，见 §10 决策记录）：历史含 tool call 记录；前端栈
> React+TS+Vite+Tailwind+shadcn；ts-rs 生成 TS 类型；debug 读目录/release 嵌入静态资源；ConfigEditor
> 组件文本形态先行；`--no-auth`/`--token`/默认生成 token；**传输层用 POST+SSE（REST 风格）而非
> WebSocket**——API 整洁优先：方便 API 级 proxy、前后端分离，未来可运行于受控 sandbox/container 经
> 统一 gateway 做 auth 与对外通信。

## 0. 定位与目标

### 定位

mag-web 是 mag 的**第三个 interface**：给**单用户**一个通过浏览器访问本机 mag 的渠道。它不是多用户
服务，不是远程协作平台——它是「本机 agent 的本地控制台」，只不过 UI 渲染在浏览器里。

参照 **codex desktop 的 UI**（见 §5）：左栏会话导航 + 主区 thread view + 底部 composer + 内联交互卡 +
右侧 delegate/progress 栏。这些模式已被证明适合 agent 会话场景，且与我们的事件模型（流式 text、
tool trace、delegation、interaction origin）一一对应。

**web 与 desktop 的关系是本次设计的核心约束**：desktop app 之后会加入本机应用专属功能（桌面通知、
keyring、菜单栏、系统托盘等），但**会话 UI 主体必须是同一份代码**。因此本设计把「UI 基础设施」
（协议类型、客户端状态层、组件库）与「壳」（web SPA / Tauri webview）严格分层，web 阶段就把分层
跑通，desktop 阶段只加壳与专属能力插槽。

**API 整洁是第二条核心约束**（Q7 拍板）：web 层未来可能运行在其他场合——典型场景是 mag 跑在受控
sandbox/container 里，经统一 gateway 做 auth 与对外通信，UI 展现形式也可能不同。因此 HTTP API 必须
REST 风格、语义自明、proxy 友好、前后端严格分离；API 本身即是产品面，不是某个特定前端的私有
后端。UI 基建与 API 面层叠清晰后，换一个 UI（或第三方客户端）只消费同一 API。

### 目标（本里程碑验证清单）

- 浏览器中完成 CLI 已验证的全部能力，且体验符合 codex desktop 的 UI 范式：
  - 会话 CRUD 与恢复（左栏会话列表，含**完整历史渲染：消息 + tool call 记录**，见 §3 P3）
  - 流式对话（TextDelta 增量渲染）
  - 工具调用可视化（ToolStarted/Finished 折叠卡）
  - 用户交互：tool 权限审批、Question/Choice 交互卡，带 origin 归因徽标（决策 D5 继承）
  - 多 agent 编排可视化（Delegation* 事件 → delegate 卡片/右侧栏线程）
  - pivot（run 中发送即 pivot，`NotPivotable` 回落 send_message——与 CLI 相同的两层语义）与 cancel
  - 配置查看/编辑/重载/应用（ConfigEditor 文本形态；`ConfigChanged` 事件 → 界面提示）
  - sources 列表（ListSources/ProbeLocalAgents）
- **REST + SSE 的整洁 API**（§2）：命令面是 REST 路由，事件面是 SSE 流；标准 HTTP 语义，可经普通
  反向代理/gateway 转发与鉴权，无连接升级。
- **同一份前端代码**不经修改地既能跑在 web（HTTP/SSE 传输）也能跑在 desktop（Tauri invoke/emit
  传输）——以传输抽象 `ITransport` 的两个实现证明。
- mag-web crate 是**纯协议翻译器**（与 mag-acp 同一纪律）：HTTP 请求 → `MagService` 方法调用，
  `subscribe` 事件流 → SSE 帧，不含任何 agent 逻辑。

### 非目标

- **多用户/多租户**：无账号体系、无会话隔离、无权限角色。单用户单实例。多用户 auth 由未来外部
  gateway 负责，不在本程序内建。
- **公网远程访问的完整方案**：默认 bind `127.0.0.1`；不内置 TLS 终端（TLS 终止属 gateway/隧道层）。
- **desktop 本机专属功能**：桌面通知、keyring、菜单栏/托盘、全局快捷键、文件关联等属于 desktop 阶段；
  本设计只在 UI 基础设施里**预留能力插槽**（§6.4），不实现。
- **移动端适配**：桌面宽度优先；不做响应式手机布局（不阻止后续加）。
- **富文本/Markdown 渲染之外的产物预览**：文件树、diff 视图、终端模拟器等 desktop 级功能不在本期。
- **多 workspace/多 Engine**：单 Engine 实例服务单 web 实例。
- **WebSocket**：经讨论拍板不采用（§10 D2）——POST+SSE 已覆盖双工需求且更 proxy/REST 友好。

## 1. 总体形态

### 1.1 crate 与依赖边界

```
crates/mag-web/          纯协议翻译器 + 静态资源服务（新 crate）
  依赖：mag-service + axum + tokio + futures + serde/serde_json
  不依赖：mag-core / agent-lib / mag-config        ← 与 mag-acp/mag-cli 同一纪律
  面对：Arc<dyn MagService>（装配 Engine 注入的是上层 bin crates/mag）

ui/                      前端 monorepo（pnpm workspace，与 crates/ 平级）
  packages/protocol/     @mag/protocol —— wire 类型的 TS 声明（§2.4，ts-rs 生成）
  packages/client/       @mag/client   —— session store + ITransport（§6.2）
  packages/ui/           @mag/ui       —— React 组件库（§5/§6.3）
  apps/web/              @mag/app-web  —— web SPA 壳（构建产物由 mag-web 伺服）
  apps/desktop/          @mag/app-desktop —— Tauri 壳（本期只占位，desktop 阶段填充）

crates/mag/src/main.rs   bin 增加 `mag --web [--host] [--port] [--token] [--no-auth]`（§1.3）
```

**依赖方向硬约束**：`@mag/app-web` 与未来的 `@mag/app-desktop` 只依赖 `@mag/ui` + `@mag/client`；
`@mag/ui` 不感知传输；`@mag/client` 不感知渲染；`@mag/protocol` 是唯一与 Rust 侧耦合的包（§2.4 的
类型同步纪律）。

### 1.2 mag-web：纯协议翻译器

axum server 同时做三件事——

1. **REST 命令面**（`/api/...`，§2.1 路由表）：请求 → 翻译成 `MagService` 方法调用 → JSON 响应
   （或错误投影，§2.3）。
2. **SSE 事件面**（`GET /api/events`，§2.2）：`subscribe` 事件流 → SSE 帧广播给所有连接。
3. **静态资源端点**（`/`）：伺服 `@mag/app-web` 的构建产物（debug 读目录 / release 嵌入，§10 D4）。

审批往返依然是试金石：`InteractionRequested{request_id, origin}`（SSE）→ 前端渲染交互卡 → 用户点击
→ `POST /api/sessions/{id}/interactions/{request_id}` → `MagService::respond_interaction` → 唤醒
driver。与 ACP/CLI 复用**同一** gate，mag-core 一行不改。

### 1.3 bin 装配

`mag --web [--host <addr>] [--port <n>] [--token <t>] [--no-auth]`：与 `--cli`/`--acp` 平级的子命令，
**同走配置系统**——读配置 → `ConfigService::load_or_default` → `Engine::from_config` → 注入
`mag_web::serve`。默认 `--host 127.0.0.1 --port <固定默认值>`；启动后打印访问 URL（含 token，
见 §4）。`mag --acp`、`mag`（CLI）行为不变。

## 2. 协议：REST 命令面 + SSE 事件面（决策 D2）

**传输选型（Q7 拍板）**：POST + SSE，不用 WebSocket。理由：标准 HTTP 语义、无连接升级、任何反向
代理/gateway 都能转发与鉴权、curl 可调试、REST 风格自明——API 整洁优先（§0）。双工需求由
「POST 发命令 + SSE 收事件」两条通道覆盖；通道生命周期分裂的代价用 §2.2 的重连对齐规则消化。

**wire 类型不变**：路由的请求/响应体与 SSE 帧载荷复用 `mag-service` 的 wire 类型（`SessionConfig`、
`UserInput`、`InteractionResponseWire`、`Event` 等）——web 管道与 Tauri 管道（invoke/emit）共享的
不变量是 **payload 类型**，传输框架各自 idiomatic（REST+SSE / invoke+emit）。

### 2.1 REST 路由表（命令面）

所有路由前缀 `/api`，JSON 请求/响应体；`{id}` = SessionId，`{rid}` = RequestId。

| 方法 | 路由 | `MagService` 方法 | 响应体 |
|---|---|---|---|
| GET | `/api/sessions` | `list_sessions` | `Vec<SessionInfo>` |
| POST | `/api/sessions` | `create_session`（body: `SessionConfig`） | `{id}` + `SessionConfig` |
| POST | `/api/sessions/{id}/resume` | `resume_session` | 204 |
| DELETE | `/api/sessions/{id}` | `delete_session` | 204 |
| GET | `/api/sessions/{id}/history` | `get_session_history`（§3 P3） | `Vec<HistoryEntry>` |
| POST | `/api/sessions/{id}/messages` | `send_message`（body: `UserInput`） | `{run_id}` |
| POST | `/api/sessions/{id}/pivot` | `pivot_message`（body: `UserInput`） | 204 |
| POST | `/api/sessions/{id}/cancel` | `cancel` | 204 |
| POST | `/api/sessions/{id}/interactions/{rid}` | `respond_interaction`（body: `InteractionResponseWire`） | 204 |
| GET | `/api/sources` | `list_sources` | `Vec<SourceInfo>` |
| POST | `/api/sources/probe` | `probe_local_agents` | `Vec<SourceInfo>` |
| GET | `/api/config` | `get_config` | `ConfigDto` |
| PUT | `/api/config` | `update_config`（body: `ConfigDto`） | 204 |
| POST | `/api/config/reload` | `reload_config` | 204 |
| POST | `/api/config/apply` | `apply_config` | 204 |
| GET | `/api/events` | （SSE 流，§2.2） | `text/event-stream` |

REST 语义说明：会话即资源；`messages`/`pivot`/`cancel`/`interactions`/`resume` 是资源上的动作子路由
（POST 动作语义，避免为了「纯 REST」扭曲 agent 领域模型）；`config` 是单例资源（GET/PUT + 动作）。
gateway 场景下这张表就是对外契约——动词少、路径可预测、无隐藏状态。

### 2.2 SSE 事件面

- `GET /api/events` → `text/event-stream`；每条事件一帧：`event: <snake_case type>` +
  `data: <Event JSON>`（`Event` 是 mag-service 的 wire 事件枚举，`serde(tag="type")`，与 Tauri 管道
  emit 的载荷同型）。
- **单连接全量事件**：`subscribe(None)`，帧内含 `SessionId`（或全局事件无 id），前端按会话分发。
  多标签页 = 多连接，mag-web 为每连接各起一个 `subscribe` 转发任务。
- **heartbeat**：每 ~15s 发一行 comment（`: ping`），防 proxy/gateway  idle 断连。
- **断线重连**：引擎状态在服务端，SSE 断开不影响 run；前端自动重连后**拉全量对齐**（`GET
  /api/sessions` + 各打开会话的 `history`），不依赖事件重放——TextDelta 是纯增量、历史是权威
  （决策 D5）。SSE 帧带单调 `id:` 仅作调试/未来扩展，服务端不维护事件日志、不支持 Last-Event-ID
  补发（重连即全量对齐，语义简单无竞态）。
- **背压**：慢消费者不阻塞引擎——每连接有界队列，溢出则断连（前端重连 + 全量对齐即可恢复）。

### 2.3 错误投影

HTTP 状态码 + JSON 错误体 `{kind, message}`：

| `ServiceError` 变体 | HTTP |
|---|---|
| `SessionNotFound` / `InteractionNotFound` | 404 |
| `NotPivotable` | 409 |
| `InvalidInput` / `Config` | 400 |
| `Unsupported` | 501 |
| `Backend` | 500 |

`kind` 为变体 snake_case tag；message 原样（secret 纪律继承：永不含量化 secret）。前端按 kind 分支
（如 `not_pivotable` → composer 自动回落 POST messages，§5.4）。非 `ServiceError` 的内部错误一律
500 + 通用 message，不泄漏内部细节。

### 2.4 TS 类型同步纪律（决策 D3，Q3 拍板）

用 **ts-rs** 在构建期从 Rust wire 类型生成 TS 声明进 `@mag/protocol`：mag-service（及 mag-config 的
DTO）相关类型加 `#[derive(TS)]`（feature-gated 依赖），`cargo test -p mag-service --features ts-export`
（或专用 xtask）导出 `.ts`，CI/本地门禁 `git diff --exit-code` 防漂移。mag-web 的 serde roundtrip
测试覆盖每个 wire 类型至少一次。**禁止手写 TS 类型**。

## 3. 对 service 主干的前置改动清单

实现 mag-web 前需要回 service 主干补的向后兼容新增（只加方法/变体/字段，与 CLI 阶段同一纪律）：

- **P1：Command 补 pivot 与配置变体**。M1/M3 给 `MagService` 加了 `pivot_message` 与四个配置方法、给
  `Event`/`ServiceEvent` 加了 `Pivot*`/`ConfigChanged`，但 wire `Command` 枚举未同步。补：
  `PivotMessage{session_id, text}`、`GetConfig`、`UpdateConfig{config: ConfigDto}`、`ReloadConfig`、
  `ApplyConfig`。web 管道虽走 REST 路由，Command 枚举仍是 Tauri 管道的调用编码，必须补齐才能两条
  管道语义一致。
- **P2：`ServiceError` 的 kind 投影**。§2.3 的 `error.kind` 需要变体 tag——给 `ServiceError` 加
  `kind(&self) -> &'static str`（或 serde 序列化），最小改动。
- **P3：会话历史查询（决策 D5，Q1 拍板：含 tool call 记录）**。新增
  `MagService::get_session_history(id) -> Result<Vec<HistoryEntry>, ServiceError>`。`HistoryEntry`
  是 `#[non_exhaustive]` 枚举，首版变体：`UserMessage{text, attachments}`、
  `AssistantMessage{text}`、`ToolCall{trace: ToolTrace}`（含终态与输出）、
  `Delegation{trace: DelegationTrace}`。**不用事件重放**——查询语义清晰，前端「先拉历史、再叠加
  增量事件」的合并逻辑简单无竞态；枚举留扩展位（后续可加 pivot 标记、usage 等）。
- **P4：`SessionInfo` 增强**。左栏需要标题/最近活动时间/状态。补 `#[serde(default)]` 字段：
  `title`（首条 user message 截断）、`last_active_at`、`status`（idle/running/awaiting_interaction）。

## 4. 单用户安全模型（Q6 拍板）

- **默认 bind `127.0.0.1`**：只有本机进程能连。这是主安全边界。
- **token 认证（默认开）**：`mag --web` 启动时**默认生成随机 token**并打印一次完整 URL
  （`http://127.0.0.1:<port>/#t=<token>`，fragment 形式——不进 HTTP 请求、不进 access log、
  不被 Referer 泄漏）。前端从 fragment 取出后存 sessionStorage（标签页级），之后**所有 API 请求经
  `Authorization: Bearer <token>` 头发送**。
  - `--token <t>`：使用外部提供的 token 而非程序生成（适配 gateway/自动化场景）。
  - `--no-auth`：显式关闭认证（打印警告；仅供受控环境）。
  - 非 loopback bind（`--host 0.0.0.0` 等）：打印显著警告；token 认证强制开启且忽略 `--no-auth`
    （防误配裸露）。
- **SSE 的 auth**：浏览器原生 `EventSource` 不能设自定义 header——`@mag/client` 的 SSE 用
  **fetch + ReadableStream** 实现（可带 `Authorization` 头），不用 `EventSource`。这样 token 永不进
  URL query（query token 会进 proxy/gateway 日志，不可接受）。
- **CSRF**：Bearer header 认证、无 cookie、无表单端点，CSRF 面近似为零；网关场景同理。
- **gateway 前向兼容**：auth 就是标准 `Authorization` 头——未来统一 gateway 可在上游替换为自己的
  鉴权（mag-web 的 token 校验可配置为关闭/信任上游），API 形态无需变化。
- **secret 纪律继承**：`GET /api/config` 返回的 DTO 里 secret 保持 `{env=...}`/`{keyring=...}` 引用
  形态，前端永不显示解析后的值；`PUT /api/config` 同理只接受引用。

## 5. UI 信息架构（参照 codex desktop）

整体三栏布局，与 codex desktop 一致：**左栏导航 / 主区 thread view / 右栏 delegate 与 progress**
（右栏可折叠）。底部 composer 横贯主区。

### 5.1 左栏（会话导航）

- 顶部固定项：**New chat**、**Sources**、**Config**（对应 CLI 的 `/new`、`/sources`、`/config`）。
- 会话列表：`GET /api/sessions` + `session_created` 事件驱动；按 cwd 分组（codex 的 project 分组
  模式），组内按最近活动排序；每项显示标题（§3 P4）、相对时间、状态徽标（running /
  awaiting-interaction / idle）。
- 恢复即点击：`POST .../resume` → `GET .../history` 渲染 thread view（含 tool call 记录）。
- 删除：条目 hover 菜单 `DELETE /api/sessions/{id}`，二次确认（不可逆操作）。

### 5.2 主区 thread view

- **消息流**：user / assistant 气泡；assistant 消息随 `text_delta` 增量渲染（流式打字效果，
  Markdown 增量渲染）。历史与增量的合并：先 `history` 全量铺底，之后到达的事件按 run/时序追加
  （`@mag/client` 去重，§6.2）。
- **工具调用卡**（折叠）：`tool_started` 插入卡片（spinner + 工具名 + 输入摘要），`tool_finished`
  更新同卡（状态徽标 finished/denied/cancelled/failed + 输出摘要，可展开全量 JSON）。卡片按
  `call_id` 关联。历史中的 `ToolCall` entry 直接渲染为已完成卡。
- **delegate 卡**：`delegation_*` 渲染为内联卡（delegate 名、状态、usage）；点击在**右栏**打开该
  delegate 的子线程视图。子线程内容由 origin 带 delegate 标注的事件汇聚（决策 D5 的 origin 归因在
  此兑现：交互卡与工具卡都标注 `[from <delegate>@depth<n>]`）。
- **交互卡**（最高视觉优先级）：`interaction_requested` 渲染为内联阻塞卡——
  - Approval：工具名 + 输入摘要 + risk/category（Permission 变体）+ 批准/拒绝（+ always）；
  - Question：问题文本 + 输入框 → `Answer{text}`；
  - Choice：选项列表 → `Choice{index}`；
  - origin 非 root 时卡片顶部显示来源徽标。
  提交即 `POST .../interactions/{rid}`；卡片进入只读已决态。**同会话多 pending 交互排队展示**
  （RequestId 不串号，前端按到达顺序排列）。
- **run 状态条**：`run_started/finished/error`（含 `RunErrorKind` 分类渲染）驱动 composer 旁状态；
  `pivot_queued/applied/dropped` 渲染为轻量系统消息。
- **配置变更提示**：`config_changed{revision}` → toast（「配置已更新（r3）；apply 后于各会话下一
  turn 边界生效」——决策 D2/CLI.md 语义原样呈现）。

### 5.3 右栏（delegate/progress，可折叠）

- 当前会话的 delegate 列表与状态；点击切换子线程视图。
- 多会话并行时显示全局 running 会话列表（跨会话跳转）。
- 首版可只实现「delegate 子线程」一节，其余留插槽。

### 5.4 composer

- 文本输入 + 发送按钮；run 进行中发送 = **pivot 两层语义**（与 CLI 一致）：先 `POST .../pivot`，
  收 409/`not_pivotable` 自动回落 `POST .../messages`——用户无感，输入框文案随状态切换
  （「发送」/「插入 pivot…」）。
- run 进行中显示 **cancel** 按钮（`POST .../cancel`）；pending 交互时 composer 不阻塞输入，但提示
  有未决交互。
- 附件：`MessageAttachment` 已在 wire 上，但引擎当前不消费附件（`send_message`/`pivot` 只取文本、
  history 的 attachments 恒空，CLI 同样未消费）。因此首版 UI **不做附件入口**——发出会被引擎静默
  丢弃的附件是死功能；待引擎真正消费附件后再加 UI（`@mag/client` store 与 `MessageBubble` 已具备
  附件载体，届时只需接 composer 入口）。富附件留后。
- `@` 提及 / 斜杠命令：不做——web 的命令面就是 GUI 元素与 REST 路由（决策 D7）。

### 5.5 Config 与 Sources 页（Q5 拍板）

- **Config 页 = `ConfigEditor` 组件**：首版为**文本形态**——`GET /api/config` → DTO → TOML 文本
  展示与编辑（语法高亮的 textarea/代码编辑器即可），保存即 `PUT /api/config`（write-through 落盘
  + 换快照已在 service 侧就绪）+ **Reload**（`POST .../reload`）+ **Apply**（`POST .../apply`，按钮
  旁注明 CLI.md D2 生效时机语义）。secret 引用原样显示/编辑，不物化。**图形化配置表单以后再加入**，
  `ConfigEditor` 组件预留形态切换插槽（text/graph 两个 mode，graph mode 后续实现）。
- **Sources 页**：`GET /api/sources` 表格（name/kind/available/version/capabilities）+ **Probe**
  按钮（`POST /api/sources/probe` → `local_agents_probed` 刷新）。

## 6. 共享 UI 基础设施（web/desktop 共用的核心）

### 6.1 分层

```
@mag/protocol   wire 类型的 TS 声明（ts-rs 构建期生成，§2.4）
@mag/client     ITransport + SessionStore（会话/消息/工具卡/交互/pending 状态机）
@mag/ui         纯 React 组件库（ThreadView/Composer/ToolCallCard/InteractionCard/
                DelegationCard/SessionSidebar/ConfigEditor/SourcesView）
app-web         壳：路由 + HttpSseTransport + 挂载 @mag/ui
app-desktop     壳：Tauri 窗口 + TauriTransport + 挂载同一 @mag/ui（desktop 阶段）
```

### 6.2 `@mag/client`：ITransport + 状态层

```ts
interface ITransport {
  send(cmd: Command): Promise<unknown>;        // web: fetch POST；tauri: invoke
  subscribe(handler: (ev: Event) => void): void; // web: SSE(fetch stream)；tauri: listen
  readonly kind: "web" | "tauri";              // 能力探测的基础（§6.4）
}
class HttpSseTransport implements ITransport { /* REST 路由映射 + fetch-SSE */ }
class TauriTransport   implements ITransport { /* invoke/listen（desktop 阶段） */ }
```

注意 `send` 的参数仍是 **`Command` 枚举的 TS 形态**——REST 路由映射收敛在 `HttpSseTransport`
内部（Command variant → method+path+body 的一张表），上层与组件不感知 HTTP。这样 Tauri 管道
（Command 直接 invoke）与 web 管道对上层完全同构，「一份编码两种管道」在 client 层闭环。

`SessionStore` 是**唯一状态权威**：消费 Event 流维护每会话的 {消息列表、工具卡、delegation、
pending 交互队列、run 状态}，暴露派生 selector。历史合并规则：`resume`/重连后先 `history` 全量
替换，再叠加其后到达的增量事件（`RunId`/时序去重）。**这层与 UI 框架无关**——同时是 web 与
desktop 的 view model，也是主要测试挂载点（scripted Event 流 → store 断言，无需浏览器）。

### 6.3 `@mag/ui`：组件库纪律

- 组件只接 props + 回调，不直接碰 transport；容器组件（在 app 壳里）连接 store。
- Storybook 维护全部组件视觉态：流式中/工具各状态/四种交互卡/delegate 嵌套/pivot 提示/错误态/
  ConfigEditor 文本态。这是 web/desktop 视觉一致性的保障，也是 review 工具。
- 样式：Tailwind + shadcn/ui + 设计 token（颜色/间距/字号集中在 `@mag/ui`），不引第三方主题包——
  desktop 壳只换窗口 chrome，不换 token。

### 6.4 能力插槽（为 desktop 预留）

- `ITransport.kind` + 显式 `Capabilities` 对象（壳在启动时注入：`{desktopNotifications: bool,
  keyring: bool, menubar: bool, ...}`）。web 壳全 false；desktop 壳逐项开。
- 组件按 capability 条件渲染（如 desktop 下出现「turn 完成时桌面通知」开关——后端挂钩就是 M3-5
  已预留的 `add_turn_complete_listener`）。
- **纪律**：desktop 专属功能一律经 capability + 插槽进入，**不 fork 组件**；fork 即设计失败。

## 7. 开放问题（已全部拍板，2026-07-20）

- **Q1 历史粒度** → **含 tool call 记录**（`HistoryEntry::ToolCall{trace}`/`Delegation{trace}`，§3 P3）。
- **Q2 前端栈** → **React + TypeScript + Vite + Tailwind + shadcn/ui**；状态层自管于 `@mag/client`。
- **Q3 TS 类型生成** → **ts-rs**（§2.4）。
- **Q4 静态资源** → **debug build 读 `ui/apps/web/dist/` 目录、release build `rust-embed` 嵌入**
  （`cfg(debug_assertions)` 切换）。
- **Q5 Config 编辑形态** → **预留 `ConfigEditor` 组件，首版文本形态**（TOML 展示/编辑/保存），
  图形化配置以后加入（§5.5）。
- **Q6 token** → **默认生成 token 做 auth；`--token <t>` 外部提供；`--no-auth` 显式关闭**（非
  loopback 下忽略，§4）。
- **Q7 传输** → **POST + SSE（REST 风格）**，不用 WebSocket。理由（拍板）：方便 API 一级 proxy、
  前后端分离、REST 整洁性；未来典型场景是 mag 跑在受控 sandbox/container 经统一 gateway 做 auth
  与对外通信，UI 形态可能不同——API 本身即产品面（§0/§2）。

## 8. 测试策略

- **mag-web（Rust 侧，全离线）**：路由映射/错误投影纯函数单测；handler 级注入 scripted
  `Arc<dyn MagService>`；协议级 e2e 用 axum `tower::ServiceExt` 内存请求 + 本地回环 SSE 客户端 +
  fake LLM 装配 Engine，跑「对话→审批→委派→pivot→cancel→config reload/apply」全链路。真实浏览器
  联调 `#[ignore]`。
- **`@mag/client`**：scripted Event 流（JSON fixture）驱动 SessionStore，vitest 断言状态机
  （含历史合并、pivot 回落、多 pending 交互排队、重连全量对齐）——不依赖浏览器。
- **`@mag/ui`**：Storybook 视觉态 + 组件交互测试（交互卡点击发出正确
  `InteractionResponseWire`）。
- **协议一致性**：ts-rs 生成物 diff 门禁（§2.4）+ mag-web 的 serde roundtrip 全变体覆盖。
- **单测试 < 1 分钟**、卡住即 bug 的纪律继承；前端测试走独立 `pnpm test`，不进 cargo 门禁，
  但纳入完成定义。

## 9. 里程碑概要（PLAN 阶段细化）

- **W1**：service 前置（§3 P1–P4）+ ts-rs 生成管线（`@mag/protocol`）。
- **W2**：mag-web crate（REST 路由 + SSE + auth + 静态资源）+ bin `--web`。
- **W3**：`@mag/client` + `@mag/ui` 核心（thread view/composer/工具卡/交互卡）+ web 壳打通对话闭环。
- **W4**：delegation 可视化 + pivot/cancel + ConfigEditor（文本形态）+ Sources 页。
- **W5**：e2e 加固（重连对齐、多标签页）+ Storybook 完善 + review。
- （desktop 阶段另立计划：Tauri 壳 + TauriTransport + capability 插槽兑现 + 本机专属功能。）

## 10. 决策记录

- **D1**：web/desktop 共用 UI 基础设施以「协议包 + 传输无关状态层 + 组件库 + 双薄壳」分层落地；
  desktop 专属功能只经 capability 插槽进入，不 fork 组件。（§6）
- **D2**（2026-07-20 拍板修订）：web 传输 = **REST 命令面（POST）+ SSE 事件面**，不用 WebSocket；
  REST 路由表即对外 API 契约（§2.1），为 gateway/proxy 场景优化。~~envelope 响应关联~~（WS 方案）
  废弃——HTTP 原生响应替代。
- **D3**：TS 类型构建期从 Rust 生成（ts-rs，Q3 拍板），CI 一致性门禁，禁止手写漂移。（§2.4）
- **D4**（Q4/Q6 拍板修订）：静态资源 debug 读目录、release `rust-embed` 嵌入；单用户安全 =
  loopback bind + **默认生成 token** + `Authorization: Bearer` 头（SSE 用 fetch 实现以携带 header）；
  `--token` 外部提供、`--no-auth` 显式关闭（非 loopback 忽略）；不内置 TLS。（§1.2/§4）
- **D5**：会话历史用新查询命令 `get_session_history`（**含 tool call 与 delegation 记录**，Q1
  拍板），权威全量 + 增量事件叠加；重连对齐同理。（§3 P3、§2.2）
- **D6**：pivot/cancel 在 composer 的交互与 CLI 完全一致（两层语义、`not_pivotable`(409) 自动
  回落），不引入 GUI 独有语义。（§5.4）
- **D7**：web 不做 slash 命令；命令面即 GUI 元素 + REST 路由。CLI 的 slash 集合与 web 的页面/按钮
  一一对应（§5.1/§5.5），两份文档互为映射表。（§5.4）
- **D8**：wire 类型（Command/Event/DTO）是 web 与 Tauri 两条管道共享的不变量；`@mag/client` 的
  `send(Command)` 把 REST 路由映射收敛在 transport 内部，上层与组件不感知 HTTP。（§2/§6.2）
