# mag-cli 设计文档（interface #2：最小 CLI，GUI/web 前置验证）

> 上位设计：[`DESIGN.md`](DESIGN.md)（§1 全局架构、§3.0 `MagService`、§10 里程碑）。参照
> [`ACP.md`](ACP.md)（interface #1 的实现级设计，本文档沿用其「纯协议翻译器、不含 agent 逻辑」的
> interface 形态）。
>
> 本文档覆盖两件事：**(a) `mag-cli` crate**——GUI/web 之前的最小 CLI interface，用真实终端回合验证
> `MagService` 的完整能力面；**(b) 运行时配置系统**——CLI 是它第一个消费者，但从第一天起就是
> GUI/web 共用的基础设施，放在 service 主干（mag-core），不放在 CLI 里。
>
> **CLI 的定位是「验证原型」**：确保底层各组件及其间的管线全部通畅、能正常运转。易用性与界面
> 美观不是本阶段的考虑因素；交互模式按「符合 GUI/web 使用方式」的风格做最小实现（例如审批是
> 模态提示、子 agent 交互统一 pop 到 root 会话），使 CLI 验证过的用法可以平移到 GUI/web。

## 0. 定位与目标

### 为什么要有 CLI

按 `DESIGN.md` §10，I1（ACP）用公开规范验证了 `MagService` 的**子集**（turn/stream/permission/
cancel）。GUI/web（I4）要行使的是**近全集**：多会话管理、`Question`/`Choice` 交互、subagent
实例树、pivot、source 运维面、配置读写。直接上 GUI 的风险（`DESIGN.md` §11 risk 6）是这些方法
「有实现但缺端到端验证」，签名到 I4 才发现不合用。CLI 是中间验证层：

- **headless 可脚本化**：和 ACP 一样无需前端工程就能端到端跑真实回合；
- **以人的视角驱动**：ACP 是协议驱动（Zed 是 client），CLI 是**人**驱动——交互提示、会话切换、
  pivot 这些 GUI 的核心 UX 第一次被真实行使；
- **配置系统的第一个消费者**：验证「启动读配置、运行中改配置、合适时机生效」的完整闭环；
- **external agent 管线的最早试炼场**：external ACP subagent（按实例拉起）是本程序的核心功能，
  提前落地、提前暴露问题（见决策 D3）。

### 目标（本里程碑验证清单）

| # | 能力 | 经 CLI 验证的内容 |
|---|---|---|
| 1 | 基本 agent 对话 + 流式输出 | `send_message` + `subscribe` → `TextDelta` 流式打印；`RunFinished`/`RunError` 终态 |
| 2 | 用户交互 | `InteractionRequested` 四种 kind（Approval/Question/Choice/Permission）在终端渲染并应答，`respond_interaction` 唤醒 driver；**子 agent 的交互统一 pop 到 root 会话处理（带来源标注）** |
| 3 | 多 agent 编排 | supervisor 经 `agent` 工具按需 spawn subagent 实例（异步实例模型，**至少涵盖 external ACP agent**）；`AgentInstanceStarted/Finished` 生命周期事件经 service 发出 |
| 4 | agent 间协作 | 实例链中的审批各层受控：子 agent（含 external ACP 实例）的 `InteractionRequested` 带 origin 穿透到 root 会话并可应答 |
| 5 | 会话持久化与恢复 | `list_sessions` / `resume_session` / `delete_session`；进程重启后会话可恢复 |
| 6 | cancel + pivot | run 进行中可 cancel（`RunError{cancelled}`）；run 进行中注入 pivot 消息，下一 step 边界生效；不可 pivot 时自动回落 `send_message`（两层语义，见 §3.2） |
| 7 | **运行时配置系统** | 启动读配置装配 LLM/tool/agent/external-agent；运行中改配置并在合适时机生效（§4） |

### 非目标

- **不做高级 TUI**：无全屏布局、无语法高亮、无鼠标。行编辑 + 历史用 rustyline 即够；输出是
  顺序打印的文本流。易用性/美观不投入。
- **不做 GUI/web 的任何工作**：mag-tauri / mag-server / app 仍属 I4。
- **不改已冻结的 `MagService` 语义**：pivot 与配置是**向后兼容新增**（只加方法/变体/字段），
  `Engine` 真实实现，ACP 侧不受影响。
- **不做配置 GUI 编辑器**：CLI 侧只有最小 slash 命令（`/config show|reload|apply`）。
- **不实现 dispatcher-routed 委派**（`DESIGN.md` §8.2）：multi-agent 走动态 subagent 体系
  （[`dyn-agents.md`](dyn-agents.md)）——单一 `agent` 工具 + 运行时实例化，不合成
  per-delegate 工具。
- **external agent 第一版只接 ACP 方向**（generic `external-acp`）；Claude Code / Codex / OpenCode
  的特化 adapter 不在本里程碑（其二进制探测经 `probe_local_agents` 如实报不可用）。

## 1. 总体形态

### 1.1 crate 与依赖边界

新增 `crates/mag-cli`，与 mag-acp 同形态——**纯 interface adapter**：

```
crates/mag-cli     只依赖 mag-service（+ rustyline / tokio / futures / serde_json）
                   面对 Arc<dyn MagService>，不含 agent 逻辑、不依赖 mag-core/agent-lib
crates/mag (bin)   唯一装配点：读配置 → 构造 mag-core::Engine → Arc<dyn MagService> → 注入 interface
```

