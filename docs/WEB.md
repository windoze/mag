# mag-web 设计文档（interface #3：web UI，与 desktop 共用 UI 基础设施）

> 前置文档：[`DESIGN.md`](DESIGN.md)（全局架构；§3.0 `MagService` trait；§4 Command/Event wire 编码——
> 本文档落地其 §4.3 的 web 管道）、[`CLI.md`](CLI.md)（interface #2；决策 D1 pivot 两层语义、D2 配置生效
> 时机、D5 交互 origin 归因——web 直接继承这些 service 语义）、[`ACP.md`](ACP.md)（interface #1）。
>
> 本文档是 **mag-web 与 mag-desktop 共享 UI 基础设施**的实现级设计。web 先行，desktop（Tauri）随后；
> 两者是同一套前端代码 + 同一份 Command/Event 编码 + 两个薄壳。

## 0. 定位与目标

### 定位

mag-web 是 mag 的**第三个 interface**：给**单用户**一个通过浏览器访问本机 mag 的渠道。它不是多用户
服务，不是远程协作平台——它是「本机 agent 的本地控制台」，只不过 UI 渲染在浏览器里。

参照 **codex desktop 的 UI**（见 §5）：左栏会话导航 + 主区 thread view + 底部 composer + 内联交互卡 +
右侧 delegate/progress 栏。这些模式已被证明适合 agent 会话场景，且与我们的事件模型（流式 text、
tool trace、delegation、interaction origin）一一对应。

**web 与 desktop 的关系是本次设计的核心约束**：desktop app 之后会加入本机应用专属功能（桌面通知、
keyring、菜单栏、系统托盘等），但**会话 UI 主体必须是同一份代码**。因此本设计把「UI 基础设施」
（协议类型、客户端状态层、组件库）与「壳」（web server 静态页 / Tauri webview）严格分层，web 阶段
就把分层跑通，desktop 阶段只加壳与专属能力插槽。

### 目标（本里程碑验证清单）

- 浏览器中完成 CLI 已验证的全部能力，且体验符合 codex desktop 的 UI 范式：
  - 会话 CRUD 与恢复（左栏会话列表，含历史渲染——CLI 没有的新需求，见 §3）
  - 流式对话（TextDelta 增量渲染）
  - 工具调用可视化（ToolStarted/Finished 折叠卡）
  - 用户交互：tool 权限审批、Question/Choice 交互卡，带 origin 归因徽标（决策 D5 继承）
  - 多 agent 编排可视化（Delegation* 事件 → delegate 卡片/右侧栏线程）
  - pivot（run 中发送即 pivot，`NotPivotable` 回落 send_message——与 CLI 相同的两层语义）与 cancel
  - 配置查看/重载/应用（`/config` 命令的 GUI 形态；`ConfigChanged` 事件 → 界面提示）
  - sources 列表（`ListSources`/`ProbeLocalAgents`）
- **同一份前端代码**不经修改地既能跑在 web（WebSocket 传输）也能跑在 desktop（Tauri invoke/emit
  传输）——以传输抽象 `ITransport` 的两个实现证明（DESIGN.md §4.3 的既定架构落地）。
- mag-web crate 是**纯协议翻译器**（与 mag-acp 同一纪律）：Command JSON → `MagService` 方法调用，
  `subscribe` 事件流 → Event JSON，不含任何 agent 逻辑。

### 非目标

- **多用户/多租户**：无账号体系、无会话隔离、无权限角色。单用户单实例。
- **公网远程访问**：默认 bind `127.0.0.1`；不内置 TLS 终端、不做反向代理指南（用户自配隧道属个人
  行为，安全模型见 §4）。
- **desktop 本机专属功能**：桌面通知、keyring、菜单栏/托盘、全局快捷键、文件关联等属于 desktop 阶段；
  本设计只在 UI 基础设施里**预留能力插槽**（§6.4），不实现。
- **移动端适配**：桌面宽度优先；不做响应式手机布局（不阻止后续加）。
- **富文本/Markdown 渲染之外的产物预览**：文件树、diff 视图、终端模拟器等 desktop 级功能不在本期。
- **多 workspace/多 Engine**：单 Engine 实例服务单 web 实例。

## 1. 总体形态

### 1.1 crate 与依赖边界

