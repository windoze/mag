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

### M2-R [DONE] M2 review：定义模型与加载

**内容**：review M2 diff——模型与设计 §3 的字段表逐项核对；优先级顺序；错误路径；serde_yml
依赖引入是否最小（无传递依赖爆炸，`cargo tree -p mag-config` 检查）；`from_toml_snapshot` 与
assembly 原逻辑语义等价（对照 `assembly.rs:593-626`）。
**验证**：门禁序列全绿；问题修复并附完成记录。

**完成记录**（2026-07-22）：review 范围 `git diff 90b0762..HEAD`（5 文件，+1664/-3），逐项
结论如下；发现两处小问题，已最小修复随本 review 提交。

- **检查单 1（字段表与优先级）**：`name`（缺省取 stem）/`description`（必填）/`kind`
  （local 缺省/acp）/`tools`（仅 local，列表或逗号分隔）/`model`（仅 local）/`command`/`env`
  （仅 acp）与 §3.1 字段表逐项一致。**发现**：`max_steps` 在 M2-1 任务规格与实现中存在但
  §3.1 字段表缺行——已补设计文档表行（`仅 local。步数预算上限；缺省用运行时默认`）。
  优先级 `Builtin < User < Project < Toml` 与 §3.2 一致：`DefinitionSource` 声明序 =
  优先级且派生 `Ord`，`merge` 经 `BTreeMap::extend` 实现 over 覆盖 base，
  `four_source_merge_priority_overrides_by_name` 四链逐级断言。
- **检查单 2（错误路径）**：description 缺失/置空/显式 null → `MissingField`；未知字段 →
  `UnknownField`（附已知字段清单）；frontmatter 缺失/未闭合 → `MissingFrontmatter`/
  `UnterminatedFrontmatter`；kind 错配字段（local 上的 `command`/`env`、acp 上的
  `tools`/`model`/`max_steps`）→ `Validation`——与字段表"仅 local"/"仅 `kind: acp`"
  作用域及本 crate "bad config is never silent" 原则一致；空 `command: []` 视同缺失
  （argv 至少需可执行文件）。错误信息均带 file stem + 字段名 + 期望/实际形态，可用性好。
- **检查单 3（依赖卫生）**：serde_yml 0.0.13 为 deprecated shim 的说法**经 crates.io 核实
  属实**（"DEPRECATED — `serde_yml` is unmaintained… a thin compatibility shim that
  forwards every call to `noyalib`"，[crates.io/crates/serde_yml](https://crates.io/crates/serde_yml)）；
  钉 `0.0.12` 正确（`^0.0.12` 语义上限 <0.0.13，不会被自动升级进 shim），**结论：维持
  现状**——迁移 `noyalib`/`serde_yaml_ng` 属独立决策，不在本里程碑。`cargo tree -p
  mag-config`：serde_yml 只带入 `indexmap`/`itoa`/`libyml`(+`anyhow`)/`memchr`/`ryu` 叶子
  crate；`tracing` 传递仅 `pin-project-lite`/`tracing-core`/`once_cell`，与 mag-core 同款
  日志门面，为"记 warn"所需的最小引入；无依赖爆炸，crate 依赖边界（不依赖
  mag-core/agent-lib）保持。
- **检查单 4（TOML 投影等价性）**：对照 `assembly.rs:593-626` 逐行核实。local 侧（排除
  绑定项、`role`→description 兜底文案逐字一致、model、enabled 工具名集）等价；
  `system_prompt` None→body `""`（旧 `DelegateBinding` 保持 Option，新模型 body 可空，
  语义等价）；`max_steps` 为任务规格新增维度（旧 binding 无 budget 字段，不属回归）。
  **偏差 1 核实**：external 空 `command` 旧逻辑确为**照抄**进 binding（无过滤），下游
  `driver.rs:1187-1194` 仅在构建 delegate 时 warn "delegation will fail when invoked"——
  `ask_<name>` 工具仍注册、调用时才失败；新逻辑改为投影期 skip+warn，是行为变化但属
  fail-fast 改进，且与 M2-1 解析器"空 command 即 `MissingField`"自洽，**结论：合理，
  记录于此**。另：`capabilities` 未进定义模型——旧 delegate 路径中它仅出现在 tracing
  span（`driver.rs:1191`），`list_sources` 仍直读 snapshot（`engine.rs:731`）不受影响，
  §3.1 md 格式本无此字段，符合设计。
- **检查单 5（冲突处理）**：同目录同名按文件名字典序后者胜+warn（路径先排序，消除
  `read_dir` 平台序不确定性，warn 文案与实际行为一致）；TOML 层 `[agents]` 与
  `[external_agents]` 同名 external 胜+warn。两条规则均为任务单未定义项的确定性裁决，
  有 warn，合理。
- **检查单 6（测试质量）**：agent_def 模块 29 个测试（18 解析 + 11 注册表）与两个任务
  验证清单一一对应；断言用 `matches!`/`contains` 而非全文比对，无脆弱断言；全部离线
  tempdir。**发现**：M2-2 偏差 3（u64→u32 溢出 warn+视同未设）声称的行为无测试覆盖，
  已补 `toml_projection_out_of_range_max_steps_is_ignored`（5e9 > u32::MAX →
  max_steps=None）。
- **修复内容**（随本 review 提交，均最小改动）：① `docs/dyn-agents.md` §3.1 字段表补
  `max_steps` 行；② `crates/mag-config/src/agent_def.rs` 新增溢出路径测试 1 个（并对
  新代码跑 `cargo fmt`）。其余代码零改动。
- **门禁结果**（全部实跑）：`cargo fmt --all -- --check` ✅；`cargo clippy --all-targets --
  -D warnings` ✅（0 warning）；`cargo test --workspace` ✅（32 套件全 ok，0 失败；
  mag-config lib 36 含新增测试）；`cargo doc --no-deps --workspace` ✅。

---

## M3 — mag-core：local 实例运行时与接线（核心）

### M3-1 [DONE] mag-service：实例生命周期 wire 事件 + ts-rs 再生成

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

**完成记录**（2026-07-22）：

- 改动：
  - `crates/mag-service/src/lib.rs`：`Event` 新增 `AgentInstanceStarted{id, instance_id,
    agent_type, description, depth}` 与 `AgentInstanceFinished{id, instance_id, agent_type,
    status, report, error}` 两变体（位于 `DelegationMessage` 之后；Option 字段沿用
    `#[serde(default, skip_serializing_if = "Option::is_none")]` 惯例）；新增
    `AgentInstanceStatusWire{Completed, Failed, Cancelled}`（`#[non_exhaustive]` +
    serde snake_case + feature-gated `#[derive(TS)]`，风格对照 `DelegationStatusWire`；
    不派生 `Default`——三态均终态、无缺省语义，与 `ToolStatusWire` 一致）；
    `ts_exports::export_ts` 按字母序注册新类型。`Delegation*` 旧变体逐字未动。
  - `crates/mag-service/src/service.rs`：`ServiceEvent` 同构追加两变体；`session_id()`
    归入 session 级 `Some(*id)` 分支；`From<Event>` 投影逐字段同构追加。
  - `ui/packages/protocol`（全部生成物，无手写 TS）：`cargo test -p mag-service
    --features ts-export export_ts` 重新生成——新增 `generated/AgentInstanceStatusWire.ts`
    （`"completed" | "failed" | "cancelled"`），`Event.ts`/`ServiceEvent.ts` 各追加两变体
    （Option 字段导出为 `field?: string | null`），`index.ts` 按排序插入 export 行。
- 测试（mag-service 内，全离线）：
  - lib.rs：`event_variants_round_trip_and_keep_stable_tags` 补两变体（稳定 tag
    `agent_instance_started`/`agent_instance_finished` + serde roundtrip）；新增
    `agent_instance_status_wire_round_trips_with_stable_tags`（三态 wire tag + roundtrip）。
  - service.rs：`service_event_variants_round_trip_and_keep_stable_tags` 补两变体；
    `event_projects_into_matching_service_event` 补 Started 与 Finished(Failed) 投影
    用例；`service_event_session_id_is_none_only_for_global_events` 补两变体 `Some(id)`
    归属断言；新增 `agent_instance_finished_projects_for_all_terminal_statuses`——
    Completed/Failed/Cancelled 三态逐一断言 Event→ServiceEvent 投影、payload 全字段
    保留（投影前后 JSON 相等）、`session_id()` 归属、tag + serde roundtrip。
- 门禁结果：`cargo fmt --all -- --check` ✅；`cargo test -p mag-service` ✅（29
  passed，0 失败）；`cargo clippy --all-targets -- -D warnings` ✅（0 warning）；
  `cargo test --workspace` ✅（全套件 0 失败）；`cargo doc --no-deps --workspace` ✅
  （mag-config 有 1 个既有 rustdoc warning，与本任务无关）；前端 `npx tsc --noEmit -p
  ui/packages/protocol/tsconfig.json` ✅；生成物 diff 即本任务提交内容，提交后
  `git diff --exit-code -- ui/packages/protocol/src` 无漂移。
- 偏差（仅实现形态，无语义偏差）：任务单代码块只列 payload 字段；两变体按既有
  `Delegation*` 惯例以 `id: SessionId` 为首字段——这是任务单第 2 点"session_id()
  归属正确"的前提（无该字段只能归全局），且 M3-3 的发布点本就持有 session 上下文。
  `AgentInstanceStatusWire` 不派生 `Default`（对照 `ToolStatusWire`；终态枚举无缺省
  语义，`DelegationStatusWire` 的 `#[default] Started` 是 `DelegationTrace` 字段缺省
  所需，此处无对应场景）。

### M3-2 [DONE] mag-core：`AgentInstanceRegistry` 实例注册表

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

**完成记录**（2026-07-22）：

- 改动：
  - `crates/mag-core/src/instances.rs`（新，约 560 行含测试）：`AgentInstanceRegistry`
    （`#[derive(Clone)]` 句柄，内裹 `Arc<RegistryInner>`，风格对照 `EventBus`；
    `RegistryInner` 持有 `counters: Mutex<BTreeMap<String,u64>>`、
    `instances: Mutex<BTreeMap<String,Arc<Instance>>>`（BTreeMap 使 `list()` 输出确定）、
    `notifications: Mutex<VecDeque<String>>`）；`Instance`（`pub id/agent_type/depth` +
    私有 `status: Mutex<InstanceStatus>`、`cancel: CancelHandle`、`done: Notify`，经
    `status()`/`cancel_handle()`/`done()` 访问）；`InstanceStatus`（`Running |
    Completed{report} | Failed{error} | Cancelled}`，`is_terminal()`）。
  - API：`next_id(type)`（per-type 计数从 1 递增，`"<type>-<n>"`）、`register(instance)
    -> Arc<Instance>`、`get`、`list`、`complete(id, status) -> bool`（首次终态迁移生效：
    置状态 + `notify_waiters` + 推通知；未知 id 或已终态返回 false）、`cancel(id) ->
    Option<InstanceStatus>`（首次迁移到 Cancelled 才触发 CancelHandle——已终态不覆盖、
    句柄至多触发一次，返回调用后状态快照）、`cancel_all()`（级联取消全部 Running
    实例，供 M3-5 session 结束/cancel 级联）、`drain_notifications()`。
  - `crates/mag-core/src/lib.rs`：注册 `mod instances;`（私有模块，全部项
    `pub(crate)`）。
- cancel 句柄最终选择：**直接用 agent-lib 的 `CancelHandle`**（任务单第 3 点的备选
  `CancellationToken` 未启用）。核实结果：`CancelHandle` 在 `agent-lib/src/facade/
  agent.rs:103` 为 `pub`（`new()`/`cancel()`/`is_cancelled()` 均 pub），且经
  `facade/mod.rs:54` re-export 到 `agent_lib::facade::CancelHandle`；mag-core 既有代码
  （`session.rs:28`、`driver.rs:47`）已在用同一路径。M3-3 驱动任务可直接把
  `instance.cancel_handle()` 传给 `Agent::run_full_with_cancel`。
- 竞态裁决：全部终态迁移走 `Instance::transition` 单一入口，**首次终态迁移胜出**
  （M3-R 要求的"cancel 与 complete 同时发生最终状态唯一"）：并发的
  complete/cancel/cancel_all 只有一个生效并产生恰好一条通知；`cancel` 只在胜出时
  触发 CancelHandle，不覆盖已达成的 Completed 报告。
- 通知队列：通知文本在迁移胜出时生成——`completed: <报告前 200 字符>`（按 char
  计、`\n` 折叠为空格、超长加 `...`）、`failed: <error 同样截断>`、`cancelled`
  （无摘要）；M3-6 经 `drain_notifications()` 消费。