子命令分派（`crates/mag/src/main.rs`）：

```
mag                  交互式 CLI（本设计；默认子命令）
mag --resume <id>    恢复指定会话后进入 CLI
mag --agent <name>   新会话绑定的 agent 条目（覆盖 [session].default_agent）
mag --config <path>  指定配置文件（缺省 ~/.config/mag/config.toml）
mag --acp            ACP server over stdio（现有，不动；同样改走配置装配）
```

> 装配顺序：启动时先跑配置系统（§4：读 DTO → 解析为 DO 图 → 装配 SourceRegistry/ToolRegistry/
> Engine），再进 interface。mag-acp 路径同样改走配置装配（目前 `Engine::new()` 空装配是占位）。

### 1.2 双任务结构

CLI 进程内两个任务，靠一个小的 `PromptCoordinator` 协调：

```
stdin ──> input task (rustyline readline) ──> 命令分派 ──> MagService 方法调用
                                                        │  ├─ /slash 命令（本地处理或调 service）
                                                        │  ├─ 纯文本 → pivot 或 send_message（§3.2 两层语义）
                                                        │  └─ 交互应答 → respond_interaction
                                                        ▼
service.subscribe(None) ──> render task ──> stdout（TextDelta 流式、工具/终态摘要）
```

> 图示为全局订阅（`subscribe(None)`）：render task 接收全部会话事件，交互提示经来源标注区分
> 归属会话——这正是目标语义（单一全局交互队列，见下）。

**交互提示协调**（关键小机制）：`InteractionRequested` 到达时 render task 向 input task 发
「请提示用户」信号；input task 用 rustyline 渲染审批/问题选项（编号选择），用户回答后调
`respond_interaction`。**pending 交互是单一全局队列**：凡是 root 会话（含其全部子 agent
实例链）的交互都进同一队列逐一提示，提示行带来源标注（见决策 D5）——与 GUI 的「每 root
会话一个模态审批队列」同构。原设计里「交互期间 render 暂停流式打印」**未实现**（原型已知
限制：流式输出与交互提示可能交错，交互期间靠队列串行应答保证可用；暂停/恢复渲染留待
GUI 阶段设计）。

**Ctrl-C**：run 进行中第一次 = `cancel(session_id)`；无 run 且无活动交互时**忽略（不退出
REPL）**——退出用 `/quit` 或 Ctrl-D（会话已持久化，退出不丢数据；`/quit`/Ctrl-D 会对仍在
飞行的 run 主动发 cancel 后退出，不无限等待自然结束）。

## 2. 命令面（slash commands）

最小集，全部解析在 mag-cli 本地，多数一一对应 `MagService` 方法：

| 命令 | 行为 | 验证点 |
|---|---|---|
| 纯文本 | run 中 → `pivot_message`（失败自动回落 `send_message`）；空闲 → `send_message` | 1, 6 |
| `/new [agent]` | `create_session`（用当前配置快照，可选指定 agent 定义）并切换 | 7 |
| `/sessions` | `list_sessions` 打印（id / provider / model / 创建时间） | 5 |
| `/resume <id>` | `resume_session` 并切换 | 5 |
| `/delete <id>` | `delete_session` | 5 |
| `/cancel` | `cancel` 当前会话 | 6 |
| `/sources` | `list_sources` + `probe_local_agents` 打印 | 3 |
| `/config show` | `get_config` 打印当前生效配置 TOML（脱敏）；不含 revision（**未实现**——service 层缺 revision 暴露，后续版本补） | 7 |
| `/config reload` | `reload_config` 显式重载；无差异摘要（**未实现**——service 层缺 revision 暴露，后续版本补） | 7 |
| `/config apply` | **全局无参 apply**：捕获当前快照并应用到所有活会话（各会话在自身闲置点/turn 边界落地，§4.4） | 7 |
| `/help`、`/quit` | 本地 | — |

## 3. pivot 与 cancel：MagService 的新增能力

### 3.1 现状与缺口

- cancel：已实现（`stream_with_cancel` + 旁路 CancelHandle，`RunErrorKind::Cancelled`）。CLI 只是
  第一个真实行使它的 interface（ACP 的 `session/cancel` 也走它）。
- pivot：agent-lib facade 已有 `AgentRunStream::interject()`，但**只在 step 边界窗口内接受**
  （`tool_result` 闭合后、下一次 LLM 调用前；不在窗口返回 `FacadeError::InvalidState`，同窗口重复
  注入也报错）。mag-core driver 目前**无 pivot 旁路**：`run_turn` 独占 `stream` 的 `&mut`，
  到达的 pivot 无处安放。`MagService` 也无对应方法。

### 3.2 设计：两层语义（决策 D1）

**第一层（service 原语，语义严格）**：`pivot_message` 只对**进行中的 turn** 注入 pivot；
会话没有 in-progress turn 时**失败**，不做任何隐式转换：

```rust
// MagService 新增方法
async fn pivot_message(&self, id: SessionId, input: UserInput) -> Result<(), ServiceError>;
// 无 in-progress turn → Err(ServiceError::NotPivotable { id })   // 新错误变体，向后兼容

// ServiceEvent 新增变体
PivotQueued  { id: SessionId, text: String }        // 已受理排队
PivotApplied { id: SessionId, text: String }        // 已在 step 边界注入
PivotDropped { id: SessionId, text: String, reason: String }  // run 结束仍未落地
```

