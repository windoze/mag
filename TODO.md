# TODO：动态 subagent 落地任务单（定义/实例分离 + 统一 `agent` 工具）

> 依据 [`PLAN.md`](PLAN.md) 与**唯一设计输入** [`docs/dyn-agents.md`](docs/dyn-agents.md)（决策
> D1–D8）。关键现状已于 2026-07-22 逐行代码核实并回写设计文档 §5.3/§9/§11，本单不再重复论证，
> 任务内直接给出结论与锚点。
> 既有计划归档：[`docs/archive/2026-07-19-mag-service/`](docs/archive/2026-07-19-mag-service/)、
> [`docs/archive/2026-07-20-mag-acp/`](docs/archive/2026-07-20-mag-acp/)、
> [`docs/archive/2026-07-20-mag-cli/`](docs/archive/2026-07-20-mag-cli/)、
> [`docs/archive/2026-07-22-mag-web/`](docs/archive/2026-07-22-mag-web/)。

## 通用执行规则

- **一次一个任务**：每次只执行「首个标题带 `[TODO]` 的任务」。完成后把 `[TODO]` 改为 `[DONE]`，在任务
  末尾补「完成记录」，提交并推送，然后继续下一个任务。
- **完成的定义**：只有标题带 `[DONE]` 才算完成。review 任务（`M<n>-R`、`F-R`）是真实任务，不得跳过。
- **编号**：任务按实现顺序编号 `M<里程碑>-<序号>`；每个里程碑末尾有独立 review 任务 `M<n>-R`；
  全部里程碑完成后有一次全计划 review `F-R`。
- **两个仓库的门禁**：
  - mag 任务（默认）：在 workspace 根执行 `cargo fmt --all -- --check` → 聚焦测试 →
    `cargo clippy --all-targets -- -D warnings` → `cargo test --workspace` →
    `cargo doc --no-deps --workspace`。
  - agent-lib 任务（M1）：agent-lib 是 workspace 外的 path 依赖（根 `Cargo.toml:20` →
    `../agent-lib`），门禁在 `/Volumes/Data/home/chenxu/repos/agent-lib` 仓库内执行同等序列，
    然后再回 mag 根执行 `cargo test --workspace` 确认 path 依赖下游无破坏。
- **依赖边界（硬约束）**：
  - agent-lib：本计划只允许 M1 列出的新表面（`run_external_once` + outcome 提 pub）；**不**做
    Send 改造、**不**改委派/ReconfigRequest 现有语义、**不**删除旧 API（`subagent`/
    `external_agent`/`prune_unregistered_delegates` 保留在库里，mag 侧停用即可）。
  - `mag-config`：不依赖 mag-core / agent-lib（现状保持，只加 serde/serde_yml 一类叶子依赖）。
  - `mag-core` 新实例模块（`instances`）：状态与驱动逻辑独立成模块，对 session/driver 只做
    参数注入，不反向持有。
  - `mag-service` wire：只做**向后兼容新增**（变体/字段），新增类型同步 ts-rs 派生与
    `ui/packages/protocol` 生成物，禁止手写 TS 类型。
- **离线测试纪律**：全部测试离线——FakeLlmClient 脚本化流（`mag-core/src/test_support.rs`）、
  fake-acp.sh（`mag-core/src/engine.rs:4457` 范式）、tempdir 配置、临时目录定义文件。不依赖网络/
  真实凭据/真实 LLM。每个测试须 1 分钟内完成，卡住即为 bug。
- **secret 纪律**：配置/定义文件中 secret 只以 `{env=...}`/`{keyring=...}` 引用形态出现；任何
  API 响应、测试、日志不输出解析后的值。
- **默认完整验证序列**（任务另有说明以任务为准）：上述对应仓库门禁序列全绿。

---

## M1 — agent-lib：一次性 external 调用面

### M1-1 [DONE] agent-lib：pub `run_external_once` + `ExternalDriveOutcome` 提 pub + completed 回收

**目标**：给 mag 提供一个"拉起 external ACP agent → 跑一个 task → 拿最终文本 → 回收进程"的
一次性 pub API。这是本计划在 agent-lib 的**唯一**新表面。

**现状锚点**（均在 `../agent-lib`）：
- `drive_external`：`src/facade/external/delegate.rs:443-451`，`pub(crate)`，签名入参为
  `name: &str, agent: &ManagedExternalAgent, ids: &FacadeIds, task: String,
  collab: &CollabBridge, parent_interaction: Option<Arc<dyn InteractionHandler>>,
  ctx: &RunContext`，返回 `Result<ExternalDriveOutcome, FacadeError>`。
- `ExternalDriveOutcome`：同文件 :204-220，`pub(crate)`，字段 `summary / usage / artifacts /
  completed / cleanup_required / session`。
- `CollabBridge`：`src/facade/collab.rs:350`，`#[derive(Default)]`，inactive 时全 no-op
  （:372-385）——一次性调用内部传 `CollabBridge::default()` 即可，**不暴露**。
- root `RunContext` 自建范式：`src/facade/agent.rs:480-485`
  （`RunContext::new_root_with_cancellation(ids.run_id(), budget, ids.trace_root(...),
  cancel_token)`）。
- 失败/取消的 detached 清扫：`delegate.rs:528-540` → `registry.cleanup_agent`
  （`src/agent/external/registry.rs:448`）；**completed 目前不回收**（:429 注释），session 挂到
  `Agent` drop 才由 `kill_on_drop` 兜底。

**改动点**：
1. `ExternalDriveOutcome` 提 `pub`（字段保持），从 `facade::external` re-export。
2. `src/facade/external/delegate.rs` 新增：

   ```rust
   pub async fn run_external_once(
       name: &str,
       agent: &ManagedExternalAgent,
       ids: &FacadeIds,
       task: String,
       parent_interaction: Option<Arc<dyn InteractionHandler>>,
       budget: BudgetLimits,
       cancel: CancellationToken,
   ) -> Result<ExternalDriveOutcome, FacadeError>
   ```

   内部：`CollabBridge::default()` + `RunContext::new_root_with_cancellation(...)` 自建 root
   ctx（一次性调用无父链），调 `drive_external`。
3. **completed 回收语义**：一次性调用语义下，outcome 落地后（completed/failed/cancelled 三种
   终态）都应 schedule `cleanup_agent`（沿用 :528-540 的 detached-sweep 范式，把条件从
   `!captured.completed` 放宽为按 `run_external_once` 的调用语义全覆盖；注意不要影响
   `drive_external` 被旧静态委派路径复用时的既有行为——做法是 wrapper 自己补 completed 的
   sweep，而不是改 `drive_external` 的条件）。
4. rustdoc：注明这是一次性语义（进程随调用终态回收）、`ManagedExternalAgent::session_handler`
   缺失时返回 `FacadeError::ExternalAgent`（沿用 :452-459）。