- 测试（模块内 9 个，全离线，聚焦 `cargo test -p mag-core instances` 全绿）——TODO
  清单逐项：id 递增 per-type 独立 `ids_increment_per_type_independently`；complete
  状态迁移 + Notify 唤醒 + 通知 `complete_transitions_status_wakes_waiters_and_
  notifies`（含二次 complete 幂等）；cancel 迁移 + 句柄触发 + 唤醒
  `cancel_fires_handle_transitions_and_wakes_waiters`；cancel_all
  `cancel_all_cancels_only_running_instances`（已终态不动、句柄不触发、三态通知
  顺序断言、二次级联幂等）；并发 register
  `concurrent_registration_allocates_unique_ids`（8 task × 10 id，current_thread +
  `tokio::spawn`，80 id 唯一无空洞）；补充用例：
  `register_get_and_list_round_trip`、`complete_notification_previews_and_flattens_
  long_reports`（200 字符截断 + 换行折叠）、`cancel_after_completion_keeps_the_
  completed_report`（complete 胜出竞态）、`unknown_ids_are_no_ops`。Notify 唤醒断言
  统一用 `Notified::enable()` 先注册再迁移的无竞态范式（`done()` rustdoc 已注明
  M3-4 的 `agent_result` 必须沿用同一范式，否则错过 check 与 wait 之间的迁移）。
- 门禁结果：`cargo fmt --all -- --check` ✅；`cargo test -p mag-core instances`
  ✅（9 passed，0 失败）；`cargo clippy --all-targets -- -D warnings` ✅（0
  warning）；`cargo test --workspace` ✅（32 套件全 ok，0 失败）；
  `cargo doc --no-deps --workspace` ✅（mag-config 1 个既有 rustdoc warning，与
  本任务无关，同 M3-1 记录）。
- 偏差（均为实现形态，无语义偏差）：
  1. 任务单代码块写 `pub struct AgentInstanceRegistry`；实现统一 `pub(crate)`——
     消费方（M3-3/3-4 工具、M3-5 接线、M3-6 通知 drain）全在 mag-core crate 内，
     interface 只经 wire 事件观察实例生命周期，不扩大 crate 公共表面。
  2. 通知不只 `complete()` 生成：`cancel`/`cancel_all` 的胜出迁移同样推通知
     （单一迁移入口自然结果；supervisor run cancel 级联后残留通知正好供 M3-6
     下一次输入前缀消费）。首次迁移胜出保证每实例恰好一条。
  3. API 清单补 `drain_notifications()`——任务单第 4 点要求 M3-6 消费队列，无
     drain 则队列无读取端。
  4. `mod instances;` 暂挂 `#[allow(dead_code)]`（lib.rs 处附注释）：骨架的消费
     者 M3-3 才落地，否则 `clippy -D warnings` 门禁被 dead_code 卡住；M3-3 接线
     时移除。
  5. `register` 签名为 `register(instance: Instance) -> Arc<Instance>`（注册并返回
     共享句柄），调用流程 `next_id` → `Instance::new` → `register`，与 M3-3
     "register 后 spawn_local"的顺序一致。

### M3-3 [DONE] mag-core：`agent` spawn 工具 + 实例驱动任务 + origin 路由 + 分层 prompt

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

**完成记录**（2026-07-22）：

- 改动：
  - `crates/mag-core/src/instances/spawn.rs`（新，约 1300 行含测试）：`InstanceSpawnContext`
    （Clone + 手写 Debug；registry、definitions、`Arc<dyn LlmClient>`、`supervisor_model:
    ModelRef`、`Arc<ToolRegistry>`、`ApprovalOverrides`、`Arc<IpcApproval>`、EventBus、
    session_id、`worktree`、depth；`child_context()` 以 depth+1 派生 child 面 ctx）；
    `SUBAGENT_SKELETON`（§4：角色=supervisor 子代理、开场为任务简报、预算内自主完成、
    审批经 origin 冒泡不与终端用户直接交互、**最后一条消息是给 supervisor 的报告**——
    结论/改动/`path:line` 引用/遗留）+ `layered_system_prompt`（空 body 只用骨架）；
    `MAX_INSTANCE_DEPTH = 8`（沿用 agent-lib `DEFAULT_MAX_DELEGATION_DEPTH` 语义：
    深度为 8 的 spawner 被拒，最深实例 depth=8）；`DEFAULT_INSTANCE_MAX_STEPS = 16`；
    `AGENT_TOOL_NAME = "agent"`；`agent_tools(ctx) -> Vec<Tool>`（M3-4 在此追加
    `agent_result`/`agent_cancel`，child 面经同一函数预留）；`agent` 工具
    （`Tool::function_with_schema`，description 内嵌 `describe_for_tool()`；schema
    `{type?, task, description?}`；handler 同步段：未知类型报错附可用列表、depth >= 8
    报错、`kind: acp` 报错待 M4、register + 发 `AgentInstanceStarted` +
    `tokio::task::spawn_local` 驱动任务，立即返回 `{"id", "status":"running"}`）；
    `OriginRouter`（`agent::InteractionHandler`：`with_origin(instance_id, depth)` +
    `tokio::select!` cancel 包装，范式照抄 `DelegationInteractionRouter`；
    cancelled 回退 `cancelled_interaction_result` 本地复刻——agent-lib 的对应函数
    crate-private）；驱动任务 `drive_instance`/`drive_local`（child facade Agent：shared
    client clone、model = def.model 或 supervisor、max_tokens 对齐 supervisor、
    max_steps = def 或 16、child 工具面 = 投影 ∩ def.tools + `agent` 自身、审批 =
    supervisor 策略投影 + per-tool tiers、interaction_handler = OriginRouter、child 的
    `ask_user` 桥也经 OriginRouter 冒泡；`run_full_with_cancel(task,
    instance.cancel_handle())`；报告 = `RunOutput.reply.text()`，即最后一个 assistant
    文本——语义对照 agent-lib `final_turn_summary`，`run_full` 内部正是用它组装）；
    终态 `registry.complete` 后**回读**实例权威状态发 `AgentInstanceFinished`
    （cancel 竞态胜出时发 Cancelled，不覆盖）。
  - `crates/mag-core/src/driver.rs`：`tool_surface` 的插件投影段抽出为
    `pub(crate) fn project_tool_plugins`（过滤 + permission ask tiers；per-tool tiers
    与 delegate start tiers 仍由 `tool_surface` 按原序叠加——**优先级逐字未动**：
    delegate tiers 先、per-tool 后），供实例路径复用（M3-5 第 2 点的"抽共用函数"提前
    落地）；`apply_per_tool_tiers` 提 pub(crate)；`IpcUserInteractionBridge` 提
    pub(crate) 并从 `Arc<IpcApproval>` 泛化为 `Arc<dyn InteractionHandler>`（supervisor
    仍传 IpcApproval，child 传 OriginRouter）。
  - `crates/mag-core/src/instances.rs`：`mod spawn;`；`Instance.depth` 语义修正为
    **1-based**（direct child = 1，见偏差 1）；模块文档更新。
  - `crates/mag-core/src/lib.rs`：移除模块级 `#[allow(dead_code)]`（registry 核心 API
    已被 spawn 真实消费）；少数 M3-4/5/6 才消费的 API（`Instance::done`、
    `AgentInstanceRegistry::{new, list, cancel, cancel_all, drain_notifications}`）与
    M3-5 接线入口 `agent_tools` 暂时挂**条目级** allow 并附注释。
  - `crates/mag-core/src/test_support.rs`（配套改动）：`FakeLlmClient` 新增
    `scripted_routes(Vec<RequestRoute>)`——按请求内容路由脚本（`RequestRoute::new` 任意
    谓词 / `system_contains`（child 的骨架 system prompt 特征）/ `user_text_contains`
    （child 开场任务简报）/ `any`（supervisor 兜底），顺序匹配、每路由独立 FIFO、未匹配
    或路由耗尽报错）；既有 FIFO 行为零变化。
- system prompt 设置方式：facade `AgentBuilder` **有** `.system()`（agent-lib
  `src/facade/agent/builder.rs:182`），直接用于组装好的两层 prompt，未走 reconfigure。
- 测试（`spawn.rs` 模块内 12 个 + test_support 2 个新路由用例，全离线，聚焦
  `cargo test -p mag-core instances::` 21 项全绿；每个测试手工装配 supervisor facade
  Agent + `agent` 工具，驱动在 current_thread runtime + `LocalSet` 内，!Send 纪律与
  session actor 一致）：
  - `spawn_returns_running_immediately_and_completes_with_report`——立即返回
    `{"id":"general-purpose-1","status":"running"}`（从 supervisor 第二次请求的 tool
    result 断言）、registry Completed（报告文本正确）、EventBus Started→Finished 顺序与
    全字段（session id、depth=1、description）、child 请求断言（system=骨架+body 两层、
    开场 user message=task、model/max_tokens 与 supervisor 对齐）；
  - `unknown_agent_type_errors_with_available_list`（附可用列表、无实例无事件）、
    `missing_or_invalid_arguments_error`、`depth_limit_errors_synchronously`（ctx
    depth=8 直接构造）；
  - `child_approval_bubbles_to_root_with_origin_and_resumes`——child 内 gated `shell`
    暂停 → root 收 `InteractionRequested`（origin.delegate=实例 id、depth=1、非 root），
    respond 后 child 续跑完成，且 shell 结果确实进入 child 后续请求（对照
    `engine/approval.rs:1248` 旧测试语义）；
  - `cancel_parked_instance_marks_it_cancelled`——审批暂停中 `registry.cancel`：句柄
    触发、终态 Cancelled、Finished(Cancelled)、通知恰好一条（OriginRouter cancel 包装
    生效）；
  - `child_run_failure_marks_instance_failed`（路由耗尽 → Failed + Finished error 透传）；
  - `child_inherits_full_surface_when_tools_unset`（None=全量+agent）、
    `child_surface_intersects_definition_tools`（explorer 只读子集+agent）、
    `child_surface_skips_unknown_definition_tools`（tempdir 定义 `tools: read_file,
    ghost` → ghost 跳过）；
  - `agent_tool_description_enumerates_definitions_and_requires_task`、
    `layered_prompt_combines_skeleton_and_body`；
  - test_support：`routes_match_by_system_marker_then_user_text_then_fallback`、
    `exhausted_route_and_unmatched_request_error`。
- 门禁结果：`cargo fmt --all -- --check` ✅；聚焦测试（instances 21、test_support 3、
  driver 16）✅；`cargo clippy --all-targets -- -D warnings` ✅（0 warning）；
  `cargo test --workspace` ✅（全套件 0 失败）；`cargo doc --no-deps --workspace` ✅
  （mag-config 1 个既有 rustdoc warning，同 M3-1/M3-2 记录，与本任务无关）。
- 偏差（均为实现形态，无语义偏差）：
  1. `Instance.depth` 从 M3-2 注释的"0 = root supervisor 派生"修正为 **1-based**
     （direct child = 1）：wire `Event::AgentInstanceStarted.depth` 文档（M3-1 已发布
     的契约）与旧委派 origin depth 语义（`engine/approval.rs:1248` 测试 depth=1）
     均为 1-based，统一避免两处 off-by-one；`InstanceSpawnContext.depth` 是 **spawner**
     的深度（root=0），实例 depth = spawner+1，深度检查 `spawner >= 8` 与 agent-lib
     `SubagentDepthExceeded` 语义逐字对齐。
  2. `InstanceSpawnContext` 增加任务单字段表未列的 `worktree: WorktreeRef`（mag-tools
     经 `ToolContext.worktree` 解析相对路径，child 必须拿到 session cwd，属"工具投影
     所需件"）与 `session_id`（事件首字段所需，M3-1 偏差记录已预告"发布点持有 session
     上下文"）。
  3. `OriginRouter.parent` 类型取 `Arc<dyn InteractionHandler>`（任务单写
     `Arc<IpcApproval>`）：照抄 `DelegationInteractionRouter` 范式，M4 external 路径
     以 `parent_interaction: Option<Arc<dyn InteractionHandler>>` 复用同一实现。
  4. M3-5 第 2 点的"抽共用函数"提前落地（`project_tool_plugins`）：child 工具面本任务
     就要用，避免复制投影逻辑；`tool_surface` 的 tier 叠加顺序（delegate start tiers →
     per-tool tiers）逐字保持。
  5. lib.rs 模块级 allow 按任务单移除；但 `agent` 工具接进 `SessionDriver` 工具面是
     M3-5 的事，故 `agent_tools` 与 M3-4/5/6 专用 API 挂条目级 `#[allow(dead_code)]`
     并附注释（与 M3-2 同一手法、粒度更细）。
  6. Cancelled 终态的 `AgentInstanceFinished` 事件 `report`/`error` 均为 `None`（wire
     文档称 error 是"failure or cancellation detail"，取消细节由 status 自身充分表达）。
  7. supervisor 面与 child 面的交集过滤：本任务 ctx 只持 registry 全量（`def.tools` ∩
     session 注册表）；session binding 级的 supervisor 面收窄（`agents.<name>.tools`）
     若要再交一层，M3-5 接线时给 ctx 加一个 supervisor 面过滤字段即可（在此记录备查）。