mag-core 实现（与 cancel 同构的旁路，不碰 agent `&mut`）：

1. driver actor 持 `pivot_queue: VecDeque<String>`（共享句柄，类似 CancelHandle）。
   `pivot_message` 在有活动 run 时入队并发 `PivotQueued`；无活动 run 返回 `NotPivotable`。
2. `run_turn` 的事件循环在**每次 poll 到事件后**尝试把队首 pivot `stream.interject(..)`：
   - 成功 → 出队，发 `PivotApplied`；
   - `InvalidState`（不在边界窗口/窗口已被占）→ 留在队首，下次再试；
   - 其它错误 → 出队并发 `PivotDropped`。
3. run 结束（完成/失败/cancel）时队列非空 → 逐条 `PivotDropped{reason}`。service 层语义到此
   为止：**不自动转 `send_message`**（是否转换是调用方决策）。

**第二层（interface 便利语义，自动回落）**：CLI 输入分派把「不能 pivot 就发新消息」做成
用户无感的回落——

```
用户键入文本
  ├─ 当前无活动 run        → send_message
  └─ 当前有活动 run        → pivot_message
        └─ 竞态：调用时 run 刚好结束（NotPivotable）→ 自动改调 send_message
```

这样 service 层保持严格语义（pivot 就是 pivot，失败如实报），用户体验上「任何时候打字都会
被送达」。GUI/web 复用同一两层模型：service 原语不变，前端在输入框层做同样的回落。

> 依据 agent-lib `docs/agent-layer.md` §4.1：pivot 是「下一个 step 边界生效的消息注入」，不违反
> tool 配对、不含 reconfig。reconfig（换 model/tools/system）只在 turn 边界——这正是配置生效
> 时机的约束来源（§4.4）。

### 3.3 `agent` 工具与子 agent 交互统一 pop 到 root 会话（决策 D5）

动态 subagent 体系（[`dyn-agents.md`](dyn-agents.md)，已实现）下，supervisor 经三个统一工具
管理 subagent 实例——定义与实例分离：类型来自 `AgentDefinition` 注册表（§4.2），实例即用
即抛、不跨会话持久化：

| 工具 | 语义 |
|---|---|
| `agent(type, task, description?)` | **异步 spawn**，立即返回 `{ id, status: "running" }`（`id` 形如 `explorer-1`，按类型计数）；实例并发推进，不阻塞 supervisor 的 run |
| `agent_result(id, timeout?)` | **阻塞**等待完成（带超时参数），返回最终报告；超时返回 `{"status": "running"}`，可继续等待 |
| `agent_cancel(id)` | 取消运行中的实例 |

- 实例生命周期经 `ServiceEvent::AgentInstanceStarted/Finished` 发出（status:
  Completed/Failed/Cancelled + 报告/错误）；完成时另向 supervisor 会话**推送完成通知**
  （run 中经 pivot 通道、run 空闲缓冲进下一次输入前缀，见 `dyn-agents.md` §5.3）。
- root 会话 cancel **级联**取消仍在运行的全部实例；阻塞中的 `agent_result` 被抢占返回。
- spawn 审批走 `[tools.agent]` tier（`approval = "ask" | "allow" | "deny"`，同普通工具）。
- 实例链中子 agent（local 实例 / external ACP 实例）的审批与提问**不经独立通道**，统一
  作为 root 会话的 `InteractionRequested` 发出，由 root 会话的界面（CLI 的单一交互队列、
  未来的 GUI 模态框）处理：
  - mag-core 侧：实例的交互经 origin 路由器汇入 root 会话的审批 gate，并在事件上标注
    **来源**：`ServiceEvent::InteractionRequested` 向后兼容新增字段
    `#[serde(default)] origin: InteractionOrigin`（`{ delegate: Option<String>, depth: u32 }`，
    实例交互的 `delegate` = 实例 id、`depth` = 嵌套深度；缺省 = root agent 本人）。子
    agent 的 Permission 类交互本就有 `actor` 字段，origin 与其互补（actor 是权限语义
    主体，origin 是渲染归属）。
  - CLI 侧：pending 交互单一队列（§1.2），来源以 `[from <实例id>@depth<n>]` 前缀标注
    （如 `[from general-purpose-1@depth1] [approval] …`）；应答仍是对 root 会话调
    `respond_interaction`，由 mag-core 路由回正确的 pending oneshot。
  - 其它会话（非当前 root 的实例链）到达的交互：CLI 只打印「会话 X 有待应答交互」
    提示，`/resume` 切过去后处理——不引入「后台应答」复杂度。

## 4. 运行时配置系统（重点）

### 4.1 设计约束

1. **GUI 共用**：配置系统是 service 主干能力，CLI 只是第一个消费者。读写都经 `MagService`，
   GUI 接入时不加新方法。
2. **「配置」与「配置文件」解耦**（决策 D4）：配置的数据形态分两层——**DTO** 对应配置文件
   （或任何配置来源：GUI 设置面板、环境变量、未来的远端下发）的 serde 镜像；**DO** 对应程序
   实际使用的运行时配置对象。两者双向转换，运行时只依赖 DO。
3. **DO 不是简单嵌套 struct**：由于动态生效需求（旧会话钉住旧配置、新会话用新配置，两版配置
   同时存活），DO 是**引用计数的关联对象树/森林**（`Arc` 共享）。「钉住」不需要复制——会话持有
   `Arc<ConfigSnapshot>`，旧图随引用自然存活，新图换根即生效。