**验证**：
- agent-lib 仓库内新增测试（复用现有 external 测试的 fake session/adapter 基建，参考
  `src/facade/external/` 既有测试）：① `run_external_once` 跑通 happy path 并返回 summary；
  ② completed 后 registry 无存活 session（回收生效）；③ cancel token 触发后返回 cancelled
  且回收生效；④ 无 session_handler 报错路径。
- 门禁：`cd ../agent-lib && cargo fmt -- --check && cargo clippy --all-targets -- -D warnings
  && cargo test`；再回 mag 根 `cargo test --workspace` 确认下游无破坏。

**完成记录**（2026-07-22）：

- 改动（agent-lib）：
  - `src/facade/external/delegate.rs`：`ExternalDriveOutcome` 提 `pub`（字段不动，rustdoc 补
    一句宿主经 `run_external_once` 获取）；新增 `pub async fn run_external_once`（签名与任务
    单一致，内部 `CollabBridge::default()` + `RunContext::new_root_with_cancellation` 自建
    root ctx，trace 根标签 `"external-once"`）；completed 时 wrapper 自己补一次 detached
    sweep。
  - 为拿到 drive 使用的 `agent_id`（`FacadeIds` 计数器单调、无法二次 mint 同值），把
    `drive_external` 主体抽为私有 `drive_external_with_agent_id`（返回值多带一个
    `AgentId`），`drive_external` 保持原签名原语义的薄包装——mint 位置、驱动逻辑、sweep
    条件 `!captured.completed` **逐字未动**，旧静态委派路径（`facade/delegate/handler.rs`）
    行为零变化。sweep 体抽为共用 `spawn_external_cleanup_sweep`（trace 节点 id 格式
    `external-cleanup-sweep/{run_id}/{agent_id}/{seq}` 不变，completed sweep 与
    uncommitted sweep 互斥，无重复 id）。
  - re-export：`facade::external::{run_external_once, ExternalDriveOutcome}`，并按 crate
    惯例在 facade 根（`agent_lib::facade::`）同步 re-export。
  - rustdoc：一次性语义、三终态回收保证（failed/cancelled 由 drive 内部既有 sweep、
    completed 由 wrapper 补 sweep）、detached 后台回收语义、`session_handler` 缺失返回
    `FacadeError::ExternalAgent`。
- 新增测试（`src/facade/external/tests.rs`，全部离线、**不依赖任何 `external-*` feature**，
  默认 `cargo test` 即运行；新增非门控 fake adapter/session `OnceFakeAdapter`/
  `OnceFakeSession` 置于真实 `ExternalSessionRegistry` 之后，并解除既有
  `RecordingWorktreeManager`/`observed_within` 两个测试基建的 cfg 门控以复用）：
  - `run_external_once_completes_and_returns_summary`（happy path，summary 回收）；
  - `run_external_once_completed_sweeps_the_live_session`（返回时 registry 尚有 1 个存活
    session → wrapper 的 detached sweep 落地后 `live_len()==0`、shutdown 计数 1、worktree
    cleanup 1）；
  - `run_external_once_cancel_abandons_and_sweeps_the_live_session`（advance 悬挂 + 50ms
    cancel → 5s 内返回 `completed=false`/`cleanup_required=true` → sweep 回收生效）；
  - `run_external_once_without_session_handler_fails_fast`（`FacadeError::ExternalAgent`，
    报文含 `no runtime session handler`）。
- 门禁结果：agent-lib `cargo fmt -- --check` ✅、`cargo clippy --all-targets -- -D warnings`
  ✅（默认与 `--features external-acp` 两组合）、`cargo test` ✅（默认 feature lib 1061
  passed 含 4 个新测试；`--features external-acp` 40 套件全绿，既有 gated sweep/路由测试
  不受影响）；mag 根 `cargo check --workspace` ✅、`cargo test --workspace` ✅（32 套件全
  ok，0 失败）。
- 偏差：仅实现形态两点（① 私有助手 `drive_external_with_agent_id` 返回 `AgentId` 供
  wrapper 补 sweep，而非任务单设想的"直接调 `drive_external`"——原因是 `FacadeIds` 无法
  复现同值 mint；② 新测试走非门控 fake adapter 而非 `external-acp` gated 基建，保证默认
  门禁命令即可执行）。无语义偏差。

### M1-R [DONE] M1 review：一次性 external 调用面

**内容**：
- review M1 全部 diff：新表面是否最小（只此一个函数 + outcome 提 pub）；`drive_external` 旧
  调用方（静态委派路径）行为是否零变化；sweep 条件没有被误改。
- 检查 rustdoc 完整、`#[non_exhaustive]` 等演化约束是否合理。
- 在 mag 根跑 `cargo test --workspace` + clippy 确认 path 依赖下游干净。
- 对照设计文档 `docs/dyn-agents.md` §6 确认语义一致（按实例拉起、完成回收）。

**验证**：门禁序列全绿；review 发现的问题已修复并附完成记录。

**完成记录**（2026-07-22）：review 通过，未发现问题，agent-lib 零改动。

- review 范围：`git diff c1f7751..8db05c7` 全量（4 文件，+491/-54）。
- 新表面最小：pub 新增仅 `run_external_once` + `ExternalDriveOutcome` 提 pub（re-export
  路径 `facade::external::` 与 facade 根两处，符合 crate 惯例）；`drive_external_with_agent_id`
  与 `spawn_external_cleanup_sweep` 均为私有。
- 旧静态委派路径零变化（唯一旧调用方 `facade/delegate/handler.rs:807`）：`agent_id` mint
  位置（worker 内 `ids.agent_id()`）未动；sweep 条件 `!captured.completed` 逐字未动；
  trace 节点 id 格式 `external-cleanup-sweep/{run_id}/{agent_id}/{seq}` 未动；`drive_external`
  保持原签名，仅为丢弃 `AgentId` 的薄包装。
- completed 补 sweep 确为 wrapper 自身行为：`run_external_once` 内 `outcome.completed &&
  session_handler` 才补扫，与 drive 的 uncommitted sweep 互斥（completed 时 drive 不扫），
  trace id 无碰撞；错误返回路径（`?`）由 drive 内部 sweep 覆盖，无泄漏。
- rustdoc 完整：一次性语义、三终态回收保证、detached 后台回收、缺 `session_handler` 报
  `FacadeError::ExternalAgent`，intra-doc 链接指向真实存在的方法。
- `#[non_exhaustive]` 判断：`ExternalDriveOutcome` 未加，与同文件同为「事实捕获 DO」的
  pub 结构 `RetainedExternalSession`（pub 字段、无 non_exhaustive）一致——模块内惯例统一，
  合理，不改。