### M3-4 [DONE] mag-core：`agent_result` 与 `agent_cancel` 工具

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

**完成记录**（2026-07-22）：

- 改动（均在 `crates/mag-core/src/`，无新依赖）：
  - `instances/spawn.rs`：`AGENT_RESULT_TOOL_NAME = "agent_result"` /
    `AGENT_CANCEL_TOOL_NAME = "agent_cancel"` / `DEFAULT_RESULT_TIMEOUT_SECS = 600`；
    `agent_tools(ctx)` 扩展为三件套（supervisor 与 child 面经同一函数同时获得，child 面
    测试断言同步更新）。`agent_result`（`Tool::function_with_schema`，schema
    `{id, timeout_secs?}`；handler 为 async：`parse_instance_id` → `timeout_secs` 校验
    （非负整数，缺省 600，`0` = 立即轮询一次）→ `registry.get`（未知 id 报错附
    `unknown_instance_error` 的实例表）→ `tokio::select! { biased; timeout(..,
    await_terminal) => .., tool_ctx.cancel.cancelled() => 报错返回 }`——`await_terminal`
    逐字采用 `Instance::done()` rustdoc 的无竞态范式（check → `Notified::enable` →
    re-check → wait，同测试 `wait_terminal`），cancel 抢占形状对照 agent-lib
    `fulfill_batch_cancellable`（drive.rs:952-975，等待为主分支、cancel 为抢占分支）。
    返回：Completed→`{id, status:"completed", report}`、Failed→`{..., status:"failed",
    error}`、Cancelled→`{..., status:"cancelled"}`（`instance_status_json` 统一渲染）、
    超时→`{id, status:"running"}`（ToolStatus::Ok，**不**置失败、实例不转台）。
    `agent_cancel`：handler 同步（同 `agent` 工具风格）：`registry.cancel(id)` 的
    first-terminal-wins 转台与 cancel handle 触发都在应答前完成，返回的已是终态快照；
    未知 id 同样报错附实例表。`unknown_instance_error`：空表提示
    "no agent instances have been spawned"，非空逐行列 `id (agent_type, status)`。
  - `instances.rs`：移除 `Instance::done` / `AgentInstanceRegistry::list` /
    `AgentInstanceRegistry::cancel` 三个条目级 `#[allow(dead_code)]` 及其"M3-4 消费"
    注释（现被两工具真实消费）；`new`/`cancel_all`/`drain_notifications` 的 allow 保留
    （M3-5/M3-6 消费）；模块文档同步。
- 测试（spawn.rs 新增 4 个 + 既有 3 个 child 面断言更新 + 1 个工具列表断言改写，全离线，
  聚焦 `cargo test -p mag-core instances` 25 项全绿）：
  - `agent_result_blocks_until_the_instance_completes`——child 停在新增 `GatedStubTool`
    （`StreamGate` 卡住的 stub 工具，test_support 预留用法）内；supervisor drive 用
    `spawn_local` 并发，断言 `agent_result` 阻塞期间 supervisor 不推进（请求数不变、
    实例 Running），开闸后收到 `{status:"completed", report}`；
  - `agent_result_timeout_returns_running_without_failing`——`timeout_secs: 0` + 永不
    开闸：立即返回 `{status:"running"}`（ToolStatus::Ok），实例保持 Running 不转台；
  - `agent_cancel_then_result_returns_cancelled`——child 停在 gated `shell` 审批上，
    supervisor 连发 `agent_cancel` → `agent_result`：两者都返回 `{status:"cancelled"}`
    终态快照，cancel handle 已触发，Cancelled 的 `AgentInstanceFinished` 经 OriginRouter
    cancel 包装及时到达；
  - `unknown_instance_id_errors_with_instance_list`——空表报错提示未 spawn；spawn 后
    `agent_result`/`agent_cancel` 对未知 id 报错并附 `general-purpose-1` 实例表；
  - `agent_tools_expose_spawn_result_and_cancel`（原 `agent_tool_description_…` 改写：
    三工具齐备、schema required 与 `timeout_secs` 声明）；child 面三测试断言工具名列表
    追加 `agent_cancel`/`agent_result`。
  - 测试基建：requests 的 `tool_results` 收集全量历史，断言取每个请求的**最后一个**
    tool result（每 step 恰好一个新结果）；新增 `await_until` 轮询助手与 `GatedStubTool`。
- 门禁结果：`cargo fmt --all -- --check` ✅；聚焦测试（instances 25）✅；
  `cargo clippy --all-targets -- -D warnings` ✅（0 warning）；`cargo test --workspace`
  ✅（全套件 0 失败）；`cargo doc --no-deps --workspace` ✅（mag-config 1 个既有
  rustdoc warning，同 M3-1/M3-2/M3-3 记录，与本任务无关）。
- 偏差（均为实现形态，无语义偏差）：
  1. Failed 终态的返回形态任务单未列，按 Completed 对称补 `{id, status:"failed", error}`
     （Cancelled 同理 `{id, status:"cancelled"}`；父任务测试要求"cancel 后 result 返回
     cancelled"即此形态）。
  2. `timeout_secs: 0` 允许（`tokio::time::timeout` 零时长 = 轮询一次现状），成为
     §5.1"立即返回当前状态"的 escape hatch；非整数/负数报错。
  3. Gated 控制点落在 stub 工具而非 LLM 流：child 走非流式 `chat` 端点，
     `StreamScript::Gated` 在该端点退化为事件拼接（`test_support.rs:62-66`），故用
     `StreamGate` 的 stub 工具用法（其 rustdoc 预留的第二种用途）卡住 child 的工具执行
     来控制完成时机。

### M3-5 [DONE] mag-core：driver 接线 + 静态委派退役 + 审批 tier + cancel 级联

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

**完成记录**（2026-07-22）：

- 改动（`crates/mag-core/src/`，另 `crates/mag/tests/engine_cli.rs`；无新依赖）：
  - `driver.rs`：
    - `SessionDriver` 新状态：`spawn_ctx: Arc<InstanceSpawnContext>`（depth=0 root ctx：
      会话级 `AgentInstanceRegistry::new()`、`SharedSpawnState`（四层定义表 + supervisor
      `ModelRef`）、共享 `Arc<dyn LlmClient>` clone、`Arc<ToolRegistry>`、
      `ApprovalOverrides`、`Arc<IpcApproval>`、EventBus、session_id、worktree（session cwd，
      缺省 `"."`））+ `cwd` 字段（apply_config 重读 project 定义目录用）。build 后回读
      `agent.state().current_model()` 校正 cell（restore 时快照模型才是权威值）。
    - `tool_surface` 追加 `agent_tools(root_ctx)`（插件投影之后、per-tool tiers 之前——
      三工具默认继承策略默认 tier，无 derived gate（§7）；`[tools.agent]` 等经既有
      `apply_per_tool_tiers` 生效）。`assemble_agent_definitions`（新自由函数）：
      builtin → user dir（`default_user_agents_dir`）→ project dir（`project_agents_dir(cwd)`）
      → TOML 层逐级 `merge`；目录层 IO 错误 warn + 跳过（缺失目录 = 空层）。
    - 退役面全删：本地/external delegate 注册循环（new/restore 各一）、`delegate_worker`、
      `external_acp_delegate`、`split_external_command`、`external_worktree_root`、
      `apply_delegate_start_tiers` + `delegate_start_tool_name`、
      `TrackedExternalSessionHandler` + `external_handlers` 字段 +
      `cleanup_external_sessions`（session.rs 调用点同步删——该机制只服务静态 external
      委派，随其整体退役）。
    - **restore 保留 `.prune_unregistered_delegates()`（偏差 1）**：零 re-registration 下它
      是纯单向旧名册清扫。
    - cancel 级联：`run_turn` 两条 Cancelled 路径（stream 建立失败的早退 + 循环终态）都
      `registry.cancel_all()`（Completed/Failed 不级联——spawn 是异步的，实例可合法存活过
      本 turn）；`impl Drop for SessionDriver` 做 session 结束级联（实例永不跨会话存活）。
    - `apply_config`：`set_definitions` 重建定义表（**全部四层重建**，目录层重读——见偏差
      2）；supervisor model 经"镜像排队的 SetModel"更新（facade reconfigure 只排队、turn
      边界才落地，且 `AgentRunStream` 无 state 访问器、stream 持有 `&mut agent` 无法回读——
      镜像值与请求同源构造；facade 对 `SetModel` 的 admission 只拒 blank model/非有限
      temperature/provider_extras 不匹配，本 driver 构造路径三者均不可能）。注释（
      new/restore/tool_surface/apply_config/`project_tool_plugins`）同步改写。
    - `reconfig_requests` 的 `ReplaceToolSet` 投影追加三工具声明（`agent_tools(&spawn_ctx)`
      `.declaration()`）——surface 收窄不再剥离实例面（对齐旧 ask_ 面"收窄不剥 delegation"
      语义），且顺带用新定义表刷新 `agent` 描述。
  - `assembly.rs`：`DelegateBinding`/`ExternalDelegateBinding` 及访问器、resolve 的两段
    delegate 映射全删；`SessionBinding` 改产 `agent_definitions: AgentDefinitionRegistry`
    （= `from_toml_snapshot(snapshot, bound)` 的 TOML 层，无配置后端时为空）+ 访问器；
    `assemble_tool_registry` 的"未知名 warn"豁免三工具名（它们是 facade 级工具，
    `[tools.agent]` 是合法条目）；模块/字段注释同步。
  - `instances.rs`：`mod spawn` 提 `pub(crate)`；移除 `AgentInstanceRegistry::new` /
    `cancel_all` 的条目级 `#[allow(dead_code)]`（`drain_notifications` 的保留，M3-6 消费）；
    模块文档更新。
  - `instances/spawn.rs`：`InstanceSpawnContext` 的 `definitions` + `supervisor_model` 两
    字段合并为 `shared: SharedSpawnState`（新类型：`Arc<RwLock>` 可换 cell，poison 恢复，
    锁不跨 await；`set_definitions`（apply_config）/`set_supervisor_model`（build 校正 +
    apply 镜像）/`definition()`（clone）/`describe_for_tool()`/`supervisor_model()`）；
    移除 `agent_tools` 的 `#[allow(dead_code)]`；root 与 child ctx 共享同一 cell（定义表
    重建对任意深度的后续 spawn 生效）。
  - `session.rs`：`session_thread` 向 `SessionDriver::new`/`restore` 传 `session_id` +
    `event_bus.clone()`；删 actor 收尾的 `cleanup_external_sessions` 调用（driver Drop 即
    级联）。
- 审批验证结论（任务单第 4 点）：agent-lib `facade/approval.rs` 的 `Approval::ask`
  decider 返回类型 `ApprovalDecision`（`agent/approval.rs:76`）**只有
  `Approve / Deny / Timeout / Cancel` 四变体，无 Ask/暂停变体**（暂停语义由
  `ApprovalKind::Ask` tier 自身表达，decider 只返回决定）→ 本任务只做 per-tool tier，
  per-type（`agent:<type>`）记入 follow-up（设计 §7 的既定偏差）。per-tool 实测语义：
  `[tools.agent] approval="deny"` 在 mag 恒注入 interaction handler 下仍**暂停**经
  IpcApproval，界面答 Deny 后 spawn 被拒（denied tool result 回馈模型，无实例无事件）。
- external 旧组装参数记录（M4-1 按新机制重建时对照；全部随 `external_acp_delegate`
  删除）：
  - spec：`ManagedExternalAgent::acp(binary, args).session_handler(handler)` +
    `.worktree(cwd)`（session cwd 存在时）；argv 首元素为 binary；
  - handler：`AcpConfig::new(binary, args).with_timeout(120s)` + 逐项
    `with_env(k, v)`（delegate env 覆盖）+ `with_working_dir(cwd)`；
    `ExternalSessionRegistry::with_worktree_manager(Arc::new(AcpAdapter::new(acp_config)),
    GitWorktreeManager::new().with_root(temp_dir/mag-external-worktrees-{pid}-{counter}))`；
    `RegistryExternalSessionHandler::new(registry)` 外包 `TrackedExternalSessionHandler`
    （记录每次 drive 的 child `AgentId`，session 结束逐 id `registry.cleanup_agent` 清扫
    completed 常驻 session）；
  - tier：start 工具 `ask_<name>` 默认 ask（`apply_delegate_start_tiers`），
    `[tools.ask_<name>]` 覆盖最终生效；
  - 事件：facade `DelegationStarted/Finished/Failed/Message` 经 `map_wire_event` 投影（该
    投影与 `delegation_trace_from_wire`/`delegation_message_from_wire` 保留——wire 契约
    与 history 投影的既有形状；facade 在零 delegate 下不再产生这些事件）。
  - 新机制下 M4-1 应改用 M1-1 的 `run_external_once` 一次性语义（进程随终态回收，无需
    tracked-handler 常驻清扫）。