4. **secret 与配置分离**：DTO 只存**引用**（env 变量名 / keyring 条目名），secret 本体仍走
   `CredentialStore`（`DESIGN.md` §3.5）；DO 里同样是引用句柄，取值发生在装配 `ProviderConfig`
   的最后一刻。配置文件可以安全进 dotfiles 仓库。
5. **可校验、可诊断**：坏配置不静默——DTO→DO 解析报行级错误，且**不炸掉正在运行的进程**
   （沿用上一份好配置）。

### 4.2 DTO ↔ DO 双向转换

新增 `crates/mag-config`：纯数据 crate（DTO serde 类型 + 校验 + DTO↔DO 转换 + TOML 读写），
不依赖 mag-core。

```
ConfigFileDto (serde, TOML 镜像)                ResolvedConfig / ConfigSnapshot (DO)
├─ providers: HashMap<String, ProviderDto>      ├─ providers: HashMap<String, Arc<ResolvedProvider>>
├─ agents:    HashMap<String, AgentDto>    ⇄    ├─ agents:    HashMap<String, Arc<ResolvedAgent>>
├─ external_agents: HashMap<String, ExtDto>     │   （ResolvedAgent 持有 Arc<ResolvedProvider> 等
├─ tools:     HashMap<String, ToolDto>          │    交叉引用 → 对象树/森林，而非嵌套值）
└─ session:   SessionDefaultsDto                ├─ external_agents, tools, session_defaults
                                                └─ revision: u64（装配时打戳）
```

- **`agents`/`external_agents` 的非绑定项 = subagent 定义来源**（动态 subagent 体系，
  [`dyn-agents.md`](dyn-agents.md) §3.2）：不再在会话建时静态注册 delegate，而是经 TOML
  投影进 `AgentDefinition` 注册表，供 `agent` 工具在运行时实例化（§3.3）；被
  `[session].default_agent` 绑定的 entry 仍是 supervisor 自身的装配，不属于注册表。
- **DTO→DO（resolve）**：校验（未知字段警告、悬空 provider 引用报错、approval 枚举合法）→
  按拓扑顺序实例化 `Arc` 对象（provider 先于引用它的 agent）→ 打 revision 戳。任何一步失败
  整体失败，不产出半个图。
- **DO→DTO（project）**：write-through 时把 DO 投影回 DTO（secret 引用原样保留，不接触值）→
  序列化 TOML 原子写（tmp + rename）。双向转换对称，round-trip 测试保证不丢字段。
- **DTO 与来源无关**：同一份 DTO 类型服务文件、GUI patch、测试构造；文件只是 DTO 的一种
  持久化。第一版只有 user 级一个文件 + `--config` 覆盖（决策 D4：分层推迟）。

```toml
# ~/.config/mag/config.toml（DTO 的 TOML 形态）
[providers.anthropic]
wire = "anthropic"
base_url = "https://api.anthropic.com"
api_key = { env = "ANTHROPIC_API_KEY" }     # secret 引用，不落盘

[providers.local_proxy]
wire = "openai"
base_url = "http://127.0.0.1:8317"
api_key = { keyring = "mag/local_proxy" }

[agents.default]                            # 会话绑定 entry（[session].default_agent）：supervisor 自身装配
provider = "anthropic"
model = "claude-sonnet-4-5"
tools = ["read_file", "list_dir", "grep", "shell", "ask_user"]

[agents.reviewer]                           # 非绑定项 → subagent 定义（TOML 来源，dyn-agents.md §3.2）
role = "只读审查 subagent"                   # role → 定义 description（出现在 agent 工具描述中）
model = "claude-haiku-4-5"                  # 实例 model 覆盖；缺省继承 supervisor，仅限同 provider
system_prompt = "审查改动，只报问题不改代码。" # → 定义正文（分层 prompt 第二层）
tools = ["read_file", "grep"]               # 与 supervisor 工具面取交集（只窄不宽）

[external_agents.peer_acp]                  # external ACP subagent 定义（kind: acp，按实例拉起进程）
kind = "acp"
command = ["peer-agent", "--acp"]           # spawn 命令行；工作目录隔离由 agent-lib 负责

[tools.shell]
approval = "ask"                            # ask | allow | deny（→ ApprovalPolicy 映射）

[session]
routing = "model_routed"
budget = { max_tokens = 200000 }
```

启动装配（bin 侧）：读文件 → DTO → resolve 成 DO 图 → 从 DO 图装配 `SourceRegistry` /
`ToolRegistry` / 默认 `SessionConfig` → `Engine::from_config(..)`（新构造器，取代 `Engine::new()`
占位装配）。装配失败给行级诊断并非零退出，不静默降级。

**subagent 定义文件（markdown）**：TOML 之外，subagent 定义更推荐以规范 markdown 文件维护
（[`dyn-agents.md`](dyn-agents.md) §3.1，YAML frontmatter + **正文即 prompt**）——
`~/.config/mag/agents/*.md`（用户级）与 `<项目>/.mag/agents/*.md`（项目级）：

