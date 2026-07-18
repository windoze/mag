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