- 测试：
  - 新增 driver 级 4 项：`new_exposes_the_agent_instance_tools`（surface 含三工具、
    `agent` 描述枚举 builtin 定义）、`apply_config_rebuilds_the_definition_table_for_later_spawns`
    （apply 后 researcher 定义可解析 + model cell 镜像 model-b + 下一请求确实落到
    model-b）、`restore_sweeps_legacy_delegates_from_an_old_snapshot`（向真实快照 JSON
    注入 legacy delegate 配方 + `ask_legacy` 声明——typed 形状取自快照自身；restore 不
    崩、surface 无 ask_legacy、`subagents()` 为空、三工具在面）、
    `drop_cancels_all_running_instances`（Drop 级联：实例转 Cancelled、cancel handle
    已触发）。
  - 新增 engine 级 4 项：`supervisor_surface_exposes_the_instance_tools_and_definitions`
    （session 启动后 surface 含三工具、无 `ask_*`（`ask_user` 除外）、描述枚举
    general-purpose/explorer/researcher 三层来源）、`agent_spawn_denied_by_the_per_tool_tier`
    （deny 暂停 → 答 Deny → 无 AgentInstanceStarted、denied result（ToolStatus::Denied）
    回馈、run 正常收尾）、`supervisor_run_cancel_cascades_to_running_instances`（child
    卡在 gated stub 工具内确证 Running；cancel → RunError(Cancelled) +
    AgentInstanceFinished(Cancelled)，顺序不限）、
    `resume_restores_the_instance_surface_and_spawns_definitions`（persist 模块：重启
    resume 不崩、surface 有三工具无 ask_*、TOML 层 researcher 定义可 spawn 并
    Completed(report)——实例可存活过 turn，Finished 事件单独等）。
  - 旧测试处置：engine.rs `mod delegation` 十个退役测试全删（生命周期/审批 pop/拒绝与
    批准 start/resume 重注册/apply 保 ask_ 面/prune/external 三件套），保留的 sources
    listing 测试独立为 `mod external_sources`；`engine/approval.rs` 的
    `delegate_interaction_pops_to_root_with_origin_and_resumes_on_response` 删（唯一直接
    用 `builder.subagent` 处；同语义已由 M3-3
    `child_approval_bubbles_to_root_with_origin_and_resumes` 覆盖），origin 路由两测试
    （`with_origin` 直接标注，不依赖静态委派）保留；persist 模块历史测试改写为
    `get_session_history_restores_messages_and_tools_after_restart`（去掉 ask_researcher
    生产路径——`history.rs` 的 Delegation 投影保留为只读旧数据，但已无活 producer，
    其 live-producer 覆盖待 M5-1 e2e）；driver.rs 四个 config-apply 断言补三工具名；
    assembly.rs 两 delegate 测试改写为定义层投影断言；`crates/mag/tests/engine_cli.rs`
    两测试改写（dialog+reload 保留、delegation 腿删除并补三工具面断言；external+resume
    改为纯 `/resume` 流程，fake-acp 基建删除——CLI 对 `AgentInstance*` 事件尚无渲染
    （`_ => {}`），实例面 CLI e2e 待 M5-1）。
  - `session_binding::explicit_empty_tool_list_builds_a_tool_less_session` 等三个
    engine 断言补三工具（`tools = []` 现剩三工具——语义文档同步）。
- 门禁结果：`cargo fmt --all -- --check` ✅；聚焦测试（driver 20、instances::spawn 16 +
  registry 9、assembly 12、engine::instances 3、engine::persist 6、mag engine_cli 4）✅；
  `cargo clippy --all-targets -- -D warnings` ✅（0 warning，另 `--no-default-features`
  check ✅）；`cargo test --workspace` ✅（32 套件 375 passed 0 failed）；
  `cargo doc --no-deps --workspace` ✅（mag-config 1 个既有 rustdoc warning，同
  M3-1..M3-4 记录，与本任务无关）。
- grep 验证（mag workspace，agent-lib 不查）：`.subagent(` 0 命中；`DelegateBinding` 0
  命中；`ask_` 命中均为活机制（`ask_tool`/`ask_user`/`ApprovalPolicyKind::Ask`）、
  `history.rs:184` 的保留投影、restore 注释与 legacy 测试夹具；`prune_unregistered` 2
  命中（driver.rs restore 的调用 + rustdoc，见偏差 1）。
- 偏差：
  1. **`.prune_unregistered_delegates()` 保留在 restore**（任务单退役清单与 grep 验证把它
     列入删除）：实证（agent-lib `facade/agent/snapshot.rs:800-843` rustdoc + restore
     build 语义）不 re-register 且不 prune 时持久化 delegate 以 auto_allow **复活**（违背
     "旧 delegate 静默消失"且静默放开审批）；清空 snapshot 字段则 agent_state 里的
     `ask_<name>` 声明失去执行体，`ensure_restored_tool_surface` 直接 InvalidState（违背
     "确认不崩"）。prune 是 agent-lib 当前表面上唯一同时满足两者的机制；零
     re-registration 下它只删不建，不构成静态委派机制的延续。driver.rs:289-294/354-359
     注释如实记录。
  2. `apply_config` 重建**全部四层**定义表（任务单写"重建 TOML 层并重 merge"）：单一
     `assemble_agent_definitions` 代码路径复用于构建与 apply，目录层重读属无害超集
     （还能拾取定义文件编辑）。
  3. supervisor model 经 apply_config 镜像排队的 `SetModel`（而非 facade 状态回读）：
     facade reconfigure 只排队、turn 边界落地，stream 持有 `&mut agent` 且 `AgentRunStream`
     无 state 访问器；镜像值与请求同源（`ModelRef::new(model, current.max_tokens(),
     current.temperature(), None)`），facade 对 `SetModel` 的 admission 拒绝情形本
     driver 均不可能产生。
  4. supervisor 面 `agent` 工具描述在 build 时烘焙；apply_config 换定义表只影响 handler
     侧解析（任务单语义即"只影响后续 spawn"），描述文本仅在 `ReplaceToolSet` 触发时
     （`agents.<name>.tools` 有配）随之刷新——纯展示层限制，记录在案。
  5. CLI（`mag-cli`）对 `AgentInstanceStarted/Finished` 尚无渲染（`_ => {}` 忽略）；两
     个 CLI e2e 的 delegation 腿删除而非改写（M5-1 补齐新机制 CLI e2e）。
  6. 附带修复：driver 测试 helper 的 `EventBus` 现与 `IpcApproval` 共享同一实例（原各自
     新建）——root ctx 与审批事件同源。

### M3-6 [DONE] mag-core：完成通知推送（pivot 通道 + 空闲缓冲）

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

**完成记录**（2026-07-22）：

- 改动（`crates/mag-core/src/`，无新依赖、未动 agent-lib）：
  - `instances.rs`：通知项结构化为 `InstanceNotification { id, text }`（`id` 是
    `PivotSource::Host { label: "agent:<id>" }` 归因所需——纯文本队列无法给出结构化 id，
    见偏差 1）；`drain_notifications() -> Vec<InstanceNotification>` 的
    `#[allow(dead_code)]` 移除（本任务消费）；`push_notification` 文本格式不变；模块文档
    同步（M3-6 由"待接线"改为现状）。
  - `driver.rs`：
    - `SessionDriver` 新字段 `pending_notifications: VecDeque<InstanceNotification>`
      （new/restore 均初始化为空）——从 registry drain 出但尚未落到 pivot 窗口的通知的
      重试缓冲；run 终态**不 drop**（与 user pivot 的 drop 语义明确区分），随 driver 跨
      turn 存活。
    - `drain_pivots` 扩展（新增 `registry` + `pending_notifications` 两参数，保持自由
      函数——`stream` 持有 `&mut self.agent`，方法化会撞借用）：每次 poll 后先把
      `registry.drain_notifications()` 并入缓冲（FIFO），再依次尝试 user pivot
      （`interject`）与实例通知（`interject_pivot` + `PivotSource::Host { label:
      format!("agent:{id}") }`）；`InvalidState`（窗口未开/本窗口已有一条）推回队首
      下次 poll 重试，其余错误按永久拒绝发 `PivotDropped`（通知的理由串带
      "instance notification" 前缀以区分）。
    - `instance_notification_pivot`：`PivotMessage::new(MessageId, user Message,
      PivotSource::Host)`——`PivotMessage`/`PivotSource` 经 `agent_lib::agent::` 路径
      引入（facade 未 re-export，未改 agent-lib）。message id 由
      `host_pivot_message_id()` 外部分配：高 64 位 = 进程唯一 tag（epoch 纳秒，
      OnceLock），低 64 位 = 进程级计数器——高位落在 facade 小计数器空间之外（同
      UUIDv7 待遇），既不与 facade 自配 id 冲突，restore 时 `continuing_after` 重播种
      扫描也会忽略（agent-lib `facade/ids.rs` 明文支持）；会话历史里重复 id 会被
      Conversation 软拒绝，此方案使其不可能发生。
    - `take_pending_notifications`：`run_turn` 开头把缓冲 + registry 的残余通知折成
      `[agent 实例通知]\n- <text>\n\n<用户输入>` 前缀块；stream 建立失败的早退路径把已
      drain 的通知放回缓冲（turn 未开始即未提交，通知不随折叠文本丢失）。
    - `run_turn` rustdoc 增补通知双通道语义段；run 终态注释明确"user pivot drop、
      通知保留"的分野。
  - `instances/spawn.rs`：一处测试断言同步为结构化通知（cancel 竞态测试）。
- 测试（全离线，engine.rs `mod instances` 新增 2 项）：
  - ① `instance_completion_pivots_into_the_running_supervisor`：双 gate 时序——
    supervisor 停在 gated `park_supervisor` 工具内、child 停在 gated `park_child`
    工具内（`ParkTool` 泛化出 `name` 字段，M3-5 cancel 测试构造点同步）；先放
    child 完成（`AgentInstanceFinished` 发出即通知已入队），再放 supervisor 到
    step 边界；断言 `PivotApplied`、run 正常收尾、第 3 个 streaming 请求的消息流含
    user 消息 `agent instance general-purpose-1 completed: child findings`（child 走
    非流式端点，streaming 请求全属 supervisor）。
  - ② `instance_completion_after_the_run_prefixes_the_next_turn_input`：child 停在
    gated 工具内，supervisor turn 1 正常收尾（无级联、无 PivotApplied）；run 空闲后
    放 child 完成；下一次 `send_message("next question")` 的第 3 个 streaming 请求
    的最后一条 user 消息以 `[agent 实例通知]\n` 开头、含
    `- agent instance general-purpose-1 completed: late findings\n`、以原输入结尾。
  - ③ 零变化：无通知路径只多出两次空 drain；既有 pivot 6 项、driver 20 项、
    instances::spawn 16 项、registry 9 项全绿即证。
- 门禁结果：`cargo fmt --all -- --check` ✅；聚焦测试（engine::instances 5、
  instances 30、driver 20、pivot 6）✅；`cargo clippy --all-targets -- -D warnings`
  ✅（0 warning）；`cargo test --workspace` ✅（32 套件 377 passed 0 failed）；
  `cargo doc --no-deps --workspace` ✅（mag-config 1 个既有 rustdoc warning，同
  M3-1..M3-5 记录，与本任务无关）。
- 偏差：
  1. `drain_notifications()` 返回类型由 `Vec<String>` 改为
      `Vec<InstanceNotification>`（任务单只锚定 API 名，未锚定签名）：pivot 归因
      `PivotSource::Host { label: format!("agent:{id}") }` 需要结构化 id，从渲染文本
      回解析会把 driver 耦合到通知文案格式；M3-2 的字符串断言已同步为结构化断言
      （`complete_notification_previews_and_flattens_long_reports` 现同时钉住 id）。
  2. 窗口限制的接受范围与任务单一致：仅 streaming 路径、step 边界、每窗口一条、
     纯文本 turn 无窗口——未落地窗口的通知不丢，转入下一次 turn 的前缀块（测试②
     覆盖的正是该路径）。

### M3-R [DONE] M3 review：local 实例运行时