| 字段 | 必选 | 说明 |
|---|---|---|
| `name` | 否 | 缺省取文件名（去 `.md`） |
| `description` | 是 | 出现在 `agent` 工具描述中，model 据此选类型；同时用于 UI |
| `kind` | 否 | `local`（缺省）或 `acp` |
| `tools` | 否 | 仅 local。逗号分隔；缺省继承 supervisor 工具面，显式给出时取交集（只窄不宽） |
| `model` | 否 | 仅 local。缺省继承 supervisor 的 model；仅限同 provider 的 model |
| `max_steps` | 否 | 仅 local。步数预算上限；缺省用运行时默认 |
| `command` / `env` | external 必选 / 否 | 仅 `kind: acp`；spawn 命令行（argv 形式）与额外环境变量 |

同名定义四来源优先级（高 → 低）：**TOML > 项目级 > 用户级 > 内置**；内置提供
`general-purpose`（零配置兜底，`agent` 工具 `type` 的缺省值）与 `explorer`（只读探查）。
定义注册表在会话创建时装配、`apply_config` 时重建（两个 markdown 目录层一并重读），只影响
之后的 spawn（§4.4）。

### 4.3 运行时模型：ConfigService（mag-core 内）

```
ConfigService {
    current: RwLock<Arc<ConfigSnapshot>>,   // 当前生效 DO 图（换根 = 换配置）
    revision: AtomicU64,                    // 单调递增，每次成功应用 +1
    path: PathBuf,                          // write-through 目标文件
    watcher / 显式 reload 入口,
    turn_complete: TurnCompleteHub,         // 见 §4.5
}
```

- **三条更新入口**：(a) 文件 watch（`notify` crate，debounce 后自动 reload）；(b) interface 显式
  触发（CLI `/config reload`，GUI 的「重新加载」按钮）；(c) interface 程序化修改
  （`update_config`，GUI 设置面板的写路径）。三条入口殊途同归：得到 DTO → resolve 成新 DO 图
  → 校验通过 → `current` 换根、revision+1。
- **写穿（write-through）**：`update_config` 的 DTO 先落盘（DO→DTO→TOML 原子写）再换根。
  文件与内存始终一致；自身写入触发的 watch 事件按 revision 去重。
- **失败语义**：resolve/校验失败 → current 不动，经事件与返回值报错；正在跑的会话零影响。
- **变更通知**：`ServiceEvent::ConfigChanged { revision, summary }`（全局事件，`session_id=None`），
  interface 据此刷新 source 列表 / 提示「新配置将在新会话生效」。

`MagService` 向后兼容新增：

```rust
async fn get_config(&self) -> Result<ConfigView, ServiceError>;              // 脱敏 DTO 视图 + revision
async fn update_config(&self, patch: ConfigPatch) -> Result<u64, ServiceError>;  // 返回新 revision
async fn reload_config(&self) -> Result<u64, ServiceError>;                  // 显式重载文件
async fn apply_config(&self) -> Result<(), ServiceError>;                    // 全局无参 apply（§4.4）
```

> `apply_config` 的权威语义是**全局无参 apply**：trait 不带 `SessionId`，不写「应用到当前
> 会话」。实现采用世代/pending 机制——调用时捕获当前 `Arc<ConfigSnapshot>` 并 bump 共享
> `generation`（pending 标记）；每个会话 actor 记录自己已应用的 generation，观察到更新的
> generation 时落地捕获的那份快照：空闲会话立即（服务 `ApplyConfig` 命令时），有 run 在飞
> 的会话在 run 终态后的 turn 边界落地。请求时捕获快照保证 apply 落地前发生的新
> reload/update 不会悄悄改变这次 apply 要应用的内容。

`ConfigView` 是脱敏投影（secret 引用显示为 `{env = "..."}` 字样，永不显示值）。`ConfigPatch`
第一版是最小结构：整段替换某个 provider/agent/external_agent/tool 节，或设置 session 缺省；
不做通用 JSON-merge-patch。

### 4.4 生效时机（决策 D2：钉住 + 显式 apply）

约束来自 agent-lib（`docs/agent-layer.md` §4.2）：**reconfig 只在 turn 边界**；turn 内工具集/
model/system 恒定。再叠加快照一致性：会话的 provider/工具装配在 restore 时重建。因此采用
**「会话创建时钉住 + 显式应用到 turn 边界」**：

| 配置类别 | 生效时机 |
|---|---|
| providers（来源增删改） | **新会话**立即用新 DO 图；既有会话钉住创建时的 `Arc<ConfigSnapshot>`（持引用即钉住，零拷贝） |
| agents.* 绑定项（supervisor 的 model/tools/system_prompt） | 同上钉住；`apply_config`（全局无参）把 current 图**排队到各会话下一 turn 边界/闲置点**应用（覆盖 model/tools/system_prompt） |
| subagent 定义（agents.* 非绑定项 / external_agents.* / 两个 markdown 目录） | 定义注册表随会话创建装配、随 `apply_config` 重建（TOML 层重投影、目录层重读）；**只影响之后的 spawn**，运行中实例钉住 spawn 时的上下文 |
| tools.*.approval（审批策略） | **仅会话（重）建时生效**：`ApprovalPolicy` 在 facade agent build 时烤死（agent-lib 无 reconfigure 变体），改审批策略对既有会话不生效；`apply_config` 也不覆盖它 |
| session 缺省（routing/budget/default_agent） | 只影响新会话 |