- 测试断言真实回收：detached sweep 的落地经 `observed_within` 轮询断言
  （`live_len()==0` + shutdown 计数 + worktree cleanup 计数），非竞态猜测；completed 测试
  还先断言返回瞬间 registry 仍有 1 个存活 session（证明回收来自 wrapper 补扫而非 drive）。
- 语义对照 `docs/dyn-agents.md` §6：按实例拉起 + 三终态（completed/failed/cancelled）回收，
  一致。
- 门禁（全部实跑复验）：agent-lib 默认 feature `cargo fmt -- --check` ✅、
  `cargo clippy --all-targets -- -D warnings` ✅、`cargo test` ✅（4 个
  `run_external_once_*` 新测试单独复跑全绿）；`--features external-acp` clippy ✅、test ✅
  （全套件 0 失败）；mag 根 `cargo test --workspace` ✅（32 套件全 ok，0 失败）。

---

## M2 — mag-config：AgentDefinition 模型与定义加载

### M2-1 [DONE] mag-config：`AgentDefinition` 统一模型 + markdown frontmatter 解析

**目标**：新增 `crates/mag-config/src/agent_def.rs`，定义跨来源统一的 subagent 定义模型与
markdown 定义文件解析器。

**现状锚点**：
- mag-config 现有 DO：`ResolvedAgent`（`crates/mag-config/src/snapshot.rs:407`，字段
  `name/provider/model/tools/system_prompt/role/budget`）、`ResolvedExternalAgent`
  （:490，`kind/command/env/capabilities`）。
- workspace 依赖集中在根 `Cargo.toml` `[workspace.dependencies]`（:19-36），**目前没有任何
  yaml/frontmatter  crate**——需新增。注意 `serde_yaml` 已归档停更，用其维护 fork
  `serde_yml`（只解析 frontmatter mapping，不引入 gray_matter）。
- 配置文件位置约定：`crates/mag/src/main.rs:234`（`$XDG_CONFIG_HOME/mag` 否则 `~/.config/mag`）。

**改动点**：
1. 根 `Cargo.toml` `[workspace.dependencies]` 加 `serde_yml`；`mag-config/Cargo.toml` 引用。
2. `agent_def.rs`：

   ```rust
   pub enum AgentKindDef {
       Local { model: Option<String>, tools: Option<Vec<String>>, max_steps: Option<u32> },
       ExternalAcp { command: Vec<String>, env: BTreeMap<String, String> },
   }
   pub struct AgentDefinition {
       pub name: String,
       pub description: String,
       pub kind: AgentKindDef,
       pub body: String,                 // markdown 正文；local=prompt 第二层，acp=task 模板
       pub source: DefinitionSource,     // Builtin | User | Project | Toml
   }
   pub fn parse_agent_md(file_stem: &str, content: &str) -> Result<AgentDefinition, AgentDefError>
   ```

   - frontmatter 手写分隔：首行须为 `---`，到下一个 `---` 行为止为 YAML mapping，其余为 body
     （trim 后为空则 body 为空串——local 定义允许空 body，只用骨架）。
   - 字段：`name`（缺省 = file_stem）、`description`（必填，缺则报错）、`kind`（`local` 缺省 /
     `acp`）、`tools`（接受 YAML 列表或逗号分隔字符串）、`model`、`max_steps`、`command`
     （acp 必填，列表）、`env`（map，可选）。未知字段报错（deny_unknown_fields 语义，手写校验）。
3. 错误类型 `AgentDefError`（thiserror，与 crate 现有错误风格一致）。

**验证**：
- 新增 `agent_def.rs` 内单测：local 全字段/最小字段、acp、name 缺省取 stem、description 缺失
  报错、frontmatter 缺失报错、未知字段报错、tools 两种写法、body 为空、body 保留 markdown
  结构。
- 门禁序列全绿。

**完成记录**（2026-07-22）：

- 改动：
  - 根 `Cargo.toml`：`[workspace.dependencies]` 加 `serde_yml = "0.0.12"`（附注释）；
    `crates/mag-config/Cargo.toml` 引用；`Cargo.lock` 更新。`cargo tree -p mag-config`
    确认无传递依赖爆炸——serde_yml 只带入 `indexmap`（与 toml 复用）/`itoa`/`libyml`
    （+`anyhow`）/`memchr`/`ryu` 一类叶子 crate，crate 仍保持轻量。
  - `crates/mag-config/src/agent_def.rs`（新，861 行含测试）：
    `AgentKindDef::{Local{model,tools,max_steps}, ExternalAcp{command,env}}`、
    `AgentDefinition{name,description,kind,body,source}`、
    `DefinitionSource::{Builtin,User,Project,Toml}`（派生 `Ord`，声明序 = §3.2 覆盖优先级
    低→高，M2-2 merge 可直接用）、`AgentDefError`（thiserror + `#[non_exhaustive]`，风格
    对齐 `ConfigError`；六个变体 `MissingFrontmatter`/`UnterminatedFrontmatter`/`Yaml`/
    `MissingField`/`UnknownField`/`Validation`，均带 file stem 便于 M2-2 记 warn 跳过）、
    `parse_agent_md`（手写 `---` 分隔按字节偏移切分，serde_yml 只解析 frontmatter
    mapping；未知字段手写校验并附已知字段清单；容忍 UTF-8 BOM 与 CRLF 行尾）。
  - `crates/mag-config/src/lib.rs`：导出 `agent_def` 模块全部 pub 项；crate 文档依赖边界
    行补 `serde_yml`，Entry points 补 `parse_agent_md`。
- 测试（`agent_def.rs` 模块内 18 个，全离线，`cargo test -p mag-config` 25+34 全绿）——
  TODO 清单逐项：local 全字段 `local_full_fields_parse`、local 最小字段
  `local_minimal_fields_use_defaults`、acp `acp_definition_parses_command_and_env`、
  name 缺省取 stem `name_defaults_to_file_stem`、description 缺失/置空
  `missing_description_is_an_error`、frontmatter 缺失 `missing_frontmatter_is_an_error`、
  未知字段 `unknown_field_is_an_error`、tools 两种写法
  `tools_accept_yaml_list_or_comma_separated_string`、body 为空
  `empty_body_after_frontmatter_is_empty_string`、body 保留 markdown 结构
  `body_preserves_markdown_structure`；补充用例：`unterminated_frontmatter_is_an_error`、
  `acp_without_command_is_an_error`（含 `command: []`）、`kind_mismatched_fields_are_errors`、
  `unknown_kind_value_is_an_error`、`invalid_field_types_are_errors`、
  `malformed_yaml_is_an_error`、`empty_or_comment_only_frontmatter_yields_all_defaults`、
  `crlf_line_endings_parse`。