```
crates/mag-web/          纯协议翻译器 + 静态资源服务（新 crate）
  依赖：mag-service + axum（或 axum 等价物）+ tokio + futures + serde/serde_json
  不依赖：mag-core / agent-lib / mag-config        ← 与 mag-acp/mag-cli 同一纪律
  面对：Arc<dyn MagService>（装配 Engine 注入的是上层 bin crates/mag）

ui/                      前端 monorepo（npm workspace，与 crates/ 平级）
  packages/protocol/     @mag/protocol —— Command/Event 的 TS 类型（§2.3）
  packages/client/       @mag/client   —— session store + ITransport（§6.2）
  packages/ui/           @mag/ui       —— React 组件库（§5/§6.3）
  apps/web/              @mag/app-web  —— web SPA 壳（构建产物由 mag-web 伺服）
  apps/desktop/          @mag/app-desktop —— Tauri 壳（本期只占位，desktop 阶段填充）

crates/mag/src/main.rs   bin 增加 `mag --web [--host] [--port]`（§1.3）
```

**依赖方向硬约束**：`@mag/app-web` 与未来的 `@mag/app-desktop` 只依赖 `@mag/ui` + `@mag/client`；
`@mag/ui` 不感知传输；`@mag/client` 不感知渲染；`@mag/protocol` 是唯一与 Rust 侧耦合的包（§2.3 的
类型同步纪律）。

### 1.2 mag-web：纯协议翻译器

与 mag-acp 同构：axum server 同时做两件事——

1. **WebSocket 端点**（如 `/ws`）：text frame 进 = Command envelope JSON → 翻译成 `MagService` 方法
   调用；`subscribe` 事件流 → 投影为 Event JSON → text frame 出。
2. **静态资源端点**（`/`）：伺服 `@mag/app-web` 的构建产物（嵌入方式见 §7 开放问题 Q4）。

审批往返依然是试金石（DESIGN.md §4.3）：`InteractionRequested{request_id, origin}` → 前端渲染交互卡
→ 用户点击 → `RespondInteraction{request_id, response}` → `MagService::respond_interaction` → 唤醒
driver。与 ACP/CLI 复用**同一** gate，mag-core 一行不改。

### 1.3 bin 装配

`mag --web [--host <addr>] [--port <n>]`：与 `--cli`/`--acp` 平级的子命令，**同走配置系统**——
读配置 → `ConfigService::load_or_default` → `Engine::from_config` → 注入 `mag_web::serve`。
默认 `--host 127.0.0.1 --port <固定默认值或随机>`；启动后打印访问 URL（含 token，见 §4）。
`mag --acp`、`mag`（CLI）行为不变。

## 2. 协议：Command/Event over WebSocket

### 2.1 一份编码，两种管道（落地 DESIGN.md §4.3）

直接复用 `mag-service` 的 `Command`/`Event` wire 编码（`serde(tag="type", rename_all="snake_case")`，
天然适配 TS discriminated union）。web 管道：WebSocket text frame = Command/Event JSON。desktop 管道
（后续）：`invoke("command", {command})` / `emit("mag://event", event)`。**同一份 JSON，两个传输**。

### 2.2 Command 响应关联：envelope（决策 D2）

现有 `Command` enum 是「方法调用」而非「请求-响应」——`CreateSession` 的 `SessionId`、
`ListSessions` 的列表、`GetConfig` 的 DTO 都需要返回值通道。WebSocket 是多命令并发管道，必须有
关联机制：

```jsonc
// 入站（前端 → mag-web）
{ "id": 41, "command": { "type": "list_sessions" } }
// 出站（mag-web → 前端）：响应
{ "id": 41, "result": [ /* Vec<SessionInfo> */ ] }
{ "id": 41, "error": { "kind": "session_not_found", "message": "..." } }
// 出站：事件（无 id，与响应可区分）
{ "event": { "type": "text_delta", "id": "<session>", "text": "..." } }
```

- `id` 由前端单调铸造（per-connection u64）；envelope 是 mag-web 层概念，**不进 mag-service 契约**
  （`Command` enum 保持纯方法形态，Tauri 管道用 invoke 的原生返回通道，不需要 envelope——
  `@mag/client` 的两个 ITransport 实现各自处理关联）。
- `error.kind` 从 `ServiceError` 的 variant tag 投影（snake_case），message 原样；前端按 kind 分支
  （如 `not_pivotable` → composer 自动回落 send_message，§5.4）。
- 事件帧与响应帧靠顶层键（`event` vs `result`/`error`）区分，前端单帧分发。

### 2.3 TS 类型同步纪律（决策 D3）