**内容**：
- review M3 全部 diff：spawn_local 纪律（全库无新增 `tokio::spawn`）；!Send 边界没有被破坏；
  origin 归因在审批事件里正确（`IpcApproval` 收到带 origin 的 interaction，对照
  `engine/approval.rs:1248` 旧测试语义）；退役面无残留；registry 并发与 cancel 竞态（cancel
  与 complete 同时发生的最终状态唯一）。
- 对照设计 §4/§5/§7/§8 逐条核对；per-type tier 的验证结论（M3-5 第 4 点）记录到完成记录，
  若是降级则更新 `docs/dyn-agents.md` §11。
**验证**：门禁序列全绿；问题修复并附完成记录。

**完成记录**（2026-07-22）：review 范围 `git diff 55c296c..HEAD`（17 文件，+5491/-1887，
M3-1..M3-6 六任务）。逐项检查单结论如下；发现一处 §7 语义缺口（supervisor 面收窄不约束
child 面，防提权意图落空），已最小修复随本 review 提交；另 §11 per-type tier 条目按实证
结论更新措辞。

- **检查单 1（!Send / spawn_local 纪律）**：diff 全量 grep `tokio::spawn`——生产代码零
  新增；唯一新命中是 `instances.rs` 并发 register 单测（registry 自身 Send，与 facade
  无关）及 spawn.rs 模块文档注释。实例驱动走 `instances/spawn.rs` 的
  `tokio::task::spawn_local`（session `LocalSet` 内，child Agent 在该 future 内构建并
  驱动到底），facade !Send 边界未破坏。
- **检查单 2（origin 归因）**：`OriginRouter`（spawn.rs:594 起）与 agent-lib
  `DelegationInteractionRouter`（`../agent-lib/src/facade/delegate/handler.rs:227-248`）
  逐行同构——`with_origin(InteractionOrigin::new(label=实例 id, depth))` +
  `tokio::select! { biased; cancel => .., parent.fulfill => .. }` 包装；
  `cancelled_interaction_result` 逐变体复刻 crate-private 的
  `cancelled_delegation_interaction_result`（Approval→Deny "interaction cancelled" /
  Question→空答 / Choice→0 / Permission→cancel）。depth 1-based：root ctx depth=0、
  实例=spawner+1；测试断言 `origin.delegate == "general-purpose-1"`、`depth == 1`、
  `!is_root()`，与旧委派测试（`engine/approval.rs:1248`）语义一致。
- **检查单 3（退役面无残留）**：mag workspace grep——`DelegateBinding` / `.subagent(` /
  `delegate_worker` / `apply_delegate_start_tiers` / `delegate_start_tool_name` /
  `external_acp_delegate` / `split_external_command` / `TrackedExternalSessionHandler` /
  `cleanup_external_sessions` 均 0 命中；`prune_unregistered` 2 命中即 M3-5 偏差 1 的
  保留调用 + rustdoc（driver.rs:302/368）；`ask_` 命中全为活机制（`ask_user`/`ask_tool`）、
  `history.rs:184` 只读旧数据投影、注释与 legacy 测试夹具。**偏差 1 论证成立**：agent-lib
  `facade/agent/snapshot.rs:793-837` rustdoc 实证——不 re-register 且不 prune 时持久化
  delegate 以 `ApprovalPolicy::default`（auto_allow）复活且 `ask_<name>` 留在工具面；
  prune 在零 re-registration 下是纯单向清扫（声明随 recipe 一起从 current/initial tool
  set 删除，不触发 InvalidState）。抽查测试
  `restore_sweeps_legacy_delegates_from_an_old_snapshot`：真实快照注入 legacy recipe +
  `ask_legacy` 声明 → restore 不崩、surface 无 `ask_legacy`、`subagents()` 为空、三工具
  在面。
- **检查单 4（registry 并发与竞态）**：全部终态迁移走 `Instance::transition` 单一入口
  （status Mutex 内判 terminal + 置位，出锁后 `notify_waiters`）——cancel 与 complete
  并发时首次迁移胜出、每实例恰好一条通知；`cancel`/`cancel_all` 仅在胜出时触发
  CancelHandle（句柄至多一次、不覆盖已达成 Completed 报告）。`agent_result` 的
  `await_terminal` 逐字为 check → `Notified::enable` → re-check → wait 无竞态范式
  （enable 先于状态检查，落在 check 与 wait 之间的迁移必然唤醒）；cancel 抢占 select 形状
  对照 agent-lib `fulfill_batch_cancellable`。run 两条 Cancelled 路径 +
  `impl Drop for SessionDriver`（driver.rs:891）均级联 `cancel_all`。
- **检查单 5（分层 prompt §4 + 工具面交集 §7）**：§4 逐条一致——`SUBAGENT_SKELETON`
  覆盖角色（supervisor 子代理）/开场=任务简报/预算内自主/审批经 origin 冒泡不直达用户/
  最终消息=报告（结论、改动、`path:line`、遗留），`layered_system_prompt` = 骨架 +
  `"\n\n"` + body（空 body 只用骨架），builder `.system()` 一次性传入（§4"两层拼好
  一次性传入"）。**§7 发现缺口**：child 面交集此前只交 session 注册表全量
  （`def.tools ∩ registry`），supervisor 绑定收窄（`agents.<name>.tools`）不约束
  child——`tools` 缺省时 child 面可能宽于 supervisor 自身，§7"缺省=继承 supervisor
  工具面 / 显式取交集（防提权）"落空；M3-3 偏差 7 把它留给 M3-5，M3-5 未落地也未记录。
  修复见下。
- **检查单 6（M3-6 通知通道）**：pivot 归因 `PivotSource::Host { label:
  format!("agent:{id}") }`（driver.rs:1025），id 来自结构化 `InstanceNotification` 而非
  文案回解析；`PivotMessage`/`PivotSource` 经 `agent_lib::agent::` 路径引入（未改
  agent-lib）；run 终态只 drop user pivot、通知留 `pending_notifications`
  （driver.rs:570-575 分野注释明确），`take_pending_notifications` 折
  `[agent 实例通知]` 前缀块、stream 建立失败早退路径放回缓冲（driver.rs:474-476）；
  message id 外部分配高位 = epoch 纳秒 tag——agent-lib `facade/ids.rs:72-75` 明文
  `continuing_after` 只考虑低 64 位空间的 id，高位非零者被忽略、facade 小计数器永不与之
  冲突，restore 重播种不受影响（实证一致）。
- **检查单 7（§8 退役清单 + §11）**：§8 七行逐条——① `ask_<name>` 合成工具退役 ✓
  （grep + engine/CLI 三处 surface 断言无 `ask_*`）；② build/restore 静态注册退役 ✓
  （零 `.subagent(`）；③ `prune_unregistered_delegates` 偏差保留，论证成立（见检查单 3）；
  ④ `apply_delegate_start_tiers` tier 迁移——实际落 per-tool `[tools.agent]`（三工具无
  derived gate，插件投影后追加、再 `apply_per_tool_tiers`），per-type 留 follow-up；
  ⑤ `agents.<name>.system_prompt` 保留为 TOML 定义层 ✓（M2-2 已核）；⑥
  `[external_agents]` 常驻连接退役 ✓（`external_acp_delegate` 删除，per-instance 拉起属
  M4）；⑦ `Delegation::single_tool` 复用——实际以 facade `Tool::function_with_schema`
  自建三工具，无 agent-lib 耦合（实现形态差异，§8 语义目标达成）。per-type tier 验证结论
  复核：`agent-lib/src/agent/approval.rs:76-85` 实证 `ApprovalDecision` 只有
  Approve/Deny/Timeout/Cancel 四变体、无 Ask/暂停变体，M3-5 结论属实；§11 条目已更新为
  实证措辞（修复 2）。
- **检查单 8（测试质量）**：M3 新增测试与各任务验证清单一一对应（registry 9、
  instances::spawn 22、driver 21、engine::instances 5、persist/CLI 改写等）；时序同步全部
  走 gate（`StreamGate`/`GatedStubTool`/`await_until`，5s 兜底超时、卡住即 bug）——仅两处
  sleep：`await_until` 轮询间隔（gate 模式，非时序断言）与 spawn.rs 的 50ms 负断言
  （断言 `agent_result` 阻塞期间 supervisor 不推进——负断言无法 gate；单线程 runtime +
  充裕窗口，慢 CI 只放大窗口不致 flaky）；engine.rs 的 10ms 为带 5s 超时的正条件轮询。
- **检查单 9（mag-acp/mag-cli wildcard）**：mag-acp `map.rs:298` `_ => None`、mag-cli
  `lib.rs:1340` `_ => {}`——`AgentInstanceStarted/Finished` 落 wildcard，编译无感（serde
  向后兼容新增成立）；展示层本阶段**不补**（设计 §11"实例的 UI 展示…本阶段只落
  journal"，M3-5 偏差 5 已记 CLI 无渲染、e2e 待 M5-1），结论记录于此。
- **修复内容**（随本 review 提交，均最小改动）：
  1. **§7 supervisor 面过滤**（防提权缺口）：`SharedSpawnState` 增
     `surface: Option<Vec<String>>`（绑定 `agents.<name>.tools` 的 enabled 投影，与
     definitions/model 同 cell hot-swappable）；`drive_local` 有效 allowlist =
     `bound ∩ def.tools`（def 缺省=继承 bound、两者皆空=全量），实例三工具豁免（与
     supervisor 面"收窄不剥实例面"语义一致）；`root_spawn_context` 以 `binding.tools()`
     播种；`apply_config` 镜像更新（entry 无 tools 键=过滤不动，同 `ReplaceToolSet`
     规则）。新增测试 2 个：`child_surface_is_bounded_by_the_supervisor_surface`
     （缺省继承收窄面 + bound∩def 交集双向断言）、
     `spawn_surface_filter_follows_the_bound_tool_list`（build 播种 + apply 重播种 +
     无键不动）。
  2. `docs/dyn-agents.md` §11 per-type tier 条目由"视变体情况，可能只有 per-tool"更新为
     M3-5 实证结论。
- **门禁结果**（全部实跑）：`cargo fmt --all -- --check` ✅；
  `cargo clippy --all-targets -- -D warnings` ✅（0 warning）；
  `cargo test --workspace` ✅（32 套件 379 passed 0 failed，含新增 2 测试）；
  `cargo doc --no-deps --workspace` ✅（mag-config 1 个既有 rustdoc warning，同
  M3-1..M3-6 记录，与本任务无关）；`cargo check -p mag-core --no-default-features` ✅。

---

## M4 — external 实例化

### M4-1 [DONE] mag-core：`agent` 工具 external 分派 + 生命周期 + fake-acp 测试

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

**完成记录**（2026-07-22）：

- 改动（全部在 `crates/mag-core/src/instances/spawn.rs`）：
  - spawn handler 同步段：`external-acp` feature on 时 `AgentKindDef::ExternalAcp` 与 Local
    走同一注册/Started/`spawn_local` 路径；feature off 时 acp 类型 spawn 同步返回清晰错误
    （指明 rebuild 需启用 `external-acp`，默认开启）。
  - `drive_instance` 按 `definition.kind` 分派 `drive_local` / `drive_external`；
    feature off 时 `drive_external` 为同签名 stub（spawn 已同步拒绝，运行期不可达）。
  - `drive_external`（feature-gated）：task = `def.body` trim 后非空 + `"\n\n"` + 调用方
    task（`external_task`，§6 任务框架模板）；**每实例新建** `ExternalSessionRegistry`，
    组装参数照 M3-5 记录——`AcpConfig::new(binary, args).with_timeout(120s)` + 逐项
    `with_env`（定义 env）+ `with_working_dir(ctx.worktree)`、
    `GitWorktreeManager::new().with_root(temp_dir/mag-external-worktrees-{pid}-{counter})`、
    `RegistryExternalSessionHandler`；`ManagedExternalAgent::acp(binary,
    args).session_handler(..).worktree(ctx.worktree)`。registry 随驱动任务 drop，进程回收
    由 `run_external_once` 的一次性语义（三终态 detached sweep）保证，无需 M3-5 旧
    tracked-handler 常驻清扫。
  - 驱动任务内调 `run_external_once(name, &agent, &FacadeIds::new(), task,
    Some(origin_router), BudgetLimits::unbounded(), cancel)`：`parent_interaction` 复用
    M3-3 的 `OriginRouter`（label=实例 id、depth=实例 depth；agent-lib 内层
    `DelegationInteractionRouter` 的 origin 标注被它整体覆盖，与 local 路径一致冒泡到
    root `IpcApproval`）；`outcome.completed` → `outcome.summary` 为实例报告；
    `completed=false` → Failed，drive error → Failed。
  - cancel 桥接：`run_external_once` 要 agent 层 `CancellationToken`，而实例持有 facade
    `CancelHandle`（其 token 私有）。驱动任务 `tokio::select!` biased 竞争 drive future
    与既有 `await_terminal(instance)`（check→enable→re-check→wait 无竞态范式）：驱动在
    飞期间观察到终态迁移必是 registry `cancel`/`cancel_all` 胜（自身 complete 在 drive
    返回后才发生），随即 fire token 让一次性调用自行 abandon + sweep，再 `drive.await`
    收尸。无轮询、无泄漏 watcher。
  - `describe_for_tool()` 的 `(acp)` 标注为 M2 既有，本任务未动；启动审批维持 M3-5 结论
    （`[tools.agent]` 统一 per-tool tier），未额外做。