- 门禁结果：`cargo fmt --all -- --check` ✅；`cargo test -p mag-config` ✅（lib 25 含 18
  新测试 + 集成 34，0 失败）；`cargo clippy --all-targets -- -D warnings` ✅（workspace，
  0 warning）；`cargo test --workspace` ✅（32 套件全 ok，0 失败）；
  `cargo doc --no-deps --workspace` ✅。
- 偏差（均为实现形态，无语义偏差）：
  1. `parse_agent_md` 签名按任务单不带 `source` 参数；返回值 `source` 缺省
     `DefinitionSource::User`，rustdoc 注明由 M2-2 的目录加载器经 pub 字段覆写（备选是
     改签名带 source，与任务单冲突）。
  2. 任务单只列出 description 缺失 / command（acp）缺失 / 未知字段三类报错；实现额外把
     **kind 错配的已知字段**（local 上的 `command`/`env`，acp 上的 `tools`/`model`/
     `max_steps`）报 `Validation` 错误而非静默丢弃，依据是 §3.1 字段表的"仅 local"/
     "仅 `kind: acp`"作用域与本 crate "bad config is never silent" 原则；空 `command: []`
     视同缺失（argv 至少要有可执行文件）。
  3. `serde_yml` 钉 `0.0.12`：crates.io 上 0.0.13 已标 deprecated——整个 crate 变为转发
     `noyalib` 的 shim，不再是 serde_yaml 血统；0.0.12 是该 fork 最后真实版本，
     `^0.0.12` 语义上限 <0.0.13 不会被自动升级。若后续要迁移 `noyalib`/`serde_yaml_ng`
     属独立决策，留给 M2-R 评估。

### M2-2 [DONE] mag-config：四来源发现与合并 + 内置定义 + TOML 投影

**目标**：把内置定义、用户目录、项目目录、TOML 配置四个来源合并成一张注册表。

**现状锚点**：
- TOML 非绑定 agent → delegate 的映射逻辑现在在 `crates/mag-core/src/assembly.rs:593-626`
  （本任务把它**吸收**到 mag-config，M3-5 才从 assembly 删除调用点）。
- 内置工具清单：`crates/mag-tools/src/tools/mod.rs:31`（`read_file, list_dir, grep, shell,
  ask_user`）——内置 `explorer` 的 tools 取只读子集 `read_file, list_dir, grep`。

**改动点**：
1. `agent_def.rs` 续：

   ```rust
   pub struct AgentDefinitionRegistry { defs: BTreeMap<String, AgentDefinition> /* 按 name */ }
   impl AgentDefinitionRegistry {
       pub fn builtin() -> Self;                       // general-purpose + explorer
       pub fn load_user_dir(dir: &Path) -> Result<Self, AgentDefError>;   // 目录不存在=空表，非错
       pub fn load_project_dir(dir: &Path) -> Result<Self, AgentDefError>;
       pub fn from_toml_snapshot(snapshot: &ConfigSnapshot, bound_agent: &str) -> Self;
       pub fn merge(base: Self, over: Self) -> Self;   // over 同名覆盖 base
       pub fn get(&self, name: &str) -> Option<&AgentDefinition>;
       pub fn describe_for_tool(&self) -> String;      // 给 agent 工具 description 用的枚举文本
   }
   pub fn default_user_agents_dir() -> PathBuf;        // $XDG_CONFIG_HOME/mag/agents 否则 ~/.config/mag/agents
   pub fn project_agents_dir(cwd: &Path) -> PathBuf;   // <cwd>/.mag/agents
   ```

2. 内置定义（`const` 文本，source=Builtin）：
   - `general-purpose`：description=通用任务执行 agent；kind=Local{model:None, tools:None（继承
     supervisor）, max_steps:None}；body=通用任务执行指令（自主完成、汇报契约与骨架一致）。
   - `explorer`：description=只读代码探查；kind=Local{tools:Some([read_file,list_dir,grep])}；
     body=探查专用指令（只读、给结论与文件引用）。
   - 骨架文本本任务不实现（M3-3），body 里不写与骨架重复的内容。
3. `from_toml_snapshot`：遍历 `agents` 表中 `name != bound_agent` 的 entry → Local 定义
   （description = `role` 或兜底 `Local subagent \`<name>\``，body = `system_prompt` 或空，
   model、tools 取 enabled 名集、budget.max_steps 取 `ResolvedAgent.budget`）；遍历
   `external_agents` 表 → ExternalAcp 定义（kind 非 acp 的跳过并 warn）。逻辑整体从
   `assembly.rs:593-626` 平移并适配。
4. 目录加载：只读 `*.md`，逐个 `parse_agent_md`，单文件解析失败**不致命**——记 warn 并跳过
   （error 类型带路径，便于日志）。

**验证**：
- 单测：内置表含两个定义且字段正确；四来源合并优先级（toml > project > user > builtin，同名
  覆盖）；目录不存在/空目录容错；坏 md 跳过不致命；`from_toml_snapshot` 对
  `docs/CLI.md:243-279` 示例配置（reviewer + peer_acp）产出正确两条定义；`describe_for_tool`
  输出含全部名字与 description。
- 门禁序列全绿。

**完成记录**（2026-07-22）：

- 改动：
  - `crates/mag-config/Cargo.toml`：加 `tracing = "0.1"`（warn 日志所需；叶子日志门面，依赖
    边界不破，与 mag-core 同款直接声明——workspace deps 未统一管 tracing）。
  - `crates/mag-config/src/agent_def.rs`（续写，现约 1600 行含测试）：
    - `AgentDefinitionRegistry { defs: BTreeMap<String, AgentDefinition> }`（派生
      Clone/Debug/Default/Eq/PartialEq）：`builtin()`（`general-purpose` Local{全 None
      继承 supervisor} + `explorer` Local{tools: [read_file, list_dir, grep]}，body 为
      const 文本——只写类型专属指令，角色与汇报契约留给 M3-3 骨架，避免重复漂移）；
      `load_user_dir`/`load_project_dir`（共用私有 `load_dir(dir, source)`：目录不存在=空
      表，read_dir 其他错误报新增的 `AgentDefError::Io{path, source}`；只读 `*.md` 普通
      文件，路径先排序保证确定性，单文件读/解析失败 warn 跳过；同目录同名冲突按字典序
      后者胜并 warn）；`from_toml_snapshot`（逻辑平移自 `assembly.rs:593-626`，见偏差 2）；
      `merge(base, over)`（`BTreeMap::extend`，over 同名覆盖）；`get`；`len`/`is_empty`
      （任务单未列，测试与 M3 枚举需要，clippy 成对要求）；`describe_for_tool()`（头行
      `Available agent types:` + 每定义一行 `- <name>[(acp)]: <description>`，BTreeMap
      字典序稳定输出，external 标 `(acp)` 提前满足 M4-1 第 2 点）。
    - `default_user_agents_dir()`（镜像 `main.rs:234` 约定，env 读取委托纯函数
      `user_agents_dir_from(xdg, home)`，测试不改进程 env）；`project_agents_dir(cwd)` =
      `<cwd>/.mag/agents`。
    - `AgentDefError` 新增 `Io` 变体（`#[non_exhaustive]` 允许；仅在 read_dir 非
      NotFound 失败时返回，带目录路径）。
  - `crates/mag-config/src/lib.rs`：导出 `AgentDefinitionRegistry`/
    `default_user_agents_dir`/`project_agents_dir`；crate 文档依赖边界行补 `tracing`，
    Entry points 补注册表一条。