- 会话钉住的实现：`create_session` 时取 `current.clone()`（`Arc`）存入会话状态；restore 时
  快照中的会话 config 决定重建哪一版装配（持久化的是 DTO 形态的会话配置，恢复时 resolve 成
  DO——若引用的 provider 在新图里已删，报明确错误并保留会话数据）。
- turn 边界应用：`apply_config` 捕获当前快照并 bump 共享 generation（pending 标记，§4.3）；
  各会话 actor 观察到更新 generation 时按捕获快照 reconfig facade `Agent`（覆盖 model/
  tools/system_prompt，并重建 subagent 定义注册表；审批策略不在 reconfig 覆盖范围内，见
  上表）。run 进行中到达的 apply 一律排队到 run 终态后的 turn 边界。应用动作挂在 §4.5 的
  turn-complete 钩子上执行。
- 时机可见性（`/config show` 显示全局 revision 与当前会话钉住的 revision、
  `ConfigChanged{summary}` 说明影响范围）**未实现**（service 层缺 revision 暴露），后续
  版本补。自动应用（改配置即影响所有空闲会话）第一版不做，GUI 阶段再评估（决策 D2 记录）。

### 4.5 turn-complete 通用通知/回调机制（决策 D2 附带）

mag-core 增加一个 interface 无关的 **turn-complete 钩子点**：driver 在每次 run 到达 committed
一致点（成功快照落库后；cancel/失败归位后）发出一次内部通知：

```rust
// mag-core 内部（不经 MagService wire；interface 侧观察仍走 ServiceEvent）
trait TurnCompleteListener: Send + Sync {
    async fn on_turn_complete(&self, ctx: TurnCompleteContext);
    // ctx: session_id / outcome(committed|cancelled|failed) / 会话钉住的 config revision / 全局 revision
}
```

- **第一消费者**：配置 apply——`apply_config` 置 pending 后，listener 在 turn 边界执行重建；
  无 run 的会话由 `apply_config` 直接触发一次检查（无需等下一个 turn 才开始排队）。
- **后续消费者**（本里程碑不实现，机制就位）：桌面通知（run 完成提醒）、usage 统计落库、
  会话标题自动生成等。listener 注册在 Engine 装配处（bin），interface 不可见。
- 失败隔离：listener  panic/错误只记日志，绝不影响 driver 状态。

### 4.6 与既有模块的关系

- `mag-sources`：`SourceRegistry`/`CredentialStore` 不动；DO 图是它的装配输入。
- `mag-tools`：registry 不动；DO 图决定注册哪些 plugin 及各自 approval 覆盖。
- `mag-acp`：不经 `ConfigView`（ACP 无此概念）；bin 装配改走配置后 ACP server 自然使用同一份
  配置。`ConfigChanged` 对 ACP 无害（它不映射该变体）。
- 未来 GUI：设置面板 = `get_config`/`update_config` 的渲染器，零新增 service 方法；桌面通知
  挂 §4.5 listener。

## 5. 对 service 主干的前置改动清单

按仓库规则（向后兼容：只加方法/变体/字段），CLI 落地前需这些前置任务：

| # | 前置项 | 位置 | 说明 |
|---|---|---|---|
| P1 | `pivot_message` + `NotPivotable` 错误 + `Pivot*` 事件 + driver pivot 队列 | mag-service / mag-core | §3.2 |
| P2 | `InteractionRequested.origin` 字段 + 子 agent 审批共用 root gate | mag-service / mag-core | §3.3 |
| P3 | `mag-config` crate（DTO/DO + 双向转换 + TOML IO）+ `ConfigService` + 四个 config 方法 + `ConfigChanged` 事件 | 新 crate / mag-service / mag-core | §4 |
| P4 | turn-complete listener 机制 | mag-core | §4.5 |
| P5 | `Engine::from_config` 装配构造器 | mag-core / bin | §4.2 |
| P6 | `ask_user` 工具（AskUserQuestion 最小版，普通 `ToolPlugin`） | mag-tools | 决策 D6 |
| P7 | 动态 subagent 体系：统一 `agent`/`agent_result`/`agent_cancel` 工具 + 实例注册表 + 完成通知，实例类型含 **external ACP agent** | mag-core / mag-config | 决策 D3；设计见 [`dyn-agents.md`](dyn-agents.md)（已实现）；`Delegation*` wire 变体保留作兼容遗留 |

> P2 的子 agent 交互路由在动态 subagent 体系中由 mag 侧 origin 路由器实现（§3.3），不再
> 依赖 agent-lib A1 的完整形态；P3 的 `apply_config` 依赖 agent-lib A2（facade
> reconfigure）。见下节。

## 5A. agent-lib 前置需求清单（2026-07-20 评估结论）

对 agent-lib（`../agent-lib`，评估时 `@0add094`）逐 API 核对后的结论：**需要改动**——
2 项硬阻塞（A1/A2）、2 项强烈建议（A3/A4）、若干可选项。§D 列出评估确认**无需改动**的部分
（设计可行性已验证）。

> **2026-07-22 更新**：动态 subagent 体系（[`dyn-agents.md`](dyn-agents.md)，已实现）落地
> 后，本节与静态委派相关的条目已被取代或退役：A1（子 agent 交互路由）由 mag 侧 origin
> 路由器实现；A3（external cancel 传导）由 `run_external_once` 一次性调用 + cancel token
> 桥接覆盖；A6（`Delegation*` 事件）、A7（delegate child 工具面）、A9（`ask_<name>`
> 审批绕法）、A10 的 DelegationTrace 项、§D 的 delegation 与「restore 需重注册
> delegate」条目随静态委派一并退役——agent-lib 实际只新增 `run_external_once` 一次性
> 调用面（TODO.md M1）。A2（facade reconfigure）、A4/A5（cancel 语义）、A8（pivot 窗口
> 可见性）与 §D 其余条目仍然有效。

