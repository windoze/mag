# 实施计划：动态 subagent（定义/实例分离 + 统一 `agent` 工具）

> **唯一设计输入**：[`docs/dyn-agents.md`](docs/dyn-agents.md)（决策 D1–D8：定义/实例分离、markdown
> 定义文件、内置 `general-purpose` 兜底、单一 `agent` 工具、异步实例模型、external 统一、
> `agent_result` 阻塞+超时）。
>
> 前置状态：
> - service 主干 / mag-acp / mag-cli / mag-web 四份计划均已完成并归档在
>   [`docs/archive/`](docs/archive/)（最新为 [`docs/archive/2026-07-22-mag-web/`](docs/archive/2026-07-22-mag-web/)）。
>
> **关键现状已于 2026-07-22 逐行代码核实**（结论已回写 `docs/dyn-agents.md` §5.3/§9/§11）：
> 1. facade `Agent::run*`/`AgentRunStream` 的 future 刻意 `!Send`（`NonNull` drop guard /
>    `Rc<RefCell>`），mag 现有纪律是**每会话 OS 线程 + current_thread runtime + `LocalSet` +
>    `spawn_local`**（`crates/mag-core/src/session.rs:420,458`）——实例驱动任务沿用同一纪律，
>    **agent-lib 不需要 Send 改造**。
> 2. origin 归因交互路由可用 pub API 在 mag 侧实现（`agent_lib::agent::{Interaction,
>    InteractionOrigin, InteractionHandler}` 均 pub，mag 已在用），复制 agent-lib
>    `DelegationInteractionRouter`（`agent-lib/src/facade/delegate/handler.rs:227-248`）范式即可。
> 3. 完成通知的推送可复用现有 pivot 通道（`AgentRunStream::interject_pivot` +
>    `PivotSource::Host{label}`，mag 经 `agent_lib::agent::` 路径可达，driver 已有
>    `drain_pivots` 机制 `driver.rs:898`）；run 空闲时缓冲、拼进下一次用户输入。**纯拉**
>    （`agent_result` 阻塞+超时）为兜底，两通道均不依赖 agent-lib 改动。
> 4. external "每委派新进程"已是现状（每次 drive mint 新 `agent_id`）；缺的只是"一次性调用 +
>    完成回收"的 pub 包装——agent-lib 唯一需要的新表面。
> 5. 现有 local delegate 的 child 工具是 declaration-only（调用即 `UnknownTool`，
>    `agent-lib/src/facade/delegate/handler.rs:189-197`）；新实例路径给 child 装配真实可执行
>    工具，顺带绕开该缺口（旧路径整体退役，见设计 §8）。

## 目标

落地 `docs/dyn-agents.md` 描述的动态 subagent 体系：

- **AgentDefinition 注册表**（mag-config）：内置（`general-purpose` + 只读 `explorer`）<
  `~/.config/mag/agents/*.md` < 项目 `.mag/agents/*.md` < TOML（`[agents]` 非绑定项 +
  `[external_agents]`）四来源合并；markdown = YAML frontmatter + 正文。
- **`agent` / `agent_result` / `agent_cancel` 三工具**（mag-core）：`agent` 异步 spawn 立即返回
  `{id, status}`；`agent_result(id, timeout?)` 阻塞等待报告；`agent_cancel(id)` 取消。同类型可
  并发多实例；嵌套深度上限 8。
- **local 实例**：spawn_local 驱动独立构建的 facade child Agent（共享 supervisor 的
  `Arc<dyn LlmClient>`、真实工具面子集、分层 system prompt、per-instance 预算与审批策略、
  origin 归因交互冒泡到 root）。
- **external 实例**：agent-lib 新增 `run_external_once` pub 包装，按实例拉起 ACP 进程、完成回收；
  与 local 同一工具入口、同一实例模型。
