# 动态 subagent 设计文档

> 上位设计：[`DESIGN.md`](DESIGN.md)。本文档定义 mag 的 subagent 体系下一形态：**定义与实例
> 分离、运行时动态实例化、local 与 external agent 统一**。参照规范：
> [Claude Code subagents](https://code.claude.com/docs/en/sub-agents)（markdown + frontmatter
> 定义文件、单一 `agent` 工具入口）与 [Codex custom agents](https://simonwillison.net/2026/Mar/16/codex-subagents/)
> （内置只读 `explorer`、工具面约束）。
>
> 状态：已实现（2026-07-22，TODO.md M1–M5 + F-R 全部完成；实现偏差见 §11 与各任务完成记录）。

## 0. 背景与动机

当前 subagent 是**配置驱动、会话建时静态注册**的：

- 除会话绑定的 entry 外，所有 `[agents.<name>]` 在 build/restore 时注册为 delegate
  （`mag-core/src/driver.rs:289`），每个 delegate 合成一个 `ask_<name>(task)` 工具
  （agent-lib `facade/delegate.rs:452`）。
- `ReconfigRequest` 没有增删 delegate 的变体，`PatchToolSet` 甚至拒绝触碰合成的
  `ask_<name>` 声明（agent-lib `facade/agent/builder.rs:860, 903`）——**运行中创建 agent
  没有通路**。
- `agents.<name>.system_prompt` 只是个裸字符串字段（`mag-config/src/dto.rs:126`），不配即
  `None`，没有任何内置默认提示词。

由此产生四个问题：

1. subagent 不能由 agent 在运行中自主创建，"动态"无从谈起；
2. 每个 delegate 一个合成工具，定义一多工具列表就膨胀；
3. subagent 没有规范的定义文件格式（参照 Claude Code / Codex），也没有兜底的通用 agent；
4. external agent 与 local agent 是两套注册路径，概念上不统一。

## 1. 目标与非目标

### 目标

- agent 可通过工具**自主、异步地**创建 subagent 实例，同一类型可并发多实例（如多个
  explorer 同时探查代码库不同位置）。
- 持久化的 subagent 以**规范 markdown 文件**定义（frontmatter + 正文），分层来源可覆盖。
- local 与 external（ACP）agent 统一进同一套"定义 → 实例化"体系。
- 分层 system prompt：内置骨架 + 定义正文；内置 `general-purpose` 兜底。

### 非目标（本阶段）

- 实例跨会话持久化（实例是短暂的，见 §5.2）。
- external agent 进程池 / 常驻复用（v1 按实例拉起，见 §6）。
- delegate 独立 provider（受 M4-R 限制，`model` 只能在 supervisor 同 provider 内选，
  见 `mag-core/src/driver.rs:1107`）。
- 后台长任务的 UI 展示与管理界面（事件进 session journal，展示层后补）。

## 2. 核心概念：定义与实例分离

所有 subagent 来源统一为 **AgentDefinition 注册表**——它只描述"这类 agent 长什么样"；
运行时通过 `agent` 工具按需**实例化**，实例即用即抛。

```
AgentDefinition {
  name, description,
  kind: Local { model?, tools?, prompt_body }
      | External { command, env, capabilities }   // acp
}
```

这一转变的附带收益：build 时的静态 delegate 注册、restore 时的
`prune_unregistered_delegates`（`driver.rs:395`）整套机制退役——实例是短暂的工具执行，
不涉及名册变更，没有跨会话一致性问题。配置从"delegate 存在性的权威"降级为"定义的初始
来源之一"。

## 3. Agent 定义

### 3.1 markdown 定义文件

以 Claude Code 规范为准：YAML frontmatter + **正文即 prompt**。

```markdown
---
name: explorer
description: 只读代码探查 agent。用于找文件、搜代码、回答代码库结构问题。
tools: read_file, list_dir, grep, glob
model: claude-haiku-4-5        # 可选
---

（正文 = 该 agent 的专属指令，追加在内置骨架之上，见 §4）
```

external agent 用 frontmatter 的 `kind` 区分：

```markdown
---
name: peer
description: ACP 对端 agent
kind: acp
command: ["peer-agent", "--acp"]
env:
  FOO: bar
---

（可选正文：拼在 task 前面的任务框架模板，见 §6）
```

字段约定：

| 字段 | 必选 | 说明 |
|---|---|---|
| `name` | 否 | 缺省取文件名（去 `.md`） |
| `description` | 是 | 出现在 `agent` 工具描述中，model 据此选类型；同时用于 UI |
| `kind` | 否 | `local`（缺省）或 `acp` |
| `tools` | 否 | 仅 local。逗号分隔；**缺省 = 继承 supervisor 工具面**；显式给出时与 supervisor 工具面**取交集**（防提权，等价 Codex 的 sandbox 约束） |
| `model` | 否 | 仅 local。缺省继承 supervisor 的 model；受 M4-R 限制只能选同 provider 的 model |
| `max_steps` | 否 | 仅 local。步数预算上限；缺省用运行时默认 |
| `command` / `env` | external 必选 / 否 | 仅 `kind: acp` |

### 3.2 定义来源与优先级

同名定义按以下优先级覆盖（低 → 高）：

1. **内置**（编译进二进制）：`general-purpose`、`explorer`（见 §3.3）
2. **用户级**：`~/.config/mag/agents/*.md`
3. **项目级**：`<project>/.mag/agents/*.md`
4. **TOML 配置**：`[agents.<name>]` 非绑定项与 `[external_agents.<name>]`——结构同构，
   继续作为定义源，成本为零；文档推荐 markdown 为首选形式。

主 agent 的会话绑定机制（`[session].default_agent`、被绑定 entry 的 provider/model/tools）
**完全不动**——绑定的是 supervisor 自身，不属于 subagent 注册表。

### 3.3 内置 agent

- **`general-purpose`**：永远存在、零配置可用的兜底类型，`agent` 工具 `type` 参数的缺省值。
  工具面 = 继承 supervisor。正文是通用任务执行指令。
- **`explorer`**：内置只读类型（`tools` 限只读集），对应 Claude Code 的 `Explore` 与 Codex
  的 `explorer`，也是并发探查场景的主力类型。

## 4. 分层 system prompt

仅适用于 local 实例（external 的提示词归对端自己管）：

1. **内置骨架**（所有 local subagent 共享，保持短小）：
   - 你是被 supervisor 派生的子代理；开场 user message 是任务简报；
   - 在预算内自主完成，不与终端用户直接交互（审批经 origin 冒泡到 root）；
   - **最后一条消息是给 supervisor 的报告**：结论、改动、关键文件引用、遗留问题——用
     汇报语气，不是对话语气。
2. **定义正文**（md 内容或 TOML `system_prompt`），追加在骨架之后。

v1 不提供"完全替换骨架"的模式；骨架只声明角色与报告契约，不与正文冲突。组装沿用
agent-lib 现有的 base + overlay 拼接（`agent/request.rs` 的 `combine_system_prompt`），
实例化时把两层拼好一次性传入即可。

## 5. `agent` 工具与异步实例模型

### 5.1 工具面

**单一工具入口**，不为任何类型合成专属工具：

```
agent(type="explorer", task="梳理 crates/mag-core 的装配流程", description="探查装配层")
→ { id: "explorer-1", status: "running" }
```

- `type`：缺省 `general-purpose`。工具描述中**动态枚举注册表所有类型及其 description**
  （local 与 external 混排，model 无需关心底层形态）。未知类型在 spawn 调用内同步报错，
  并列出可用类型。
- `task`：任务简报，作为实例的开场 user message（沿用 `delegation_opening_input` 语义，
  agent-lib `facade/delegate/handler.rs:302`）。
- `description`：可选短标签，用于 UI 与日志。

配套工具：

| 工具 | 语义 |
|---|---|
| `agent` | 异步 spawn，立即返回 `{ id, status }` |
| `agent_result(id, timeout?)` | **阻塞**等待完成（带超时参数），返回最终报告 |
| `agent_cancel(id)` | 取消运行中的实例 |

拉取语义定为"阻塞 + 超时"而非"立即返回当前状态"：supervisor 调用它时本来就是要等结果；
纯状态查询由完成通知（§5.3）覆盖。

### 5.2 实例生命周期

`running → completed / failed / cancelled`。**实例是短暂的**：

- 不跨会话持久化，不进 restore 路径；
- session 结束时仍在运行的实例一律 cancel；
- 完成后的报告进 session journal，供事后查看。

spawn 本身是异步的：child 拿到开场提示词后要跑完整 agent loop，`agent` 调用只负责创建
实例并立即返回。

### 5.3 结果收集：拉 + 推双通道

- **拉**：`agent_result(id)` 阻塞等待（§5.1）。
- **推**：实例完成时向 supervisor 会话注入一条完成通知（实例 id + 状态 + 报告摘要），
  supervisor 无需干等即可在后续 step 感知。注入通道已于 2026-07-22 核实：pending context
  不对外暴露，可用通道是——run 进行中经 pivot（`AgentRunStream::interject_pivot` +
  `PivotSource::Host{label}`，mag 复用 driver 已有 `drain_pivots` 机制；纯文本 turn 无窗口，
  通知可能推迟到下一边界）；run 空闲时缓冲、拼进下一次用户输入。两条均在 mag 侧实现，
  不依赖 agent-lib 改动；纯拉（`agent_result`）始终兜底。

### 5.4 并发与嵌套

- 多实例并发：supervisor 在同一 turn 发多个 `agent` 调用，各自立即返回，实例**并发执行**
  ——并发性由异步实例模型保证，不依赖工具执行器是否并行执行 tool call。
- local 实例共享 supervisor 的 LLM client，跑在运行时 executor 上。
- 嵌套：child 的工具面默认可含 `agent` 工具，沿用现有委派深度上限（8，
  agent-lib `facade/delegate.rs:448`）。

## 6. external agent 统一

external agent 从"build 时连好常驻"改为**按实例拉起**：

- spawn 时启动进程（`command` + `env`）、建立 ACP 会话、发送 task（md 正文作为任务框架
  模板拼在 task 前）；完成（`completed/failed/cancelled`）后回收进程。
- 同一类型并发多实例 = 多进程，天然隔离。进程池/常驻复用留作后续优化。
- 实例的权限请求沿用 origin 冒泡到 root 会话处理（与现有 delegate 一致）。
- 分层 prompt（§4）不适用；报告契约同样不适用——对端的最终消息即报告。

TOML `[external_agents.<name>]` 与 markdown `kind: acp` 定义等价，同为注册表来源。

## 7. 工具面与审批策略

- **实例工具面**：定义 `tools` 缺省继承 supervisor；显式给出时与 supervisor 工具面取交集。
  `agent` 工具默认包含（支持嵌套，深度受限）。
- **spawn 审批**：沿用现有 delegate start tier 的模式（`apply_delegate_start_tiers`，
  `driver.rs:1245`），粒度细化到类型：`agent:explorer` 可免审、`agent:peer`（external）
  要审。策略表达从 `ask_tool("ask_<name>")` 迁移为按 `agent:<type>` 匹配。
- **预算**：实例必须有预算兜底——定义可声明 budget，缺省用全局默认，防止失控。
- **审批冒泡**：实例运行中的工具审批带 origin 统一 pop 到 root（现状不变）。

## 8. 与现有机制的关系（退役清单）

| 现有机制 | 去向 |
|---|---|
| 每个 delegate 一个 `ask_<name>` 合成工具 | 退役，由单一 `agent` 工具取代 |
| build/restore 时静态注册 delegate（`driver.rs:289/383`） | 退役，改为定义注册表 + 运行时实例化 |
| `prune_unregistered_delegates`（`driver.rs:395`） | 退役，实例无名册概念 |
| `apply_delegate_start_tiers` 的 `ask_<name>` tier | 迁移为 `agent:<type>` tier |
| `agents.<name>.system_prompt` 裸字段 | 保留为定义来源之一，纳入分层 prompt 第二层 |
| `[external_agents]` build 时常驻连接 | 改为按实例拉起 |
| agent-lib `Delegation::single_tool`（预留的动态名册模式） | 作为 `agent` 工具的实现底子复用 |

## 9. 实现影响分布

**实现分布已于 2026-07-22 逐行核实并修正**（原估计"agent-lib 是大头"不成立）：

- **agent-lib**（最小表面，仅一项）：pub `run_external_once` 一次性 external 调用包装
  （包装 `pub(crate)` 的 `drive_external`，`facade/external/delegate.rs:443`）+
  `ExternalDriveOutcome` 提 pub + completed 态进程回收。不需要 Send 改造（facade run
  future 刻意 `!Send`，mag 沿用每会话 current_thread + `LocalSet` + `spawn_local` 纪律，
  `mag-core/src/session.rs:420,458`）；不需要 ReconfigRequest/委派机制改动；origin 路由、
  pivot 通知均可由 mag 经已有 pub API（`agent_lib::agent::` 路径）实现。
- **mag-config**：`AgentDefinition` 统一模型；markdown 定义文件发现与 frontmatter 解析
  （手写 `---` 分隔 + `serde_yml`，`serde_yaml` 已停更）；四来源优先级合并；TOML 投影
  （吸收 `assembly.rs:593-626` 的映射逻辑）。
- **mag-core**（主体）：实例注册表（`instances` 模块）；`agent` / `agent_result` /
  `agent_cancel` 三工具（facade `Tool::function_with_schema`，闭包捕获共享状态）；实例
  spawn_local 驱动任务（child 为独立构建的 facade Agent，共享 supervisor client，工具面
  真实可执行——顺带绕开旧 delegate child 工具 declaration-only 的缺口，
  `agent-lib/src/facade/delegate/handler.rs:189-197`）；origin 交互路由（复制
  `DelegationInteractionRouter` 范式）；wire `Event`/`ServiceEvent` 实例生命周期变体
  （+ts-rs）；完成通知（pivot + 空闲缓冲）；静态委派退役（§8）；审批 tier 与 cancel 级联。
- **docs**：`CLI.md` agents 章节重写；本文件为设计依据。

## 10. 决策记录

- **D1**：动态创建的触发方是 agent 自主（工具调用），不是用户命令；否则失去"动态"的
  意义。
- **D2**：持久化 subagent 留在配置体系，但升级为规范 markdown 定义文件，参照 Claude
  Code（格式）与 Codex（内置 explorer、工具面约束）。
- **D3**：动态 agent 从定义**实例化**，同一类型可并发多实例；实例即用即抛，不持久化。
- **D4**：分层 system prompt（内置骨架 + 定义正文），且必须有内置 `general-purpose`
  兜底。
- **D5**：创建工具就叫 `agent`，单工具 + `type` 参数；不为每个类型合成专属工具。
- **D6**：spawn 是异步操作——概念上 child 处理开场提示词就不是即时返回的；结果收集 =
  `agent_result` 拉 + 完成通知推。
- **D7**：external agent 统一进同一注册表与实例模型，按实例拉起。
- **D8**：`agent_result` 语义为阻塞 + 超时参数，而非立即返回当前状态。

## 11. 已知限制与后续方向

- `model` 字段受 M4-R 限制（delegate 独立 provider 未接通），只能选 supervisor 同
  provider 的 model。
- external 按实例拉起有进程启动开销；进程池 / 常驻复用是后续优化。
- external 实例对「持续输出但永不完成」的对端**无 wall-clock 上限**：120s 是 ACP
  transport 的**每读 idle 超时**（静默对端每读最多挂 120s → SessionLost → Failed +
  进程回收），不约束整体运行时长；不再需要的运行依赖协作式 cancel（`agent_cancel` /
  会话级级联）终止。进程池 / 常驻复用同为后续优化（M4-R 结论）。
- 实例的 UI 展示（TUI/web 中的实例列表、实时进度）依赖实例事件投影，本阶段只落
  journal。
- 完成通知：run 中经 pivot 通道（纯文本 turn 无窗口，可能延迟到下一边界）；run 空闲
  经下一次输入前缀缓冲；无"空闲即触发新 turn"的通道。
- 嵌套实例的深度/预算由 mag 侧注册表实现，不与 agent-lib 的 `RunContext` 委派链共享
  （实例是独立 root ctx）；深度上限语义沿用 8。
- spawn 审批粒度：v1 只有 per-tool tier（`[tools.agent]`）——M3-5 实证 agent-lib
  `ApprovalDecision` 只有 `Approve / Deny / Timeout / Cancel` 四变体、无 Ask/暂停变体
  （暂停语义由 `ApprovalKind::Ask` tier 自身表达），per-type（`agent:<type>`）留
  follow-up。
- supervisor 面 `agent` 工具的类型枚举描述在会话 build 时烘焙：`apply_config` 重建
  定义表后新定义立即可 spawn（handler 侧解析走新表），但展示给 model 的描述文本仅随
  `ReplaceToolSet`（绑定项配了 `tools` 时）刷新；无该键时会话内描述保持旧表，属纯
  展示层滞后（M3-5 偏差 4）。
