# 实施计划：mag-cli（最小 CLI 验证原型 + 运行时配置系统）

> **唯一设计输入**：[`docs/CLI.md`](docs/CLI.md)（mag-cli 的实现级设计，含决策 D1–D6、
> §5 service 主干前置改动清单、§5A agent-lib 前置需求清单）。参照 [`docs/DESIGN.md`](docs/DESIGN.md)
> §3.0（`MagService` trait）、§6（来源与外部 agent）。
>
> 前置状态：
> - service 主干（`mag-service` 接口 + `mag-core` 实现 + `mag-tools`/`mag-sources` 核心）已全部 `[DONE]`，
>   `MagService` 契约已冻结，计划归档在 [`docs/archive/2026-07-19-mag-service/`](docs/archive/2026-07-19-mag-service/)。
> - 第一个 interface `mag-acp` 已全部 `[DONE]`，计划归档在
>   [`docs/archive/2026-07-20-mag-acp/`](docs/archive/2026-07-20-mag-acp/)。
> - `docs/CLI.md` §5A 的 agent-lib 前置需求（A/B 类）已在 agent-lib 侧全部实现、review 并推送，
>   其 gap 清单与计划归档在 `../agent-lib/docs/mag-gaps.md` 与 agent-lib 的 archive。
>
> **本计划只覆盖 `docs/CLI.md` §5 的 service 主干前置改动 + `mag-cli` crate 本身。**
> 逐任务清单见 [`TODO.md`](TODO.md)。

## 目标

落地 `docs/CLI.md` 描述的 **mag-cli**：一个最小 CLI 验证原型（interface #2），用「符合 GUI/web 使用模式」
的风格验证底层组件与管线全部通畅——基本 agent 对话（流式输出）、用户交互（tool 权限申请 +
AskUserQuestion 式通用交互）、多 agent 编排（含 **external ACP agent**）、agent 间协作、会话持久化与恢复、
会话内 cancel 与 pivot message。具体交付：

- **pivot 能力**（`docs/CLI.md` §3.2，决策 D1 两层语义）：`MagService::pivot_message` +
  `ServiceError::NotPivotable` + `PivotQueued/Applied/Dropped` 事件变体（全部向后兼容新增）；mag-core driver
  内 pivot 队列旁路（与 CancelHandle 同构），run_turn 每次 poll 后尝试 `AgentRunStream::interject()`，
  未落地在 run 结束发 `PivotDropped`。
- **交互归因**（`docs/CLI.md` §3.3，决策 D5）：`InteractionRequested` 增加 `origin` 字段
  （`#[serde(default)]` 向后兼容），子 agent 交互统一 pop 到 root 会话并标注来源
  （agent-lib 已提供带 `InteractionOrigin{delegate,depth}` 的父级路由）。
- **运行时配置系统**（`docs/CLI.md` §4，重点，决策 D2/D4）：新 crate `mag-config`（纯数据：DTO serde
  类型 + TOML 读写 + 行级校验 + DTO↔DO 双向转换，DO 为 `Arc` 对象树）；mag-core 内 `ConfigService`
  （`RwLock<Arc<ConfigSnapshot>>` + revision + write-through 原子写 + 文件 watch）；`MagService` 新增
  `get_config/update_config/reload_config/apply_config` 四方法 + `ConfigChanged` 事件；turn-complete
  通用通知/回调机制（`apply_config` 是第一消费者，经 agent-lib `Agent::reconfigure` 在 turn 边界应用）；
  `Engine::from_config` 装配构造器 + bin 读配置（`--config` 覆盖，`mag --acp` 同走配置）。
- **delegation 接线**（`docs/CLI.md` §5 P7，决策 D3）：model-routed `ask_<name>` 委派（local LLM
  subagent 用 agent-lib `Agent::worker()`；external ACP agent 用 mag-sources ACP slot →
  `ManagedExternalAgent::acp(..)` + `default_external_session_handler`，mag-core 需开 agent-lib
  `external-acp` feature）；`Delegation*` 事件映射；委派审批 `ApprovalPolicy::ask_tool("ask_<name>")`
  走 IpcApproval；restore 重注册全部 delegate。
- **ask_user 工具**（`docs/CLI.md` §5 P6，决策 D6）：mag-tools 普通 `ToolPlugin`，阻塞式 handler +
  闭包捕获 mag 侧交互桥 + `select!` `ToolContext::cancel`，发 `InteractionKindWire::Question/Choice`。
- **`mag-cli` crate**（`docs/CLI.md` §1/§2）：只依赖 `mag-service`（+ rustyline/tokio/futures/serde_json），
  面对 `Arc<dyn MagService>`；input/render 双任务 + PromptCoordinator（单一 pending 交互队列、origin 标注
  渲染）；slash 命令（`/new /sessions /resume /delete /cancel /sources /config show|reload|apply
  /help /quit`）；pivot 两层语义（run 中打字走 `pivot_message`，`NotPivotable` 自动回落 `send_message`）；
  Ctrl-C = cancel；bin 子命令（`mag` 默认 CLI、`--resume <id>`、`--config <path>`、`--acp` 保留）。