- 偏差：
  1. **budget 传 `BudgetLimits::unbounded()`**（任务单只写"传 budget"，§7"实例必须有预算
     兜底"未对 external 落地字段）：external 运行时对 mag 是黑盒，agent-lib 自身一次性
     调用测试也用 unbounded；external 实例的兜底是 ACP 120s 请求超时 + 实例协作式
     cancel。定义模型当前无 external budget 字段，留作后续。
  2. 空 `command` 防御：md frontmatter 解析已强制非空，TOML `[external_agents]` 投影不
     校验——`split_external_command` 返回 `Option`，空 command 在 drive 时映射为
     Failed（报文指明 argv 形式）。
  3. fake-acp.sh 范式从 M3-5 删除的 `engine.rs:4457` 基建恢复并简化为纯 argv 传参
     （log/mode/session），新增 `hang`（cancel 路径）与 `permission`（权限冒泡）两模式；
     响应 id 硬编码 1/2/3 的依据复核为 adapter `next_request_id` 从 0 预增
     （initialize=1、session/new=2、session/prompt=3）。
- 测试（`spawn.rs` 新增 `#[cfg(all(unix, feature = "external-acp"))] mod external_acp`，
  全离线、tempdir、共约 1.3s）：
  - `external_instance_completes_and_reclaims_the_process`：spawn → 立即 running →
    Completed(report="external summary")；Started/Finished 事件顺序与 wire 字段与
    local 路径一致；fake 日志含 initialize/session/new/session/prompt 且 prompt 文本为
    `Task frame template.\n\ninspect the vault`（模板拼接实证）；sweep 的
    session/cancel 到达 fake（SESSION_CANCELLED）→ 进程退出。
  - `external_instance_cancel_abandons_and_reclaims_the_process`：hang 模式卡 prompt →
    `registry.cancel` → Cancelled + Finished(Cancelled) 事件 → sweep 回收
    （session/cancel 到 fake）。
  - `external_permission_request_bubbles_to_root_with_origin`：fake 发
    `session/request_permission` → root 事件流 InteractionRequested 带
    origin(delegate="peer-1", depth=1)、kind=Permission → 答 Approve → 对端收到
    `"id":100` 应答后续跑完成。
  - `external_process_crash_marks_instance_failed`：crash_prompt 模式 → Failed + 非空
    诊断 + Finished(Failed)；drive 失败本身即进程已退出的实证（否则 wait_terminal
    超时）。
- 门禁结果（全部实跑）：`cargo fmt --all -- --check` ✅；聚焦测试（instances::spawn 21、
  instances 全量 36、mag-core 154）✅；`cargo check -p mag-core --no-default-features`
  ✅；`cargo clippy --all-targets -- -D warnings` ✅（0 warning）；
  `cargo test --workspace` ✅（32 套件全 ok、0 failed）；`cargo doc --no-deps
  --workspace` ✅（mag-config 1 个既有 rustdoc warning，同 M3-1..M3-6 记录）；新 4 测试
  复跑一遍仍全绿，测试后 `ps` 无 fake-acp 进程残留。

### M4-R [DONE] M4 review：external 实例化

**内容**：review M4 diff——进程生命周期（无泄漏：completed/failed/cancelled 三态后无子进程
残留）；feature gate 边界；origin 归因；与设计 §6 逐条核对。
**验证**：门禁序列全绿；问题修复并附完成记录。

**完成记录**（2026-07-22，review 范围 `5a4fa80..1b26754`，未改代码——未发现需修复的问题）：

1. **进程生命周期 ✅**：回收链路逐环核实——`drive_external` 在任一进程存在之前先返回的两条
   早退路径（空 command、`ManagedExternalAgent::build` 失败）无泄漏窗口；进程只能由
   `run_external_once` 内部拉起，agent-lib 侧三终态全覆盖（`delegate.rs:530` 未提交 outcome
   →drive 自扫；`delegate.rs:665` completed→wrapper 补扫；spawn 后 handshake 失败 →
   transport drop 时 `kill_on_drop` 兜底，`connection.rs:104-120`）；sweep 为 detached
   `tokio::spawn`，mag registry 随驱动任务 drop 不影响其在飞 sweep。测试后 `pgrep fake-acp`
   无残留。已知边界（agent-lib 既有、非 M4 回归）：驱动任务被整体 drop（会话 teardown）时只有
   `kill_on_drop` 杀直接子进程，孙进程依赖正常 sweep 的进程组终止。
2. **cancel 桥接 ✅**：`CancelHandle` 只在 registry `cancel`/`cancel_all` **赢得**首终态迁移后
   才 fire（`instances.rs:258-265`），而驱动自身的 `complete` 只在 drive 返回后发生——故
   select 在飞期间观察到终态必是 cancel 胜出，论证成立。`await_terminal` 为
   check→enable→re-check→wait 无竞态范式，基于 `Notify`，无轮询、无泄漏 watcher；`biased`
   让已就绪的 drive 优先，竞态残余由 registry 首终态胜出 + `drive_instance` 读回状态兜底。
   fire token 后 `drive.await` 有界（agent-lib 读循环与 cancellation `select!`，hang 测试实证）。
3. **feature gate ✅**：off 时 `cargo check -p mag-core --no-default-features`（含
   `--all-targets`）干净；spawn 同步拒绝报文明确（指明 rebuild 启用 `external-acp`、默认
   开启）；off 时 spawn 同步拒绝 ⇒ 无 external 实例可注册 ⇒ `drive_external` stub 运行期
   不可达，论证成立。
4. **origin 归因 ✅**：agent-lib 内层 `DelegationInteractionRouter`（delegate=定义名）标注后
   经 mag `OriginRouter.fulfill` 的 `with_origin` **整体覆盖**（`interaction.rs:105` 替换语义），
   最终 origin=(实例 id, 实例 depth)；测试实证 delegate="peer-1"、depth=1、kind=Permission、
   应答回到对端（fake 收到 `"id":100`）。
5. **偏差复核**：
   - ① `BudgetLimits::unbounded()` **结论：v1 可接受，§7 未被实质违反**。BudgetLimits 的
     step/token 维度对黑盒 external 不可观测，agent-lib 自有一次性调用同样传 unbounded；
     §7"定义可声明 budget"在定义模型中本无 external 字段可映射。兜底实况：120s 是 ACP
     transport 的**每读 idle 超时**（`AcpConfig::timeout` → `SpawnedAcpAgent.read_timeout`，
     M4-1 记录"120s 请求超时"措辞在此更正）——静默对端每读最多挂 120s → SessionLost →
     Failed + sweep；不需要的运行由协作式 cancel 终止。**残余缺口**：持续有输出但永不完成
     的对端无 wall-clock 上限，留作 §11 已知限制（M5-2/F-R 收录）。
   - ② 空 command Failed 映射保留，但偏差前提更正：TOML 投影**有**校验（`agent_def.rs:850`
     空 command 跳过 + `io.rs:156` 加载期拒绝空 argv），md 解析亦强制非空——该 Failed 映射
     经两条真实配置路径均不可达，属无害纵深防御，不动。
   - ③ fake-acp argv 传参核实无误：adapter `next_request_id` 从 0 预增（`adapter.rs:287`），
     initialize=1/session/new=2/session/prompt=3 与脚本硬编码一致；`"id":100` 为对端自发
     请求的独立 id 空间。
6. **设计 §6 逐条 ✅**：按实例拉起（每实例独立 registry+进程）✅；md 正文作 task 框架模板
   拼在 task 前（`external_task`，测试实证 `Task frame template.\n\ninspect the vault`）✅；
   多实例=多进程（per-instance 组装 + pid/counter 唯一 worktree root）✅；权限请求 origin
   冒泡 root ✅（见 4）；分层 prompt 不适用（body 只进 task 文本）、报告=对端最终消息 ✅；
   TOML `[external_agents]` 与 md `kind: acp` 同为注册表来源（M2-2 投影）✅。
7. **测试质量 ✅**：4 测试断言到位——事件全字段等值断言、ACP 握手三帧、模板拼接、sweep 的
   session/cancel 到达 fake（进程退出的实证：fake 收到 cancel 即写标记并 `exit 0`，crash 测试
   以 drive 在 5s 内返回 Err 反证进程已退）；无 flaky 因子（全部 `await_until`/`wait_terminal`
   5s 兜底、无裸 sleep；`cfg(all(unix, feature = "external-acp"))` 显式门控）；复跑 2 遍全绿
   （0.95s/0.71s）。

**门禁**：`cargo fmt --all -- --check` ✅；`cargo clippy --all-targets -- -D warnings` ✅
（0 warning）；`cargo test --workspace` ✅（全套件 ok、0 failed）；`cargo doc --no-deps
--workspace` ✅（mag-config 1 个既有 rustdoc warning，同 M3 起记录）；`cargo check -p
mag-core --no-default-features`（含 `--all-targets`）✅；external 4 测试复跑 2 遍 ✅。

---

## M5 — e2e 加固 + 文档

### M5-1 [DONE] mag-core：端到端集成测试

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

**完成记录**（2026-07-22）：

- 改动（仅 `crates/mag-core/src/engine.rs`，无生产代码改动、无新依赖）：`mod instances`
  新增 6 个 engine 级 e2e 测试（真 `Engine` + `ConfigService` 配置 + 会话事件流全链路，
  FakeLlmClient scripted_routes 按内容路由 supervisor/child 请求），模块文档更新；
  `StubTool` 补 `permission` 字段（场景 3 的 gated shell 需要，与 `external_sources` 同款）；
  新测试基建均为模块内私有：`collect_events_until`（累计事件断言）、`drain_ready`
  （非阻塞排空）、`poll_until`（5s 兜底轮询）、`tool_results`、`instance_starts`/
  `instance_finishes`（事件提取）、`EnvGuard`（env 变量 save/restore）、cfg 门控的
  `fake_acp_script`/`toml_string`。
- 场景覆盖对应关系（每场景一测试，全离线，6 测试合计约 2s）：
  1. 并行多实例 → `two_explorer_instances_run_concurrently_and_report_through_agent_result`：
     一个 turn 内两次 `agent` 调用 spawn 两个 explorer；并发实证为结构性断言——两 child 各自
     完成首次 LLM 请求并停在共享 gated `read_file`（explorer 只读面 ∩ 注册表）内、且此刻无
     任何 Finished 事件（supervisor 阻塞在 `agent_result` 中）；开闸后两报告乱序完成、各自
     经 `agent_result` 取回（请求 4/5 的 tool result 逐一断言）。同类型两 child 无法按
     system prompt 区分，路由用 `user_text_contains`（开场任务简报）——M3-3 的
     scripted_routes 够用，**无需增强**。
  2. 报告契约 → `child_final_text_becomes_the_agent_result_report_verbatim`：child 最终
     assistant 文本（多行、约 280 字符、刻意超过通知摘要的 200 字符截断线）逐字等于
     Finished 事件 report 与 `agent_result` JSON payload 的 report 字段（serde 解析后等值
     断言，排除扁平化/截断）。
  3. 审批冒泡 → `child_tool_approval_bubbles_to_the_root_session_and_resumes`：模块级对应
     M3-3 `child_approval_bubbles_to_root_with_origin_and_resumes`（本测试为全链路版：真
     Engine、`respond_interaction` 服务调用、会话事件流）；断言 origin
     （delegate=general-purpose-1、depth=1、非 root）、kind=Approval、批准后 child 续跑完成
     且 shell 结果进入 child 后续请求；supervisor RunFinished 与冒泡事件的两种合法时序均
     容忍。
  4. cancel 级联 → `supervisor_cancel_preempts_a_blocked_agent_result_and_cascades`：
     M3-5 `supervisor_run_cancel_cascades_to_running_instances` 的加强腿——supervisor 阻塞在
     `agent_result` 内（stream 请求数 == 2 确证）时 cancel，验证 M3-4 的 cancel 抢占在
     engine 级生效（无抢占则 RunError(Cancelled) 在 5s 兜底内永不到达）+ 运行中实例级联
     Cancelled。
  5. external/local 混合 → `local_and_external_instances_run_side_by_side`（
     `#[cfg(all(unix, feature = "external-acp"))]`）：external 生命周期单测在 M4-1 模块级
     （4 个），本测试补任务单要求的混合场景——`[external_agents.peer]`（TOML 投影）经
     fake-acp 进程 + local general-purpose 同 turn 并存，各自完成、报告取回、ACP 握手三帧
     与任务文本到达对端、终态 sweep 回收进程（session/cancel 或 SESSION_CANCELLED 轮询
     断言）；测试后 `pgrep fake-acp` 无残留。
  6. 定义加载 e2e → `agent_definitions_load_from_all_sources_and_toml_wins`：tempdir 项目
     `.mag/agents/foo.md` + 用户目录（`EnvGuard` 临时指 `XDG_CONFIG_HOME` 到 tempdir，含
     同名 foo 与独有 m5e1-scout）+ TOML `[agents.foo]` 同名覆盖；断言 `agent` 工具
     description 枚举（TOML 版 foo 描述在、user/project 版不可见、m5e1-scout 在、内置
     两类型在）与 spawn 行为（foo-1 完成、child 请求 model=model-f、system 含 TOML body
     不含 user/project body）。