### A. 硬阻塞（不做则本里程碑核心目标无法达成）

**A1. 委派子 agent 的交互路由到父级异步 `InteractionHandler`（带来源标注）**

- 现状：local subagent 的子作用域用 worker 自带 `ApprovalPolicy` 新建**同步** `FacadeApproval`
  （`facade/delegate.rs:1465`），headless 下 ask 即 deny；external ACP agent 的
  `session/request_permission` 经 `NeedInteraction` pop 到 `EmptyExternalScope`
  （`facade/external.rs:1544-1550`，代码注释自述 "richer external approval wiring" 未落地）→
  `UnhandledRequirement` **直接失败整个委派**。父级注入的 `InteractionHandler` 对子 agent 完全
  不可见；`Interaction { step_id, kind }` 无 delegate 归属字段。
- 需求：委派驱动路径上，子 agent（local + external）暂停的交互路由到父级注入的
  `InteractionHandler`（或提供 per-delegate handler 包装接缝，如
  `Fn(delegate_name) -> Arc<dyn InteractionHandler>`），并携带来源（delegate 名 / 深度）。
- 阻塞：决策 D3（external ACP agent 审批穿透）、D5（统一 pop 到 root）、前置项 P2。

**A2. facade 级 reconfigure API（turn 边界生效）**

- 现状：agent 层 `ReconfigRequest` 机制齐备（`SetModel` / `ReplaceToolSet` /
  `SetSystemPromptOverlay` / ...，`agent/state/queue.rs:186-229`），但 facade 从未接线
  （`facade/agent.rs:1679-1681` 自述 "no reconfiguration ... on the base agent path"）；
  snapshot/restore 也**不能**换 model/system/tool 声明（恢复快照权威，`snapshot.rs:864-869`，
  重注入不同工具集会留下模型看到的声明与实际执行闭包的静默不一致）。
- 需求：facade 暴露 reconfig 排队入口并接线 reconfig handler，turn 边界应用（语义与
  `docs/agent-layer.md` §4.2 一致——机器层已支持，只差透出）。
- 阻塞：决策 D2 的 `apply_config`（把新 model/tools/system 应用到活会话）。

### B. 强烈建议（有 mag 侧兜底，但不做则验证体验受损）

**A3. cancel 对 external agent 的传导与清理**

- 现状：cancel 只协作式 abandon drive 并置 `cleanup_required`，**不杀子进程**（facade 从不调
  `registry.cleanup_agent`）；ACP read loop 只在行间隔查 `is_cancelled`，子进程静默时最长阻塞
  120 s（`DEFAULT_EXTERNAL_IO_TIMEOUT`）。
- 需求：(a) ACP read loop 对 cancellation 做 `select!`；(b) cancel/流 drop 时对 abandoned
  external session 触发清理（或提供 facade 级清理入口）。
- mag 兜底：自行持有 `RegistryExternalSessionHandler`（`default_external_session_handler` 经
  `.registry()` 暴露）做 sweep；但 120 s 阻塞只能 agent-lib 内修。

**A4. cancel 抢占被阻塞的 tool/interaction 批**

- 现状：cancel 只在 tool 批启动前/完成后检查（`facade/agent/stream.rs:244-305`）；被阻塞的
  tool handler 不被抢占 → `ask_user` 类长阻塞工具会冻结 run（无事件、无 pivot 窗口）直到返回。
- mag 兜底：handler 内 `select!` `ToolContext::cancel` 自行响应（`ask_user` 必须这么做）。
  批级抢占是根治，可后置。

### C. 可选（不阻塞；遇到再做）

- **A5** 专用 `FacadeError::Cancelled` 变体（现为 stringly `"agent run cancelled (cursor: …)"`，
  mag 已用 `cancel.is_cancelled()` 判别兜底）。
- **A6** `DelegationProgress`/`DelegationMessage` 真正发射（变体与 wire 投影齐备但生产代码从不
  emit）——「agent 间协作」可视化的素材。
- **A7** local subagent 可执行工具（worker 现只携带声明，子 agent 任何 tool call 得
  `UnknownTool`；LLM subagent 实为纯文本循环——第一版把 LLM subagent 当「审查/咨询」角色用
  即可绕开）。
- **A8** pivot 边界窗口对 host 可见（事件/标志），免去盲重试；queued pivot 跨窗口存活。
- **A9** external-start approval 走异步 handler（现 sync-only，headless deny；mag 用
  `ApprovalPolicy::ask_tool("ask_<name>")` 把「是否启动委派」经父级 `IpcApproval` 审批即可绕开）。
- **A10** `DelegationTrace` 增加 `is_external`；`RunEvent` 携带 run id（mag 已自铸 run id 兜底）。

### D. 无需 agent-lib 改动（评估确认可行）

- **pivot**：`interject` 失败无副作用，盲重试是安全用法；mag 的「队列 + 每次 poll 重试」设计
  成立（`PivotMessage`/`PivotSource` 在 `agent_lib::agent`，`IntoUserMessage` 在
  `agent_lib::facade`，均不在 prelude，显式 import）。