- 测试（`agent_def.rs` 模块内新增 10 个，全离线、tempdir；`cargo test -p mag-config`
  lib 35 + 集成 34 + doctest 1 全绿）——TODO 清单逐项：内置表
  `builtin_registry_contains_general_purpose_and_explorer`、四来源优先级
  `four_source_merge_priority_overrides_by_name`（user/project tempdir + TOML 快照四链
  merge，逐级断言 source 与字段）、目录容错 `missing_and_empty_directories_load_empty`、
  坏文件 `broken_markdown_is_skipped_not_fatal`（坏 md 排序在前仍不中止、非 `.md` 与
  `*.md` 目录被忽略）、TOML 投影 `toml_projection_matches_cli_example`（§4.2 示例逐字，
  reviewer + peer_acp 恰好两条、bound `default` 被排除、role/system_prompt 缺省走兜底
  description 与空 body）、`describe_for_tool_lists_all_names_and_descriptions`（全名字
  + description + `(acp)` 标注 + 字典序）；补充用例：
  `toml_projection_maps_role_prompt_model_tools_and_budget`（role→description、
  system_prompt→body、budget.max_steps→max_steps、disabled 工具被滤出 enabled 名集、
  换 bound_agent 改变排除项）、`toml_projection_skips_external_without_command`（偏差
  2）、`user_agents_dir_prefers_xdg_then_home`（含空值视同未设）、
  `project_agents_dir_is_cwd_dot_mag_agents`。
- 门禁结果：`cargo fmt --all -- --check` ✅；`cargo test -p mag-config` ✅（70 项全绿）；
  `cargo clippy --all-targets -- -D warnings` ✅（workspace，0 warning）；
  `cargo test --workspace` ✅（全套件 0 失败）；`cargo doc --no-deps --workspace` ✅
  （注册表 rustdoc 内含 no_run doctest 的四链 merge 组装示例，编译通过）。
- 偏差（均为实现形态，无语义偏差）：
  1. mag-config 新增 `tracing` 依赖——任务单要求"记 warn"，crate 原无日志门面；属
     "serde/serde_yml 一类叶子依赖"范畴，不引入 mag-core/agent-lib 依赖。
  2. `from_toml_snapshot` 对 `command` 为空的 external entry **warn 并跳过**而非照抄
     （assembly 旧逻辑原样复制）：M2-1 已把 `AgentKindDef::ExternalAcp.command` 定死为
     "never empty"，解析器侧空 command 即 `MissingField`，投影侧保持一致。external 定义
     的兜底 description 取 `External ACP subagent \`<name>\``（旧
     `ExternalDelegateBinding` 本无 description 字段，无可平移值）。
  3. `ResolvedAgent.budget.max_steps` 是 `u64`、定义为 `u32`：超出 u32 范围时 warn 并
     视同未设（沿用运行默认），不静默截断。
  4. 同目录同名冲突的胜者规则为任务单未定义项：实现按文件名字典序后者胜 + warn（先排
     序再加载，消除 read_dir 的平台序不确定性）；TOML 层 `[agents]` 与
     `[external_agents]` 同名时 external 胜 + warn。

### M2-R [TODO] M2 review：定义模型与加载

**内容**：review M2 diff——模型与设计 §3 的字段表逐项核对；优先级顺序；错误路径；serde_yml
依赖引入是否最小（无传递依赖爆炸，`cargo tree -p mag-config` 检查）；`from_toml_snapshot` 与
assembly 原逻辑语义等价（对照 `assembly.rs:593-626`）。
**验证**：门禁序列全绿；问题修复并附完成记录。

---

## M3 — mag-core：local 实例运行时与接线（核心）

### M3-1 [TODO] mag-service：实例生命周期 wire 事件 + ts-rs 再生成

**目标**：wire 层新增实例生命周期事件变体，供 M3-3 的实例驱动任务向 EventBus 发布、
service 层投影给所有 interface。

**现状锚点**：
- `Event` 枚举：`crates/mag-service/src/lib.rs:280-306`（`DelegationStarted/Finished/...` 即
  在此）；`ServiceEvent` 投影：`crates/mag-service/src/service.rs:527-566`；ts-rs 派生与
  `ui/packages/protocol` 生成管线为 W1 交付物（见归档
  `docs/archive/2026-07-22-mag-web/PLAN.md` W1 与该包内 README/scripts）。

**改动点**：
1. `Event` 新增（serde 向后兼容，只做新增）：

   ```rust
   AgentInstanceStarted { instance_id: String, agent_type: String, description: Option<String>, depth: u32 },
   AgentInstanceFinished { instance_id: String, agent_type: String, status: AgentInstanceStatusWire,
                           report: Option<String>, error: Option<String> },
   ```
   `AgentInstanceStatusWire { Completed, Failed, Cancelled }`（serde + ts-rs 派生，风格对照
   既有 `DelegationStatusWire`，`lib.rs:597`）。
2. `ServiceEvent` 对应变体与投影（service.rs:527-566 同构追加）；`session_id()` 归属正确。
3. ts-rs：新类型加 `#[derive(TS)]`（feature-gated，同既有 wire 类型）；按
   `ui/packages/protocol` 的生成流程重新生成 TS 声明，确认 diff 门禁无漂移。
4. **不删** `Delegation*` 旧变体（历史兼容，见 M3-5）。

**验证**：
- mag-service 内投影单测（对照 `driver.rs:1478+` 既有 wire 投影测试风格）；
  `cargo test -p mag-service`；ts-rs 生成物 diff 干净；门禁序列全绿。

### M3-2 [TODO] mag-core：`AgentInstanceRegistry` 实例注册表

**目标**：新增 `crates/mag-core/src/instances.rs`（模块骨架），提供跨工具调用、跨任务共享的
实例状态表。

**改动点**：
1. 类型：

   ```rust
   pub struct AgentInstanceRegistry { /* Arc 共享 */ }
   pub(crate) struct Instance {
       pub id: String,                    // "<type>-<n>"，per-type 递增计数
       pub agent_type: String,
       pub depth: u32,
       status: Mutex<InstanceStatus>,     // Running | Completed{report} | Failed{error} | Cancelled
       cancel: CancelHandle,              // agent_lib facade CancelHandle（agent-lib/src/facade/agent.rs:103）
       done: tokio::sync::Notify,         // 终态通知
   }
   ```