- 门禁结果（全部实跑）：`cargo fmt --all -- --check` ✅；聚焦测试
  `cargo test -p mag-core engine::instances` ✅（11 passed，含 6 新测试，2.05s）；
  `cargo clippy --all-targets -- -D warnings` ✅（0 warning）；
  `cargo test --workspace` ✅（全套件 ok，0 failed）；
  `cargo doc --no-deps --workspace` ✅（mag-config 1 个既有 rustdoc warning，同 M3-1 起
  记录，与本任务无关）；`cargo check -p mag-core --no-default-features --all-targets` ✅
  （cfg 门控的场景 5 在 feature off 下编译干净）。
- flaky 复跑：6 个新测试 `--exact` 连跑 3 遍全绿（各 2.02s）；时序同步全部走
  gate/poll_until（5s 兜底，卡住即 bug），无裸 sleep 时序断言。
- 偏差：
  1. **两个 CLI e2e 的 delegation 腿不在本任务补齐，留给 M5-R**：M3-5 偏差 5 把它们指向
     M5-1，评估结论是不合适——CLI 对 `AgentInstanceStarted/Finished` 尚无渲染（`_ => {}`），
     CLI 级可断言面只有"实例生命周期事件经真实 CLI 路径不炸"；且 CLI 测试的 FakeLlmClient
     是平铺 FIFO，supervisor/child 请求交错下需先移植 scripted_routes 内容路由才能确定性
     脚本化（约 150-200 行另一 crate 测试基建），超出本任务 mag-core engine 级定位。M5-R
     若认为该 smoke 有价值，按"CLI 测试 FakeLlmClient 加内容路由 + 一个 spawn turn 的
     [finished] 断言"实施。
  2. 场景 6 的用户目录层经 `XDG_CONFIG_HOME` 进程级 env 注入（driver 直读
     `default_user_agents_dir()`，无其他注入点）：定义名用 e2e 专有名（`foo`/
     `m5e1-scout`），并发测试在同窗口建会话只会在其 `agent` 描述里多出追加行，既有断言全
     为 `contains` 型，零冲突；`EnvGuard` drop 恢复原值。
  3. 测试暴露并修正两处脚本坑（属测试自身修正，非生产行为变化）：scenario 1 的 gated
     `read_file` 与 `registry()` 既有同名 stub 冲突改裸注册表；scenario 2 的
     `agent_result` 断言取请求**最后一个** tool result（spawn 的立即返回也是
     `status:"running"` 的 JSON，取首个会张冠李戴——M3-4 测试基建已有同结论）。

### M5-2 [DONE] 文档更新：CLI.md agents 章节 + dyn-agents.md 状态

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

**完成记录**（2026-07-22）：

- 改动（仅文档，无代码）：
  - `docs/CLI.md`：
    - §4.2：新增「非绑定项 = subagent 定义来源」bullet（TOML 投影进 `AgentDefinition`
      注册表，绑定项仍是 supervisor 自身装配）；TOML 示例注释更新——`[agents.reviewer]`
      改为定义来源写法（`role`→description、`system_prompt`→定义正文、`model` 限同
      provider，删去会误导的 `provider = "local_proxy"` 行），`[external_agents.peer_acp]`
      注释改为「按实例拉起进程」；示例后新增 markdown 定义文件块
      （`~/.config/mag/agents/*.md` / `<项目>/.mag/agents/*.md`、8 字段 frontmatter 表、
      内置 `general-purpose`/`explorer`、四来源优先级 TOML > 项目 > 用户 > 内置、注册表
      装配/重建时机）。
    - §3.3 重写为 `agent`/`agent_result`/`agent_cancel` 工具表 + 异步实例模型（spawn
      立即返回 `{id,status:"running"}`、生命周期事件 `AgentInstanceStarted/Finished`、
      完成通知 pivot/空闲缓冲双通道、cancel 级联、`[tools.agent]` tier、origin 冒泡
      `{delegate=实例id, depth}`、CLI `[from <id>@depth<n>]` 前缀——与 mag-cli
      `origin_prefix` 实际格式一致）。
    - §4.4 生效时机表：原 providers/external_agents 与 agents.* 两行重排为 providers /
      绑定项 / subagent 定义三行（定义注册表随会话创建装配、随 `apply_config` 重建、只
      影响之后的 spawn）；turn 边界应用 bullet 补「并重建 subagent 定义注册表」。
    - §5 P7 行重写为动态 subagent 体系（已实现，指向 `dyn-agents.md`）；表后注更正 P2
      的 origin 路由由 mag 侧 origin 路由器实现（不再依赖 agent-lib A1）。
    - §5A 顶部加 2026-07-22 supersede 注记（委派相关条目 A1/A3/A6/A7/A9/A10 与 §D 两条
      随静态委派退役或被取代；A2/A4/A5/A8 与 §D 其余条目仍有效）；§6 external 测试
      bullet、§7 D3/D5、§0 目标表 #3/#4 与非目标 dispatcher 条目、§1.2 render 注记同步
      改写；`ask_<name>` 作为现行机制的描述全部移除（仅 §5A 历史评估正文与 supersede
      注记中作为史实保留）。
  - `docs/dyn-agents.md`：状态行改为「已实现（2026-07-22，TODO.md M1–M5-1）」；§11 新增
    wall-clock 条目（external 实例对「持续输出但永不完成」的对端无整体时长上限——120s
    为 ACP transport 每读 idle 超时，仅覆盖静默对端；依赖协作式 cancel 终止；进程池/
    常驻复用为后续优化；M4-R 结论）。
  - 根 `README.md`：grep 确认无 delegation/subagent/`ask_` 提及（仅 agent-lib 库名与
    mag-sources 的 local-agent 描述），未改动。
- 抽查（文档 ↔ 真实代码）：工具名 `agent`/`agent_result`/`agent_cancel`
  （`crates/mag-core/src/instances/spawn.rs:90-96`）；spawn 返回 `{id,status:"running"}`、
  `agent_result` 超时返回 `{"status":"running"}`（spawn.rs:10/34）；`AgentDefinition`/
  `AgentKindDef`（`crates/mag-config/src/agent_def.rs:57/80`，Local{model,tools,max_steps} /
  ExternalAcp{command,env}）；frontmatter 8 字段（agent_def.rs:24-33 KNOWN_FIELDS）；四
  来源优先级 Builtin<User<Project<Toml（agent_def.rs:37-52）；TOML 投影 role→description、
  provider 不投影（agent_def.rs:782-840）；`AgentInstanceStarted/Finished`
  （`crates/mag-service/src/service.rs:454-483`）；`InteractionOrigin{delegate,depth}` 及
  serde default=root（`crates/mag-service/src/lib.rs:689-700`）；定义表 apply 时重建、目录
  层重读、只影响之后的 spawn（`crates/mag-core/src/driver.rs:674-679,723-734`）；
  `[tools.agent]` tier（engine.rs:4848-4860）；CLI origin 前缀格式
  `[from {delegate}@depth{n}]`（`crates/mag-cli/src/lib.rs:1552-1554`）；120s 每读 idle
  超时（spawn.rs:840,878 + M4-R 记录）。文档内相对链接 `(dyn-agents.md)`/`(DESIGN.md)`/
  `(ACP.md)` 目标均存在。
- 门禁（doc 任务两项，实跑）：`cargo fmt --all -- --check` ✅；`cargo test --workspace`
  ✅（全套件 ok、0 failed、exit 0）。
- 偏差：
  1. §5A 正文未逐条改写——它是注明日期（2026-07-20）的 agent-lib 评估记录，且其中
     `ask_<name>` 等陈述对 agent-lib 库本身仍为事实（旧 API 按计划保留在库内、mag 侧
     停用）；以节首 supersede 注记消除与现行体系的矛盾。
  2. §4.4 表格由 4 行扩为 5 行（subagent 定义独立一行），§6 测试策略相应由「四行」改为
     「每行」。

### M5-R [DONE] M5 review：e2e 与文档

**内容**：review M5 diff——测试覆盖与场景清单一一对应、无 flaky（连跑 3 遍）；文档与实现
一致（抽查工具名、字段名、路径）。
**验证**：门禁序列全绿；问题修复并附完成记录。

**完成记录**（2026-07-22）：

review 范围 `f725317..HEAD`（`45df2fd` + `9d1d200` + `2185d3c`）。逐项结论：

1. **测试覆盖 ✅**：6 个 e2e 与 M5-1 场景清单一一对应（并发 → :5479、报告契约 → :5625、
   审批冒泡 → :5718、cancel 级联 → :5858、local/external 混合 → :5953、定义加载 → :6104，
   均 `crates/mag-core/src/engine.rs`）。断言深度抽查全部成立：并发实证是结构性的（两
   child 各停在共享 gated `read_file` 且此刻零 Finished，开闸后乱序完成、报告逐一回到
   对应 `agent_result`）；报告逐字（多行 ~280 字符，Finished 事件与 `agent_result` JSON
   payload 双路等值断言，取请求最后一个 tool result 避开 spawn 的 running JSON）；origin
   三项断言（delegate=general-purpose-1、depth=1、非 root）+ 批准后 shell 结果确入 child
   后续请求；cancel 级联（supervisor 阻塞在 `agent_result` 内 stream 请求数 == 2 确证，
   RunError(Cancelled) + 实例 Cancelled 双断言）；进程回收（ACP 握手三帧 + 任务文本到达
   对端 + session/cancel 或 SESSION_CANCELLED 轮询断言）；定义优先级（description 枚举
   四来源、TOML 版 model/body 生效、user/project 版 body 不可见）。
2. **无 flaky ✅**：本 review 实跑 `cargo test -p mag-core --lib engine::instances` 连跑
   3 遍，11 passed × 3（各约 2.02s）；时序同步全走 gate/`poll_until`（5s 兜底）/`collect_
   events_until`，无裸 sleep 时序断言；跑后 `pgrep fake-acp` 无残留。
3. **文档与实现一致 ✅**（M5-2 已抽查，本 review 复核关键点）：工具名
   `agent`/`agent_result`/`agent_cancel`；frontmatter 字段表 8 字段与
   `mag-config/src/agent_def.rs:24-33` KNOWN_FIELDS 逐一相符；四来源优先级
   TOML>项目>用户>内置与 `DefinitionSource` 派生 `Ord`（agent_def.rs:37-52）一致；CLI
   origin 前缀 `[from <id>@depth<n>]` 与 `mag-cli/src/lib.rs:1552-1557` `origin_prefix`
   实际格式一致；§5A supersede 注记抽查 A2（facade reconfigure 仍有效）核实——
   `driver.rs:751` 实调 `self.agent.reconfigure(request)`（apply_config turn 边界路径），
   注记准确。
4. **CLI e2e delegation 腿（M5-1 偏差 1）处置：不补，结论记录于此**。理由：
   (a) 可断言面只有渲染——mag-cli 对 `AgentInstanceStarted/Finished` 走 `_ => {}`
   （设计 §11 明确实例 UI 展示后补），一个 spawn turn 在 CLI 级只能断言
   `[tool …] name=agent` 通用 trace 与 `[finished]`，而通用 tool trace 渲染已被既有
   `ask_user` 腿覆盖；(b) 机制实质（spawn/drive/result/cancel/通知）已由 M5-1 的 6 个
   engine 级 e2e + M3/M4 模块测试覆盖，CLI 路径无实例专属装配（`Engine::with_config_
   service` 共享构造，实例上下文在 driver 内），M3-5 已断言三工具在 CLI 驱动面上架；
   (c) 成本高——CLI 测试的 FakeLlmClient 是平铺 FIFO，supervisor stream 请求与 child
   chat 请求交错下必须先移植 scripted_routes 内容路由（约 150-200 行另一 crate 测试
   基建，且与 mag-core test_support 有漂移风险）。判断 smoke 价值不抵成本；待实例
   UI 展示落地时（§11）随渲染面一并补 CLI e2e。