`Command`/`Event`/`SessionConfig`/`InteractionKindWire`/`ConfigDto` 等是 Rust 侧定义的契约，TS 侧必须
同步且**不允许手写漂移**。方案：用 `ts-rs`（或 `schemars` + quicktype，二选一，见 §7 Q3）在构建期从
Rust 类型生成 TS 声明进 `@mag/protocol`，CI 加一致性门禁（重新生成后 `git diff --exit-code`）。
mag-web 的 serde roundtrip 测试覆盖每个 Command/Event 变体至少一次。

### 2.4 连接生命周期

- **单连接多会话**：一条 WebSocket 连接接收**全部**会话的事件（`subscribe(None)`），前端按事件里的
  `SessionId` 分发到各 thread view——多会话并行 run 是常态（多标签/多 delegate），不为每个会话开
  连接。`subscribe(Some(id))` 的过滤语义保留给未来需要。
- **断线重连**：引擎状态在服务端，WS 断开不影响 run；前端重连后重新 `list_sessions` + 拉取当前
  会话历史（§3 P3）+ 重新订阅。run 中途断开期间的事件**不重放**——前端以「拉历史对齐」代替
  「补事件」，丢失的增量由全量历史兜底（TextDelta 是纯增量，历史是权威）。
- **多标签页**：同一浏览器的多个标签页是同一单用户的多个连接，各自订阅全量事件——天然支持
  「两个标签页看两个会话」。事件广播由 mag-web 为每个连接各起一个 `subscribe` 转发任务。

## 3. 对 service 主干的前置改动清单

实现 mag-web 前需要回 service 主干补的向后兼容新增（只加方法/变体/字段，与 CLI 阶段同一纪律）：

- **P1：Command 补 pivot 与配置变体**。M1/M3 给 `MagService` 加了 `pivot_message` 与四个配置方法、给
  `Event`/`ServiceEvent` 加了 `Pivot*`/`ConfigChanged`，但 wire `Command` 枚举未同步。补：
  `PivotMessage{session_id, text}`、`GetConfig`、`UpdateConfig{config: ConfigDto}`、`ReloadConfig`、
  `ApplyConfig`。（Tauri 管道 invoke 同样走 Command enum，必须补齐才能两条管道一致。）
- **P2：`ServiceError` 的 wire 投影**。envelope 的 `error.kind` 需要 `ServiceError` 的 variant tag
  （`#[serde(tag)]` 或显式 `kind()` 方法），目前 `ServiceError` 未必 serde 序列化友好——按最小改动
  补（不序列化整个错误，只投影 kind + message）。
- **P3：会话历史查询（web 独有新需求，决策 D5）**。CLI 是线性终端不需要历史回放；web 的 thread
  view 必须在 `resume_session`/重连后渲染完整历史。新增 `MagService::get_session_history(id)
  -> Result<Vec<HistoryEntry>, ServiceError>`（`HistoryEntry` = user/assistant 消息 + tool trace +
  delegation 摘要的归一化只读投影，serde 完整）。**不用事件重放**（把历史伪造成实时事件流）——
  查询语义更清晰，前端「先拉历史、再叠加增量事件」的合并逻辑简单且无竞态。
- **P4（可选）：`SessionInfo` 增强**。左栏需要标题/最近活动时间/状态徽标。若现有 `SessionInfo`
  不足（只有 id/config），补 `last_active_at`/`title`（标题可先取首条 user message 截断）等
  `#[serde(default)]` 字段。

## 4. 单用户安全模型

- **默认 bind `127.0.0.1`**：只有本机进程能连。这是主安全边界。
- **启动 token**（决策 D4）：`mag --web` 启动时生成随机 token，打印一次完整 URL
  （`http://127.0.0.1:<port>/#t=<token>`，fragment 形式——不进 HTTP 请求、不进 access log、
  不被 Referer 泄漏）。前端从 fragment 取出后经 WebSocket 首帧 `hello{token}` 认证，失败即断。
  token 存 sessionStorage（标签页级，不落 localStorage 长期盘）。
- **`--host 0.0.0.0`/非 loopback**：打印显著警告（「任何能到达本机的人都可操作你的 agent」）且
  token 变为强制（loopback 下也不省略——统一路径，减少分支），不内置 TLS（公网暴露属用户自建
  隧道范畴，文档中提示风险即止）。
- **CSRF**：纯 WebSocket + fragment token，无 cookie 认证，无表单端点，CSRF 面近似为零。
- **secret 纪律继承**：`GetConfig` 返回的 DTO 里 secret 保持 `{env=...}`/`{keyring=...}` 引用形态
  （M3 已保证），前端永不显示解析后的值；`UpdateConfig` 同理只接受引用。