2. API：`next_id(type)`、`register(instance)`、`get(id)`、`complete(id, status)`（置状态 +
   `notify_waiters`）、`cancel(id)`（触发 CancelHandle + 置 Cancelled + notify）、
   `cancel_all()`（session 结束/cancel 级联用）、`list()`。
3. `CancelHandle` 的构造/触发 API 以 agent-lib `facade/agent.rs:103` 实际为准（实现时核对；
   若其构造不对外，则改为注册表存 `tokio::sync::CancellationToken`、实例驱动任务经
   `run_full_with_cancel` 的 cancel 参数桥接）。
4. 完成通知队列：`notifications: Mutex<VecDeque<String>>`（人读文本，如
   `"agent instance explorer-1 completed: <报告前 200 字符>"`），M3-6 消费。

**验证**：模块内单测——id 递增、complete/cancel 状态迁移 + Notify 唤醒、cancel_all、并发
register（`tokio::test` 多 task）。门禁全绿。

### M3-3 [TODO] mag-core：`agent` spawn 工具 + 实例驱动任务 + origin 路由 + 分层 prompt

**目标**：实现 `agent` 工具（异步 spawn，立即返回 `{id, status}`）与 local 实例的完整驱动。
这是本计划最核心的任务。

**现状锚点**：
- facade 工具构造：`agent-lib/src/facade/tool.rs:330` `Tool::function_with_schema(name, desc,
  schema, handler)`；handler 为 `Fn(ToolContext, Args) -> Future<Result<Out, Err>>` 的 async
  闭包，可自由捕获 `Arc` 共享状态；`ToolContext.cancel: CancellationToken`（tool.rs:60-79）。
  mag 现有范式：`driver.rs:1380-1398` 的 `facade_tool`。
- **!Send 纪律**：facade run future 刻意 `!Send`（`NonNull` drop guard），mag 现有方案是每会话
  OS 线程 + current_thread runtime + `LocalSet` + `spawn_local`
  （`crates/mag-core/src/session.rs:420,458-462`，注释 :11-17）。工具 handler 在 session
  driver task 内被 poll（即已在 LocalSet 内），**`tokio::task::spawn_local` 直接可用**；
  严禁 `tokio::spawn`（会编译失败）。
- origin 路由范式（复制对象）：`agent-lib/src/facade/delegate/handler.rs:227-248` 的
  `DelegationInteractionRouter`（`with_origin` + `tokio::select!` cancel 包装）；
  `Interaction::with_origin`/`InteractionOrigin::new` 均 pub（`agent_lib::agent::` 路径，
  mag 已在 `driver.rs:41-44` 使用）。
- child Agent 构建：facade `Agent::builder()`（`driver.rs:269-279` 即 mag 现有调用）；
  system prompt 设置——builder 若有 `.system()` 用之，否则 build 后首 run 前经
  `reconfigure(SetSystemPromptOverlay)`（facade 允许 run 间 reconfigure，
  `agent-lib/src/facade/agent.rs:624-629`）；以 builder.rs 实际 API 为准。
- 深度上限语义：8（沿用 `DEFAULT_MAX_DELEGATION_DEPTH` 语义），由注册表 depth 字段在 mag 侧
  实现（spawn 闭包捕获 depth+1）。

**改动点**（均落在 `crates/mag-core/src/instances.rs` 及新 submodule）：
1. **共享上下文** `InstanceSpawnContext`（Arc）：`registry`、`definitions:
   AgentDefinitionRegistry`、`client: Arc<dyn LlmClient>`（supervisor 共享）、
   `supervisor_model: ModelRef`、`plugins / tool 投影所需件`、`interaction: Arc<IpcApproval>`、
   `events: EventBus`、`depth: u32`。
2. **`agent` 工具**：schema `{type?: string（缺省 general-purpose）, task: string（必填）,
   description?: string}`；description 文本用 `definitions.describe_for_tool()` 枚举全部类型。
   handler 同步段：查 `definitions.get(type)`——未知类型立即返回错误并附可用列表；depth >= 8
   立即报错。然后：
   - 分层 prompt 组装：`SUBAGENT_SKELETON`（内置常量：角色=supervisor 子代理、开场为任务简报、
     预算内自主完成、不与终端用户交互、**最后一条消息是给 supervisor 的报告**——结论/改动/
     文件引用/遗留问题）+ `"\n\n"` + `def.body`；external 的 body 处理在 M4。
   - child 工具面：supervisor 的 facade 工具投影（复用 `tool_surface` 的投影逻辑，M3-5 抽出
     共用函数）按 `def.kind.tools` 过滤（`None`=全量继承；`Some=list∩supervisor 面`），再
     追加三工具自身（depth+1 的新 spawn ctx）。
   - child 审批策略：`ApprovalPolicy`（facade builder API，approval.rs:300-396）按 supervisor
     策略投影 + per-tool tiers。
   - origin router：`OriginRouter { label: instance_id, depth, parent: interaction.clone() }`
     实现 `agent::InteractionHandler`（含 cancel select 包装），作为 child 的
     `interaction_handler`。
   - child budget：`def.max_steps` 或默认常量 `DEFAULT_INSTANCE_MAX_STEPS = 16`；max_tokens
     与 supervisor 对齐。
3. **驱动任务**：`registry.register` 后 `tokio::task::spawn_local`：构建 child facade Agent
   （shared client clone），`run_full_with_cancel(task, cancel_handle)` 跑到终态；终态写
   `registry.complete(id, status)` + push 完成通知队列 + 发
   `Event::AgentInstanceStarted/Finished` 到 EventBus。报告文本 = 最后一个 assistant 文本
   （`RunOutput` 字段以实际为准，语义对照 agent-lib `final_turn_summary`）。
4. handler 立即返回 `ToolResult::text(json!({"id": id, "status": "running"}))`。

**验证**：
- 单测/集成测试用 FakeLlmClient（`test_support.rs:97`，`tool_use_stream` :287）：supervisor
  脚本 `tool_use("agent", {type:"general-purpose", task:...})` → 断言立即返回 running；
  child 的 LLM 调用共享同一 FakeLlmClient（脚本需覆盖交错，必要时给 FakeLlmClient 增加按
  请求内容路由脚本的能力——允许作为本任务的配套改动）；断言 registry 终态、EventBus 收到
  Started/Finished、`agent` 工具对未知类型/超深的同步报错。
- 门禁序列全绿。

### M3-4 [TODO] mag-core：`agent_result` 与 `agent_cancel` 工具

**目标**：拉取/取消配套工具。