5. **`9d1d200` 修复完整 ✅**：M5-2 标题已恢复为独立节（TODO.md:1509，M5-2 完成时标
   [DONE]），其「目标/验证」正文与完成记录无残缺，M5-1 完成记录末尾无吞并痕迹。
6. **门禁（全部实跑）**：`cargo fmt --all -- --check` ✅；`cargo clippy --all-targets
   -- -D warnings` ✅；`cargo test --workspace` ✅（32 目标全 ok、0 failed、exit 0）；
   `cargo doc --no-deps --workspace` ✅（mag-config 1 个既有 rustdoc warning，同 M3-1
   起记录）。review 无代码改动。

---

## F-R [DONE] 全计划 review

**内容**：
- 对照 `docs/dyn-agents.md` 全量核对：D1–D8 逐条落地情况；§8 退役清单全库 grep 复核
  （`ask_`、`subagent(`、`prune_unregistered`、`DelegateBinding`）；§11 已知限制清单更新
  （新增实现中发现的限制）。
- 全量门禁：fmt → clippy → `cargo test --workspace`（含 feature 组合）→ doc；agent-lib
  仓库同序列。
- 抽查 rustdoc 覆盖（新 pub API 全部有文档）；ts-rs 生成物无漂移；secret 纪律。
- 修复全部发现的问题，必要时在依赖位置插最小修复任务。

**验证**：门禁全绿；`docs/dyn-agents.md` 状态与偏差记录最终一致；完成记录归档。

**完成记录**（2026-07-22）：

review 范围：mag `git diff 3e2a56f..HEAD`（24 文件，+9554/-1858，14 commit）+ agent-lib
`8db05c7`（唯一 commit）。逐项核对结论：

1. **D1–D8 逐条落地 ✅**（代码证据）：
   - D1 agent 自主创建：`agent` 是普通 facade 工具、由模型 tool call 触发
     （`instances/spawn.rs:323` `agent_tool`），无用户命令路径。
   - D2 markdown 定义：`parse_agent_md`（`mag-config/src/agent_def.rs:205`，YAML
     frontmatter + 正文即 prompt，Claude Code 格式）；内置只读 `explorer`
     （`builtin()` :648，`tools=[read_file,list_dir,grep]`，Codex 参照）。
   - D3 实例化多并发：`register` + `spawn_local` 后立即返回
     `{"id","status":"running"}`（spawn.rs:402-415）；实例即用即抛、不进 restore。
   - D4 分层 prompt + 兜底：`SUBAGENT_SKELETON`（spawn.rs:120）+
     `layered_system_prompt`（:621）骨架+正文两层；`DEFAULT_AGENT_TYPE =
     "general-purpose"`（:103），builtin 定义永存。
   - D5 单一 `agent` 工具：`AGENT_TOOL_NAME` + `type` 参数（:90、:331-348），描述动态
     枚举全部类型；无 per-type 合成工具（§8 grep 见检查单 2）。
   - D6 异步 spawn + 拉/推双通道：拉 = `agent_result` 阻塞（:494-507）；推 = run 中
     pivot `PivotSource::Host { label: "agent:<id>" }`（driver.rs:1045）+ run 空闲
     `[agent 实例通知]` 前缀缓冲（driver.rs:621-623、:965）。
   - D7 external 统一：同一 `agent` 工具按 `definition.kind` 分派（spawn.rs:712-717）；
     external 经 `run_external_once` 按实例拉起、终态回收（:901），与 local 同一
     注册表/事件/生命周期/审批冒泡。
   - D8 `agent_result` 阻塞+超时：`tokio::time::timeout(timeout_secs, await_terminal)`
     缺省 600s（:100、:494），超时返回 `running` 不置失败（:507），cancel 可抢占。
2. **§8 退役清单 grep 复核 ✅**（mag workspace，agent-lib 库内旧 API 按计划保留不查）：
   `subagent(`/`DelegateBinding`/`delegate_worker`/`apply_delegate_start_tiers`/
   `delegate_start_tool_name`/`external_acp_delegate`/`TrackedExternalSessionHandler`/
   `cleanup_external_sessions` 在 `crates/` 均 0 命中；`ask_` 命中全为活机制
   （`ask_user`/`ask_tool`/`ApprovalPolicyKind::Ask`）、`history.rs:184` 只读旧数据
   投影、退役注释与 legacy 测试夹具、以及"surface 无 `ask_*`"否定断言；
   `prune_unregistered` 仅 2 命中（driver.rs:302 rustdoc + :368 restore 调用）=
   M3-5 偏差 1 的唯一残留。偏差论证复核成立：agent-lib `snapshot.rs:793-837` rustdoc
   实证不 re-register 且不 prune 时持久化 delegate 以 `ApprovalPolicy::default`
   （auto_allow）复活且 `ask_<name>` 留在工具面；零 re-registration 下 prune 是纯单向
   清扫（声明随 recipe 一并从 current/initial tool set 移除）。§8 七行去向逐条与实现
   一致（⑦ `Delegation::single_tool` 复用的实现形态差异 M3-R 已记录在案）。
3. **§11 已知限制 ✅（补 1 条）**：既有 7 条（M4-R model 同 provider 限制、external
   启动开销、external 无 wall-clock 上限、实例 UI 后补、通知通道窗口限制、嵌套独立
   root ctx、per-type tier follow-up）逐条与实现核对一致，无"记录了但已解决"条目。
   发现 1 条"实现了但没记录"：M3-5 偏差 4（supervisor 面 `agent` 工具描述 build 时
   烘焙，apply_config 后仅随 `ReplaceToolSet` 刷新——driver.rs:833-839 实证）此前只在
   完成记录里，已正式补入 §11（修复 2）。
4. **全量门禁（全部实跑）**：
   - mag：`cargo fmt --all -- --check` ✅；`cargo clippy --all-targets -- -D warnings`
     ✅（0 warning）；`cargo test --workspace` ✅（exit 0，全套件 ok、0 failed）；
     `cargo check -p mag-core --no-default-features --all-targets` ✅；
     `cargo doc --no-deps --workspace` ✅（mag-config 1 个既有 rustdoc warning，同
     M3-1 起记录）。
   - agent-lib：`cargo fmt -- --check` ✅；`cargo clippy --all-targets -- -D warnings`
     默认与 `--features external-acp` 两组合 ✅；`cargo test` 默认与
     `--features external-acp` 两组合 ✅。
   - ts-rs：`cargo test -p mag-service --features ts-export export_ts` 后
     `git diff --exit-code -- ui/packages/protocol/src` ✅ 无漂移。
5. **rustdoc 覆盖 ✅**：本计划新 pub API 全部有文档——agent_def 全模块
   （`#![warn(missing_docs)]` + clippy `-D warnings` 机械保证）；`run_external_once`
   一次性语义/三终态回收保证/Errors 段齐备（agent-lib delegate.rs:600-633）；wire 新
   变体与 `AgentInstanceStatusWire` 类型级+字段级文档（mag-service lib.rs:307-323、
   :642-648）。
6. **secret 纪律 ✅**：`git diff 3e2a56f..HEAD` 全量按常见 secret 形态（sk-/bearer/
   password/api_key/AKIA/ghp_/xox 等）grep，无解析后 secret 值（仅 "task-frame" 一类
   误命中）；配置与测试夹具中 secret 仅以 `{env=...}`/`{keyring=...}` 引用形态出现
   （agent_def.rs:1282/1287）。
7. **计划外改动审查 ✅**：`--stat` 24 文件全部落在任务单 scope（mag-config 定义模型、
   mag-core 实例运行时与接线、mag-service wire、测试、文档、ts-rs 生成物、TODO 与依赖
   清单）；`ui/` 下用户 4 个未提交文件（apps/web/package.json、pnpm-lock.yaml、
   postcss.config.js、tailwind.config.ts）未被任何 commit 触及（本任务提交也不含）；
   agent-lib 仅 `8db05c7` 一个 commit 且工作树干净。
8. **PLAN.md 完成定义逐条 ✅**：17 个任务（含 M1-R..M5-R 各 review）全 `[DONE]` 且有
   完成记录（本任务收尾后 18/18）；两仓库门禁序列实跑全绿（检查单 4）；测试全离线
   （FakeLlmClient scripted_routes / fake-acp.sh / tempdir），实例相关套件均为秒级；
   退役面 grep 干净（检查单 2）；公开 API rustdoc 全覆盖（检查单 5）；wire 变体 ts-rs
   无漂移（检查单 4）；本记录即 F-R，发现的问题已修复（下）。

**修复内容**（随本 review 提交，均最小改动）：
1. `crates/mag-core/src/instances/spawn.rs` `split_external_command` rustdoc 更正：原文
   称"TOML `[external_agents]` 投影不校验空 command"，与 M4-R 已更正的结论相悖
   （`agent_def.rs:850` 空 command skip+warn、`io.rs:157` 加载期拒绝空 argv）——改为
   如实描述两条真实配置路径均已校验、`None` 臂为不可达的纵深防御。
2. `docs/dyn-agents.md` §11 补 supervisor 面 `agent` 描述烘焙滞后条目（M3-5 偏差 4 的
   正式收录，见检查单 3）。

**遗留事项**：全部收录于 §11（7+1 条），本 review 无新增；M5-R defer 的 CLI 实例渲染
e2e 维持原结论，随实例 UI 里程碑一并补。

---

## F-R 后设计修订（2026-07-22）：child 工具面归属 agent 种类

**主题**：动态 subagent 的 child 工具面语义变更。M3-R 引入的"交集"语义（`def.tools ∩
supervisor 绑定面`）被推翻：工具面是 **agent 种类的属性**，supervisor 绑定 allowlist 彻底退出
child 面计算。

**语义变更**：

- 删除 `SharedSpawnState.surface`（`crates/mag-core/src/instances/spawn.rs`）及 driver 侧的
  播种（`root_spawn_context` 的 `binding.tools()`）与 `apply_config` 镜像更新。
- child 面解析顺序：定义 `tools` 显式给出 → 恰好那份列表；缺省 → session 缺省 toolset（新增
  `[session].default_subagent_tools`，纯名字列表，不做 ResolvedTool 投影，校验推迟到面构建）；
  再缺省 → session 可用工具全量。三种结果都过可用性校验：注册表未知名与
  `[tools.x] enabled = false` 禁用名 drop + warn（每会话一次）；`enabled = false` 与
  `approval = "deny"` 是 session 级硬边界——前者在面构建时排除，后者走审批层（现状不动）。
- `agent` / `agent_result` / `agent_cancel` 三工具作为运行时能力照旧默认附加给每个 child。
- TOML `[agents.<name>]` 非绑定项投影不变（其 `tools` → `def.tools`）；绑定项（supervisor）的
  `tools` 语义不变，只约束 supervisor 自己。
- `default_subagent_tools` 随会话创建播种、随 `apply_config` 重建（与定义注册表同语义），只
  影响之后的 spawn。

**理由**：supervisor 面是主 agent 自身的行为塑造（绑定项对自身能力的裁剪），与 child 无关；
交集语义会静默阉割项目携带的定义（定义声明的工具因 supervisor 未配而悄悄消失），并堵死
coordinator 拓扑（面被刻意收窄的 supervisor 无法派出需要更多工具的 child）。

**涉及 commit**：`62fb5d3`

**测试**：M3-R 两个"supervisor 收窄约束 child"测试语义反转（driver.rs
`child_surface_ignores_the_supervisor_bound_tool_list` +
`default_subagent_tools_seed_at_build_and_follow_config_apply`）；M3-3
`child_inherits_full_surface_when_tools_unset` → spawn.rs
`child_surface_defaults_to_toolset_then_full_registry`（缺省取 toolset / 未配置取全量）；新增
`[session].default_subagent_tools` 配置链路（mag-config
`session_default_subagent_tools_resolves_and_round_trips` + 上述 driver 测试）、显式 def.tools
优先于缺省（`explicit_definition_tools_win_over_the_default_toolset`）、未知名/enabled=false
drop + warn（`child_surface_skips_unknown_default_toolset_names`、
`child_surface_drops_tools_disabled_at_session_level`、
`unavailable_surface_names_warn_once_per_session`）；既有测试全绿。

**文档**：`docs/dyn-agents.md` §3.1 字段表 `tools` 行、§3.3 `general-purpose` 条目、§7 工具面
条目重写；`docs/CLI.md` §4.2 TOML 示例与 frontmatter 字段表 `tools` 行、[session] 说明、§4.4
生效时机表同步。