## 5. UI 信息架构（参照 codex desktop）

整体三栏布局，与 codex desktop 一致：**左栏导航 / 主区 thread view / 右栏 delegate 与 progress**
（右栏可折叠）。底部 composer 横贯主区。

### 5.1 左栏（会话导航）

- 顶部固定项：**New chat**、**Sources**、**Config**（对应 CLI 的 `/new`、`/sources`、`/config`）。
- 会话列表：`ListSessions` + `SessionCreated` 事件驱动；按 cwd 分组（codex 的 project 分组模式），
  组内按最近活动排序；每项显示标题（首条消息截断，见 §3 P4）、相对时间、状态徽标（running /
  awaiting-approval / idle）。
- 恢复即点击：`ResumeSession` → 拉历史（§3 P3）渲染 thread view。
- 删除：条目右键/hover 菜单 `DeleteSession`，二次确认（不可逆操作）。

### 5.2 主区 thread view

- **消息流**：user / assistant 气泡；assistant 消息随 `TextDelta` 增量渲染（流式打字效果，
  Markdown 增量渲染）。
- **工具调用卡**（折叠）：`ToolStarted` 插入卡片（spinner + 工具名 + 输入摘要），`ToolFinished`
  更新同卡（状态徽标 finished/denied/cancelled/failed + 输出摘要，可展开全量 JSON）。卡片按
  `call_id` 关联——与 codex 的工具折叠卡一致。
- **delegate 卡**：`DelegationStarted/Finished/Failed` 渲染为内联卡（delegate 名、状态、usage）；
  点击在**右栏**打开该 delegate 的子线程视图（codex 的 subagent threads 模式）。子线程内容由
  origin 带 delegate 标注的事件汇聚（决策 D5 的 origin 归因在此兑现：交互卡与工具卡都能标注
  `[from <delegate>@depth<n>]`）。
- **交互卡**（最高视觉优先级）：`InteractionRequested` 渲染为内联阻塞卡——
  - Approval：工具名 + 输入摘要 + risk/category（Permission 变体）+ 批准/拒绝（+ always，映射
    `ApprovalDecisionWire`）；
  - Question：问题文本 + 输入框 → `Answer{text}`；
  - Choice：选项列表 → `Choice{index}`；
  - origin 非 root 时卡片顶部显示来源徽标。
  提交即 `RespondInteraction`；卡片进入只读已决态。**同会话多 pending 交互排队展示**（M2 保证
  RequestId 不串号，前端按到达顺序排列即可）。
- **run 状态条**：`RunStarted`/`RunFinished`/`RunError`（含 `RunErrorKind` 分类渲染：cancelled /
  budget / loop 用非错误色）驱动 composer 旁的状态指示；`PivotQueued/Applied/Dropped` 渲染为一条
  轻量系统消息（「pivot 已排队/已插入/已丢弃：原因」）。
- **配置变更提示**：`ConfigChanged{revision}` → toast（「配置已更新（r3）；apply 后于各会话下一
  turn 边界生效」——决策 D2 语义原样呈现）。

### 5.3 右栏（delegate/progress，可折叠）

- 当前会话的 delegate 列表与状态；点击切换子线程视图。
- 多会话并行时显示全局 running 会话列表（跨会话跳转）。
- 首版可只实现「delegate 子线程」一节，其余留插槽。

### 5.4 composer

- 文本输入 + 发送按钮；run 进行中发送 = **pivot 两层语义**（与 CLI 一致）：先 `PivotMessage`，
  收 `not_pivotable` 错误自动回落 `SendMessage`——用户无感，输入框文案随状态切换（「发送」/
  「插入 pivot…」）。
- run 进行中显示 **cancel** 按钮（`CancelRun`）；pending 交互时 composer 让位于交互卡（不阻塞
  输入，但提示有未决交互）。
- 附件：`MessageAttachment` 已在 wire 上，首版 UI 只做最小文件附加（路径文本），富附件留后。
- `@` 提及 / 斜杠命令：不做——web 的命令面就是 GUI 元素本身（New chat 按钮、Config 页），CLI 的
  slash 命令不照搬。

### 5.5 Config 与 Sources 页

- **Config 页**：当前配置展示（`GetConfig` → DTO → TOML 渲染，secret 引用原样显示）+ **Reload**
  （`ReloadConfig`）+ **Apply**（`ApplyConfig`，按钮旁注明 D2 生效时机语义）。编辑形态（TOML
  文本框 vs 结构化表单）见 §7 Q5；首版可以是只读 + reload/apply 两个动作——更新配置文件本身由
  用户编辑文件 + reload 完成，闭环已成立。