**改动点**（`crates/mag-core/src/instances.rs` 续）：
1. `agent_result`：schema `{id: string, timeout_secs?: number（缺省 600）}`。handler：
   `tokio::time::timeout` 等该实例 `done.notify()`，同时 `tokio::select!` 上
   `ToolContext.cancel`（cancel 抢占范式对照 `agent-lib/src/agent/drive.rs:952-975`）。
   返回：完成→`{id, status:"completed", report}`；超时→`{id, status:"running"}`（**不**置
   失败，只是这次没等到）；未知 id→错误并附 `registry.list()`。
2. `agent_cancel`：schema `{id: string}`；`registry.cancel(id)`；返回终态。
3. 两工具对 supervisor 与 child 工具面均可用（同 M3-3 的 ctx 捕获）。

**验证**：FakeLlmClient + Gated 脚本（`test_support.rs:257`）控制 child 完成时机：阻塞等到
结果、超时返回 running、cancel 后 result 返回 cancelled、未知 id 报错。门禁全绿。

### M3-5 [TODO] mag-core：driver 接线 + 静态委派退役 + 审批 tier + cancel 级联

**目标**：把三工具接入 supervisor 工具面，删除旧静态委派路径，迁移审批语义。

**现状锚点（退役面，全部在 `crates/mag-core/src/`）**：
- `driver.rs`：本地 delegate 注册 :289-292、external :296-300；restore 对应 :383-394 +
  `prune_unregistered_delegates` :402-404；`delegate_worker` :1128-1170；
  `apply_delegate_start_tiers` :1245-1253 + `delegate_start_tool_name` :1261。
- `assembly.rs`：delegate 映射 :593-626（已被 M2-2 吸收）；`DelegateBinding` :463、
  `ExternalDelegateBinding` :486 及其访问器。
- 旧测试：`engine.rs:4553`（生命周期）、:4648（审批 pop）、:4872（resume 重注册）、:5075
  （prune）、:5284（external fake-acp）——删除或改写为新机制的等价测试（M5-1 补齐 e2e，
  本任务保证编译与既有其他测试绿）。

**改动点**：
1. `SessionDriver`：持有 `Arc<dyn LlmClient>` clone、supervisor `ModelRef`、
   `AgentDefinitionRegistry`（assembly 构建：builtin → user dir → project dir(会话 cwd) →
   TOML 投影，逐级 merge；`apply_config` 时重建 TOML 层并重 merge——定义只影响后续 spawn）。
2. `tool_surface`（:1061）：追加三工具（M3-3 的构造函数，depth=0 的 root ctx）；抽出
   "plugins → facade 工具"投影为共用函数供实例路径复用。
3. 删除上述退役面代码与 binding 访问器；`SessionBinding::resolve` 不再产出 delegates。
4. 审批：`[tools.agent]` / `[tools.agent_result]` / `[tools.agent_cancel]` 走现有 per-tool
   tier 机制（`apply_per_tool_tiers` :1267）；默认 tier = 继承默认策略。**附带验证**：
   `Approval::ask` 的 decider 返回类型 `ApprovalDecision`（agent-lib facade/approval.rs:210）
   是否含 Ask/暂停变体——若有，实现 per-type（`agent:<type>`）tier（读取 `[tools."agent:<type>"]`
   覆盖）；若无，本任务只做 per-tool，per-type 记入 follow-up（回复设计 §7 的偏差）。
5. cancel 级联：supervisor run cancel / session 结束时 `registry.cancel_all()`（落点：
   driver cancel 路径与 SessionDriver drop）。
6. restore 兼容：旧 session snapshot 可能含持久化 delegate spec——restore 不再注册任何
   delegate（facade 恢复构建器不再被调用 subagent），确认不崩且旧 delegate 静默消失；
   `history.rs:54` 的 Delegation 历史投影保留（只读旧数据）。
7. `apply_config`（:641）：定义注册表重建 + 注释更新（:620-640 的 delegate 相关注释同步改）。

**验证**：
- 全库 grep 退役面干净：`ask_`、`builder.subagent`、`prune_unregistered`、`DelegateBinding`
  在 mag workspace 无残留引用（agent-lib 库内 API 保留，不 grep 它）。
- 既有测试（除本任务删除/改写的 delegation 旧测试外）全绿；新增：session 启动后 supervisor
  工具面含三工具；`[tools.agent] approval="deny"` 时 spawn 被拒；restore 旧 snapshot 不崩。
- 门禁序列全绿。

### M3-6 [TODO] mag-core：完成通知推送（pivot 通道 + 空闲缓冲）

**目标**：实例完成时 supervisor 不干等也能感知。

**现状锚点**：
- pivot 通道：`AgentRunStream::interject_pivot(PivotMessage)`（agent-lib
  `facade/agent/stream.rs:784`）；`PivotMessage`/`PivotSource::Host{label}` 经
  `agent_lib::agent::` 路径可达（`state/queue.rs:92-110`，facade 未 re-export，直接用 agent
  路径，**无需改 agent-lib**）；窗口限制：仅 streaming 路径、step 边界、每窗口一条、纯文本
  turn 无窗口。
- mag 现有机制：`drain_pivots`（`driver.rs:898-921`）每次 poll 后重试 enqueue；run 结束后未
  消费 pivot 直接 drop（:925-937）——实例通知**不能** drop，要转入空闲缓冲。
- 空闲注入：run 空闲无任何通道，缓冲拼进下一次用户输入（`run_turn` :484 开头）。

**改动点**（`driver.rs`）：
1. driver 持有实例通知队列（M3-2 registry 内）的读取端；`drain_pivots` 扩展：user pivot 与
   实例通知一起 drain，实例通知以 `PivotSource::Host { label: format!("agent:{id}") }` 经
   `interject_pivot` 入队。
2. run 终态时未消费的实例通知不 drop，保留在队列；`run_turn` 开头把残余通知作为前缀块拼入
   用户输入（格式如 `[agent 实例通知]\n- explorer-1 completed: ...`）。
3. 通知文本在 registry `complete()` 时生成（含报告摘要截断）。

**验证**：测试——① run 中 spawn 实例 + child 完成，断言通知经 pivot 进入 supervisor 后续
LLM 请求的消息流（FakeLlmClient 记录请求内容可断）；② run 结束后完成的实例，通知在下一次
`send_message` 的 LLM 请求前缀出现；③ 无通知时行为零变化。门禁全绿。

### M3-R [TODO] M3 review：local 实例运行时

**内容**：
- review M3 全部 diff：spawn_local 纪律（全库无新增 `tokio::spawn`）；!Send 边界没有被破坏；
  origin 归因在审批事件里正确（`IpcApproval` 收到带 origin 的 interaction，对照
  `engine/approval.rs:1248` 旧测试语义）；退役面无残留；registry 并发与 cancel 竞态（cancel
  与 complete 同时发生的最终状态唯一）。