- **充分的离线测试**（`docs/CLI.md` §6）：映射/转换纯函数单测 + 注入 fake `LlmClient` / scripted
  `Arc<dyn MagService>` + e2e 管道驱动 stdin/stdout；真实联调一律 `#[ignore]`。

## 非目标（本计划）

- **易用性/界面美观**：CLI 是验证原型，不做高级 TUI（无彩色 diff、无 spinner 动画、无多行编辑）。
- **不改已冻结契约的既有语义**：对 `MagService` 只做向后兼容新增（方法/变体/字段），不改既有方法签名与
  事件语义；`mag-acp` 的 ACP interface 行为不变（它经 bin 装配自然使用新配置系统）。
- **不做 GUI/web**：本计划只交付 CLI；但所有设计（配置系统、事件模型、交互归因、双任务结构）必须符合
  GUI/web 使用模式（`docs/CLI.md` §0）。
- **不做 ACP server 方向的改动**（那是 mag-acp，已完成）；external ACP agent 是 client 方向消费。
- **AskUserQuestion 不做 GUI 级设计**：作为普通 plugin 先行（决策 D6），GUI 阶段再详细设计。
- **不经 Command/Event**：Command/Event 是 tauri/web 的平行 wire 编码（`docs/DESIGN.md` §4）；CLI 直接面对
  `MagService`（`docs/CLI.md` §1.1）。

## 里程碑

逐层增加概念、每层可独立**离线**验证。顺序照 `docs/CLI.md` §5 前置清单 → CLI 本身。

### M1 — pivot 能力（`docs/CLI.md` §3.2，决策 D1）

`mag-service` 加 `pivot_message` 方法 + `ServiceError::NotPivotable` + `PivotQueued/Applied/Dropped` 事件；
mag-core driver 加 pivot 队列旁路（`interject()`，InvalidState 留队重试，run 结束未落地发 `PivotDropped`）。
重点文件：`crates/mag-service/src/{service.rs,lib.rs}`、`crates/mag-core/src/driver.rs`。

### M2 — 交互归因（`docs/CLI.md` §3.3，决策 D5）

`mag-service` 加 `InteractionOrigin` wire 类型 + `InteractionRequested` 加 `#[serde(default)] origin` 字段；
mag-core 把 agent-lib 路由来的子 agent 交互映射到 wire origin。重点文件：
`crates/mag-service/src/service.rs`、`crates/mag-core/src/{driver.rs,approval.rs}`。

### M3 — 运行时配置系统（`docs/CLI.md` §4，决策 D2/D4，重点）

新 crate `mag-config`（DTO/TOML/校验/DTO↔DO）；mag-core `ConfigService`；`MagService` 四方法 +
`ConfigChanged` 事件；turn-complete listener 机制 + `apply_config`；`Engine::from_config` + bin 读配置。
重点文件：`crates/mag-config/src/*`（新）、`crates/mag-core/src/{config.rs,engine.rs}`、
`crates/mag-service/src/service.rs`、`crates/mag/src/main.rs`。

### M4 — delegation 接线（`docs/CLI.md` §5 P7，决策 D3）

model-routed `ask_<name>` 委派（local LLM subagent + external ACP agent 两条来源）；`Delegation*` 事件映射；
委派审批走 IpcApproval；restore 重注册 delegate。重点文件：`crates/mag-core/src/{engine.rs,driver.rs,
delegate.rs}`（新）、`crates/mag-sources/src/registry.rs`、mag-core `Cargo.toml`（开 `external-acp`）。

### M5 — ask_user 工具（`docs/CLI.md` §5 P6，决策 D6）

mag-tools 普通 `ToolPlugin`：`ask_user`（Question/Choice），阻塞式 handler + cancel 抢占。重点文件：
`crates/mag-tools/src/ask_user.rs`（新）、`crates/mag-tools/src/lib.rs`。

### M6 — mag-cli crate（`docs/CLI.md` §1/§2）

`crates/mag-cli`（只依赖 `mag-service`）：双任务 REPL + PromptCoordinator + slash 命令 + pivot 回落 +
Ctrl-C cancel + bin 装配 + e2e。重点文件：`crates/mag-cli/src/*`（新）、`crates/mag/src/main.rs`。

## 完成定义

- `TODO.md` 中全部任务（含各 `M<n>-R` review 任务）标 `[DONE]` 且有完成记录。
- 每任务通过门禁序列：`cargo fmt --all -- --check` → 聚焦测试 → `cargo clippy --all-targets -- -D warnings`
  → `cargo test --workspace` → `cargo doc --no-deps --workspace`。
- 全部测试离线（fake LlmClient / scripted `Arc<dyn MagService>` / 管道 e2e），单测试 < 1 分钟；真实联调
  `#[ignore]` 干净跳过。
- 对冻结契约只加不改；公开 API 全部带 rustdoc。
- 全部完成后做一次完整 review，修复发现的问题。