- **Sources 页**：`ListSources` 表格（name/kind/available/version/capabilities）+ **Probe** 按钮
  （`ProbeLocalAgents` → `LocalAgentsProbed` 刷新）。

## 6. 共享 UI 基础设施（web/desktop 共用的核心）

### 6.1 分层

```
@mag/protocol   Command/Event/wire 类型的 TS 声明（构建期从 Rust 生成，§2.3）
@mag/client     ITransport + SessionStore（会话/消息/工具卡/交互/pending 状态机）
@mag/ui         纯 React 组件库（ThreadView/Composer/ToolCallCard/InteractionCard/
                DelegationCard/SessionSidebar/ConfigView/SourcesView）
app-web         壳：路由 + WebSocketTransport + 挂载 @mag/ui
app-desktop     壳：Tauri 窗口 + TauriTransport + 挂载同一 @mag/ui（desktop 阶段）
```

### 6.2 `@mag/client`：ITransport + 状态层

```ts
interface ITransport {
  send(cmd: Command): Promise<unknown>;        // envelope/invoke 关联由各实现内部处理
  subscribe(handler: (ev: Event) => void): void;
  readonly kind: "web" | "tauri";              // 能力探测的基础（§6.4）
}
class WebSocketTransport implements ITransport { /* §2.2 envelope id 关联 */ }
class TauriTransport   implements ITransport { /* invoke 原生返回通道 */ }
```

`SessionStore` 是**唯一状态权威**：消费 Event 流维护每会话的 {消息列表、工具卡、delegation、
pending 交互队列、run 状态}，暴露给组件的只有派生计算（selector）。历史合并规则：`resume`/
重连后先 `get_session_history` 全量替换，再叠加其后到达的增量事件（以 `RunId`/事件顺序去重）。
**这层与 UI 框架无关**——它同时是 web 与 desktop 的「view model」，也是未来测试的主要挂载点
（scripted Event 流 → store 状态断言，无需浏览器）。

### 6.3 `@mag/ui`：组件库纪律

- 组件只接 props + 回调，不直接碰 transport；容器组件（在 app 壳里）连接 store。
- Storybook（或等价物）维护全部组件的视觉态：流式中/工具各状态/四种交互卡/delegate 嵌套/
  pivot 提示/错误态。这是 web/desktop 视觉一致性的保障，也是 review 工具。
- 样式：Tailwind + 设计 token（颜色/间距/字号常量集中在 `@mag/ui`），不引第三方主题包——
  desktop 壳只换窗口 chrome，不换 token。

### 6.4 能力插槽（为 desktop 预留）

- `ITransport.kind` + 显式 `Capabilities` 对象（壳在启动时注入：`{desktopNotifications: bool,
  keyring: bool, menubar: bool, ...}`）。web 壳全 false；desktop 壳逐项开。
- 组件按 capability 条件渲染（如 desktop 下 composer 旁出现「turn 完成时桌面通知」开关——后端
  挂钩就是 M3-5 已预留的 `add_turn_complete_listener`）。
- **纪律**：desktop 专属功能一律经 capability + 插槽进入，**不 fork 组件**；fork 即设计失败。

## 7. 开放问题

- **Q1（P3 历史粒度）**：`HistoryEntry` 要还原到什么程度？方案 a：只还原文本消息（user/assistant），
  工具卡/delegation 不还原（简单，但 resume 后丢失执行痕迹）；方案 b：连 ToolTrace/DelegationTrace
  一起还原（thread view 完整，实现量大）。倾向 **b**，但允许首版 a + 后续补 b（`HistoryEntry`
  是 `#[non_exhaustive]` 枚举，可向后兼容扩展）。
- **Q2（前端栈确认）**：React 18 + TypeScript + Vite + Tailwind + shadcn/ui 作为默认栈（与本机
  webapp-building 惯例一致）；状态层不引 Redux 等重框架（`@mag/client` 自管）。有异议否？
- **Q3（TS 类型生成）**：`ts-rs`（宏标注，生成 .ts）vs `schemars`+quicktype（JSON Schema 中转）。
  倾向 **ts-rs**（步骤少、产物干净），需给 wire 类型加 derive——在 mag-service 内加 feature-gated
  `ts-rs` 依赖可接受否？