- 对照设计 §4/§5/§7/§8 逐条核对；per-type tier 的验证结论（M3-5 第 4 点）记录到完成记录，
  若是降级则更新 `docs/dyn-agents.md` §11。
**验证**：门禁序列全绿；问题修复并附完成记录。

---

## M4 — external 实例化

### M4-1 [TODO] mag-core：`agent` 工具 external 分派 + 生命周期 + fake-acp 测试

**目标**：`kind: acp` 的定义经同一 `agent` 工具 spawn，按实例拉起进程、完成回收。

**现状锚点**：
- `run_external_once`（M1-1 交付）；`ManagedExternalAgent` 组装逻辑（从旧
  `external_acp_delegate` 平移，`driver.rs:1183-1217`）：`AcpConfig::new(binary,
  args).with_timeout(120s).with_env(...)`、`ExternalSessionRegistry::with_worktree_manager(
  AcpAdapter, GitWorktreeManager)`、`ManagedExternalAgent::acp(...).session_handler(...)`；
  整个 external 面 feature-gated（`external-acp`）。
- fake-acp 测试基建：`engine.rs:4457` 的 fake-acp.sh 范式。

**改动点**（`crates/mag-core/src/instances.rs` 续，feature-gated）：
1. spawn handler 按 `def.kind` 分派：Local 走 M3-3 路径；ExternalAcp 走：
   - task = `def.body`（task 模板，非空时）+ `"\n\n"` + 调用方 `task`；
   - **每实例新建** `ExternalSessionRegistry`（进程隔离天然；回收由 `run_external_once` 的
     一次性语义保证，registry 随驱动任务 drop）；
   - 驱动任务内调 `run_external_once(name, &agent, &ids, task, Some(origin_router), budget,
     cancel)`；outcome.summary → 实例报告；`completed=false`/error → Failed。
   - origin router 复用 M3-3 的同一个实现（external 的 ACP 权限请求经它冒泡到 root）。
2. 启动审批：v1 由 `[tools.agent]` 统一 tier 覆盖（同 M3-5 结论）；external 类型在
   `describe_for_tool()` 文本中标注 `(acp)` 便于 model 辨识。
3. TOML `[external_agents]` 定义已由 M2-2 投影进注册表，本任务只需接通。

**验证**：
- fake-acp.sh 集成测试：spawn external → Started/Finished 事件、报告回收、进程不残留
  （registry drop 后无子进程）；cancel 路径；权限请求冒泡到 root（fake-acp 发
  session/request_permission，断言 InteractionRequested 带 origin）。
- feature off 时编译干净（`cargo check -p mag-core --no-default-features` 或对应 feature
  组合）。门禁全绿。

### M4-R [TODO] M4 review：external 实例化

**内容**：review M4 diff——进程生命周期（无泄漏：completed/failed/cancelled 三态后无子进程
残留）；feature gate 边界；origin 归因；与设计 §6 逐条核对。
**验证**：门禁序列全绿；问题修复并附完成记录。

---

## M5 — e2e 加固 + 文档

### M5-1 [TODO] mag-core：端到端集成测试

**目标**：补齐跨模块 e2e 场景（`engine.rs` 测试风格，`engine_with_config` :4329 基建）。

**场景清单**（每场景一个测试，全部离线）：
1. **并行多实例**：supervisor 一个 turn 内两次 `agent` 调用（两个 explorer），断言两实例并发
   推进（完成顺序乱序可接受）、各自报告可 `agent_result` 取回——FakeLlmClient 需支持
   supervisor/多 child 请求交错（若 M3-3 未做内容路由，本任务补：按请求首条 user message
   前缀或 system prompt 特征路由脚本）。
2. **报告契约**：child 的最终 assistant 文本原样成为 `agent_result` 的 report。
3. **审批冒泡**：child 内触发需审批工具（如 shell），断言 root 收到带 origin
   （delegate=实例 id、depth=1）的 InteractionRequested，respond 后 child 续跑。
4. **cancel 级联**：supervisor cancel → 运行中实例全部 Cancelled；`agent_result` 立即返回。
5. **external 生命周期**（fake-acp）：见 M4-1，本任务补与 local 混合的场景。
6. **定义加载 e2e**：tempdir 项目 `.mag/agents/foo.md` + 用户目录定义 + TOML 定义同名覆盖，
   断言 `agent` 工具 description 枚举与 spawn 行为。

**验证**：全部新测试绿且 < 1 分钟/个；门禁序列全绿。

### M5-2 [TODO] 文档更新：CLI.md agents 章节 + dyn-agents.md 状态

**目标**：
1. `docs/CLI.md`：§4.2 配置示例与结构图（:219-279）更新——`[agents.<name>]`/`[external_agents]`
   语义改为"subagent 定义来源"；新增 `~/.config/mag/agents/*.md` 与 `.mag/agents/*.md` 定义
   文件说明（frontmatter 字段表，照 `docs/dyn-agents.md` §3.1）；§4.4 生效时机表（:330-331）、
   §3.3（:182-200）与 §5（:374-390）的 delegation 段落重写为 `agent`/`agent_result`/
   `agent_cancel` 工具与实例模型；`ask_<name>` 相关描述删除。
2. `docs/dyn-agents.md`：状态行更新为"已实现"，并记录实现偏差（per-type tier 结论、通知通道
   实际形态等，以 M3-5/M3-6 完成记录为准）。
3. 根 `README.md` 若提及 delegation/subagent 一并更新（先 grep 确认）。

**验证**：文档内链接与文件引用有效（对照真实代码行号抽查）；门禁序列全绿（doc 任务至少
fmt + workspace test）。

### M5-R [TODO] M5 review：e2e 与文档

**内容**：review M5 diff——测试覆盖与场景清单一一对应、无 flaky（连跑 3 遍）；文档与实现
一致（抽查工具名、字段名、路径）。
**验证**：门禁序列全绿；问题修复并附完成记录。

---

## F-R [TODO] 全计划 review

**内容**：
- 对照 `docs/dyn-agents.md` 全量核对：D1–D8 逐条落地情况；§8 退役清单全库 grep 复核
  （`ask_`、`subagent(`、`prune_unregistered`、`DelegateBinding`）；§11 已知限制清单更新
  （新增实现中发现的限制）。
- 全量门禁：fmt → clippy → `cargo test --workspace`（含 feature 组合）→ doc；agent-lib
  仓库同序列。
- 抽查 rustdoc 覆盖（新 pub API 全部有文档）；ts-rs 生成物无漂移；secret 纪律。
- 修复全部发现的问题，必要时在依赖位置插最小修复任务。

**验证**：门禁全绿；`docs/dyn-agents.md` 状态与偏差记录最终一致；完成记录归档。