- **delegation**：model-routed `ask_<name>` 自动生成、名字冲突 build 期报错；
  `DelegationStarted/Finished/Failed/Artifact/Escalated` 事件真实发射（流式与非流式两路）。
- **external ACP agent**：presets / `build_with_default_session_handler()` / worktree 隔离齐备，
  behind `external-acp` feature（默认关，mag 在依赖声明里打开即可）。
- **snapshot/restore**：`RestoreExternal::MarkInterrupted` 为默认保守策略；restore 重注入
  `interaction_handler` / `external_agent`（含 session handler）/ `subagent` 入口齐备。
  **注意陷阱**：restore 不重新 `.subagent(..)` 注册，子 agent 审批策略静默回落
  `ApprovalPolicy::default()` = auto_allow——mag 恢复路径必须重注册全部 delegate。
- **ask_user**：用「阻塞式 tool handler + 闭包捕获 mag 侧 `IpcApproval` + `select!`
  `ToolContext::cancel`」实现，不需要机器层发射 `Question`/`Choice`（机器层从不发这两个变体，
  但它们 serde/wire 齐备，mag 经 `IpcApproval` 直发 `InteractionRequested` 即可）。
- **cancel 后会话复用**：committed 历史不变、cursor 回 `Idle`、同一 `Agent` 可继续 `stream()`
  （agent-lib 有测试覆盖）。
- **约束**：`AgentRunStream` 是 `!Send` 且整个 run 借用 `&mut Agent`——mag 的 per-session actor
  模型（`DESIGN.md` §3.1）已天然满足，pivot 重试必须在 driver 自己的循环里做（§3.2 设计即如此）。

## 6. 测试策略

沿用仓库离线纪律（假 LLM、无网络、单测试 < 1 分钟）：

- **mag-cli 单测**：命令解析；pivot 两层语义（`NotPivotable` 时自动回落 `send_message` 的分派
  逻辑）；交互提示协调（scripted `Arc<dyn MagService>` 重放 `InteractionRequested` 序列，含
  `origin` 标注的子 agent 交互，断言 `respond_interaction` 次序与内容）；渲染去交错。
- **e2e（pty 或管道驱动 stdin/stdout）**：`Engine::with_llm_client(fake)` 装配真 CLI，脚本化
  输入断言输出——覆盖验证清单 1/2/5/6 的完整回合。
- **配置系统**：TOML 解析/行级错误；DTO↔DO round-trip 不丢字段；resolve 拓扑与悬空引用报错；
  write-through 原子写；watch 去抖与自身写入去重；reload 失败沿用旧配置；生效时机矩阵
  （§4.4 每行一个测试）；`apply_config` 经 turn-complete listener 在边界生效。
- **pivot**：边界内注入成功 / 窗口外重试后成功 / run 结束前未落地发 `PivotDropped` /
  无 run 时 `NotPivotable` / cancel 后队列清空。
- **external ACP subagent 实例**：用 agent-lib 的 ACP client 方向驱动一个 fake ACP server
  （fake-acp.sh 范式；或第二个 mag 实例的 `mag --acp`，缺环境 `#[ignore]`），断言实例生命
  周期事件、子 agent 审批带 origin 穿透到 root、cancel 级联到运行中实例。
- 真实 LLM / 真实 external agent 联调（冒烟）：`#[ignore]`，缺环境干净跳过。

## 7. 已拍板决策记录

| # | 决策 | 内容 |
|---|---|---|
| D1 | **pivot 两层语义** | 第一层 service 原语：只对 in-progress turn 注入，无 in-progress turn 返回 `NotPivotable`，不隐式转换；第二层 interface 便利：不能 pivot 自动回落 `send_message`。两层兼顾严格语义与「随时打字必送达」体验（§3.2） |
| D2 | **配置生效：钉住 + 显式 apply + turn-complete 钩子** | 会话创建时钉住 `Arc<ConfigSnapshot>`；`apply_config` 排队到 turn 边界；mag-core 提供通用 turn-complete listener 机制，配置 apply 是其第一消费者，后续可挂桌面通知等（§4.4/§4.5）。自动应用档 GUI 阶段再评估 |
| D3 | **multi-agent 至少涵盖 external ACP agent** | external agent 是本程序核心功能，动态 subagent 体系（P7，[`dyn-agents.md`](dyn-agents.md)）把 `external-acp` 方向纳入本里程碑，尽早暴露问题；Claude Code/Codex/OpenCode 特化 adapter 推迟 |
| D4 | **DTO/DO 双向转换，配置≠配置文件** | DTO 是来源无关的 serde 镜像（文件只是其一种持久化）；DO 是 `Arc` 引用计数的对象树/森林，「钉住」= 持引用；双向转换对称可 round-trip；第一版只 user 级一层文件，project 分层推迟（§4.1/§4.2） |
| D5 | **子 agent 交互统一 pop 到 root 会话** | 实例链所有 `InteractionRequested` 经 root 会话发出、带 `origin` 来源标注，由 root 会话的单一交互队列处理——与 GUI/web 的模态队列同构（§3.3） |
| D6 | **ask_user 先行做普通 plugin** | AskUserQuestion 作为普通 `ToolPlugin` 注册，经 tool call 触发 `Question`/`Choice` 交互验证通道；GUI 阶段再评估是否提升为特殊宿主能力（§5 P6） |