- **完成通知**：拉（`agent_result`）+ 推（run 中经 pivot `Host` 通道、空闲时拼入下次输入）。
- **旧静态委派退役**：`ask_<name>` 合成工具、build/restore 静态注册、`prune_unregistered_delegates`
  全部移除，相关测试迁移到新机制。
- **充分的离线测试**：FakeLlmClient 脚本化流（`mag-core/src/test_support.rs`）+ fake-acp.sh
  （`engine.rs:4457` 范式）覆盖并行 spawn、报告回收、审批冒泡、cancel、external 生命周期。

## 非目标（本计划）

- 实例跨会话持久化；实例的 UI 展示（事件落 wire/journal，前端渲染另立计划）。
- external 进程池/常驻复用；delegate 独立 provider（M4-R 限制，`model` 仅同 provider 内可选）。
- 嵌套实例的 mid-run 流式事件细粒度投影（v1 只投影实例生命周期事件）。
- per-type spawn 审批粒度（`agent:<type>`）若 `ApprovalDecision` 无 Ask 变体则降级为
  per-tool tier（`[tools.agent]`），per-type 留 follow-up（实现时验证，见 TODO M3-5）。
- 主 agent 绑定机制、`[session].default_agent`、providers 解析——一行不动。

## 里程碑

### M1 — agent-lib：一次性 external 调用面

新增 pub `run_external_once`（包装 `pub(crate)` 的 `drive_external`，隐藏 `CollabBridge`、自建或
可选继承 `RunContext`）+ `ExternalDriveOutcome` 提 pub + completed 实例的进程回收语义。
**注意：agent-lib 是 workspace 外的 path 依赖（`../agent-lib`），门禁在其仓库内执行。**

### M2 — mag-config：AgentDefinition 模型与定义加载

统一 `AgentDefinition`（Local/ExternalAcp 两 kind）+ frontmatter 解析（手写 `---` 分隔 +
新增 yaml 依赖）+ 目录发现（内置/user/project）+ TOML 投影（吸收 assembly.rs:593-626 的映射
逻辑）+ 优先级合并 + 内置 `general-purpose`/`explorer` 定义。

### M3 — mag-core：local 实例运行时与接线（核心）

wire `Event`/`ServiceEvent` 实例生命周期变体（+ts-rs 再生成）→ `AgentInstanceRegistry` →
`agent` 工具（同步校验 + child 装配 + spawn_local 驱动 + 事件转发 + origin 路由）→
`agent_result`/`agent_cancel` → driver 接线（三工具进工具面、静态委派退役、审批 tier、cancel
级联）→ 完成通知推送（pivot + 空闲缓冲）。

### M4 — external 实例化

`agent` 工具按 `kind` 分派到 `run_external_once`；`TrackedExternalSessionHandler` 复用与完成
回收；external 启动审批冒泡；fake-acp 离线测试。

### M5 — e2e 加固 + 文档

端到端集成测试（并行 explorer、报告回收、审批 origin、cancel、external 生命周期）；`docs/CLI.md`
agents 章节重写；`docs/dyn-agents.md` 状态更新。

## 完成定义

- `TODO.md` 中全部任务（含各 `M<n>-R` review 任务与 `F-R`）标 `[DONE]` 且有完成记录。
- 每任务通过门禁序列：`cargo fmt --all -- --check` → 聚焦测试 → `cargo clippy --all-targets
  -- -D warnings` → `cargo test --workspace` → `cargo doc --no-deps --workspace`；agent-lib
  任务在 `../agent-lib` 仓库内执行同等序列。
- 全部测试离线（FakeLlmClient 脚本化流 / fake-acp.sh / tempdir 配置），单测试 < 1 分钟。
- 静态委派退役后无残留引用（`ask_`、`subagent(`、`prune_unregistered` 全库 grep 干净）；
  公开 API 全部带 rustdoc；wire 新增变体同步 ts-rs 生成物、无手写漂移。
- 全部完成后做一次完整 review（`F-R`），修复发现的问题。