- **Q4（静态资源）**：构建产物用 `rust-embed` 编译进 mag 二进制（单文件分发，`mag --web` 即开即用）
  vs 运行时读 `ui/apps/web/dist/`（前端迭代不用重编 Rust）。倾向**两者**：debug build 读目录、
  release build 嵌入（`cfg(debug_assertions)` 切换）。
- **Q5（Config 编辑形态）**：首版只读 + reload/apply（编辑靠用户改文件）是否够？还是首版就要
  TOML 文本编辑 + `UpdateConfig` 写回（write-through 已就绪）？倾向**首版只读**，TOML 编辑第二版，
  结构化表单等 desktop 阶段再议。
- **Q6（token 必要性）**：纯 loopback 场景 token 是否多余？我认为不多余（本机恶意网页/其他用户的
  浏览器也可能打 127.0.0.1，fragment token 成本极低），但若你想极简，可以提供 `--no-auth` 显式
  关闭（打印警告）。要不要这个逃生门？
- **Q7（WS vs SSE+POST）**：WebSocket 双工 vs 「EventSource(SSE) 收事件 + fetch POST 发命令」。
  SSE 方案对代理/调试更友好、无连接升级问题，但两条通道生命周期分裂。倾向 **WebSocket**
  （单连接、与 Tauri emit/invoke 的对偶性更整齐）。有异议否？

## 8. 测试策略

- **mag-web（Rust 侧，全离线）**：映射/envelope 纯函数单测；handler 级注入 scripted
  `Arc<dyn MagService>`；协议级 e2e 用 axum 的内存 WS（`tokio::io::duplex` 或本地回环端口）+
  fake LLM 装配 Engine，跑「对话→审批→委派→pivot→cancel→config reload」全链路。真实浏览器
  联调 `#[ignore]`。
- **`@mag/client`**：scripted Event 流（JSON fixture）驱动 SessionStore，vitest 断言状态机
  （含历史合并、pivot 回落、多 pending 交互排队）——不依赖浏览器。
- **`@mag/ui`**：Storybook 视觉态 + 组件交互测试（交互卡点击发出正确
  `InteractionResponseWire`）。
- **协议一致性**：ts-rs 生成物 diff 门禁（§2.3）+ mag-web 的 serde roundtrip 全变体覆盖。
- **单测试 < 1 分钟**、卡住即 bug 的纪律继承；前端测试不进 `cargo test --workspace` 门禁，
  走独立 `pnpm test`，但纳入完成定义。

## 9. 里程碑概要（ PLAN 阶段细化）

- **W1**：service 前置（§3 P1–P4）+ `@mag/protocol` 生成管线。
- **W2**：mag-web crate（envelope、WS 翻译、静态资源、token）+ bin `--web`。
- **W3**：`@mag/client` + `@mag/ui` 核心（thread view/composer/工具卡/交互卡）+ web 壳打通对话闭环。
- **W4**：delegation 可视化 + pivot/cancel + Config/Sources 页。
- **W5**：e2e 加固 + Storybook 完善 + review。
- （desktop 阶段另立计划：Tauri 壳 + TauriTransport + capability 插槽兑现 + 本机专属功能。）

## 10. 决策记录

- **D1**：web/desktop 共用 UI 基础设施以「协议包 + 传输无关状态层 + 组件库 + 双薄壳」分层落地；
  desktop 专属功能只经 capability 插槽进入，不 fork 组件。（§6）
- **D2**：Command 响应关联用 mag-web 层 envelope `{id, command}` / `{id, result|error}`，不进
  mag-service 契约；Tauri 管道用 invoke 原生返回。（§2.2）
- **D3**：TS 类型构建期从 Rust 生成（ts-rs 倾向），CI 一致性门禁，禁止手写漂移。（§2.3）
- **D4**：单用户安全 = loopback bind + fragment token + WS 首帧认证；非 loopback 强制 token + 显著
  警告；不内置 TLS。（§4）
- **D5**：会话历史用新查询命令 `get_session_history`（权威全量 + 增量事件叠加），不用事件重放；
  重连对齐同理。（§3 P3、§2.4）
- **D6**：pivot/cancel 在 composer 的交互与 CLI 完全一致（两层语义、`not_pivotable` 自动回落），
  不引入 GUI 独有语义。（§5.4）
- **D7**：web 不做 slash 命令；命令面即 GUI 元素。CLI 的 slash 集合与 web 的页面/按钮一一对应
  （§5.1/§5.5），两份文档互为映射表。（§5.4）
