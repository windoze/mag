# 执行计划 — W4-R [TODO] W4 review

## 任务定义（TODO.md W4-R）
- 对照 `docs/WEB.md` §5 全节逐项核对 CLI 能力在 web 的呈现覆盖（§0 目标清单）。
- 检查 origin 归因、pivot 回落、D2 语义文案。
- 发现问题直接修复并补测试。
- 验证：`pnpm -r test`/`pnpm -r build` 绿 + 默认验证序列；完成记录逐项列出 §0 清单结论。

## 范围
- W4-1 (`b2b00ba`) delegation 可视化：client `selectDelegationGroups`、ui `DelegationCard`/`DelegateThreadPanel`、app-web 右栏 drill-down + 折叠。
- W4-2 (`6e52ab5`) pivot/cancel 完备 + run 状态：pivot 三态系统消息、RunErrorKind 分类渲染、composer 三态、全局 running 列表跨会话跳转。
- W4-3 (`6603ffc`) ConfigEditor（文本形态）+ Sources 页：TOML 转换、Save/Reload/Apply、secret 引用纪律、D2 生效时机文案、Sources 表格 + Probe。

## 执行步骤
1. [x] 读 `docs/WEB.md` §0 目标清单、§5 全节（5.1–5.5）、D2/D5/D6 决策。
2. [ ] 派 review 子代理并行分块核查（对照 spec 逐项，找 bug 级/偏差级问题）：
   - 块 A：§5.2/§5.3 delegation + origin 归因（W4-1 diff + store 分组 selector + 徽标）。
   - 块 B：§5.2/§5.4 pivot 回落（409→send_message）、run 状态条、composer 三态、全局 running 列表（W4-2）。
   - 块 C：§5.5 ConfigEditor（TOML 转换正确性、Save/Reload/Apply、secret 引用不物化、D2 文案）+ Sources 页（W4-3）。
   - 块 D：§0 目标清单逐项覆盖核查（§5 全节 vs 实现）。
3. [ ] 汇总发现；bug 级问题当场修复并补回归测试。
4. [ ] 验证序列：
   - `pnpm format` / `pnpm lint` / `pnpm -r test` / `pnpm -r build` / `build-storybook`
   - `cargo fmt --all -- --check` / `cargo clippy --all-targets -- -D warnings`
   - 若 Rust 源码零改动（git 实证），`cargo test --workspace`/`cargo doc` 复用最近全绿结果并注明；否则重跑。
5. [ ] TODO.md：W4-R 标题加 `[DONE]`，完成记录逐项列出 §0 清单结论 + 发现/修复清单。
6. [ ] 提交（含 memory/claude_plan.md 更新），停止。

## 已知记录在案偏差（review 时复核，不阻塞）
- W3-R：§5.2「批准/拒绝（+ always）」与 wire 不符（wire 无 always 变体，mag-acp 已拍板）→ 随 F-R 复核文档。
- W4-1：wire 工具事件无 origin 字段，delegate 内部工具卡无法在 root 事件流归因（D5 origin 仅定义在交互上）→ ToolCallCard origin 徽标渲染已就绪。
- W4-2 之前各任务完成记录：Rust 源码自 W3-4 (`0fe4985`) 起零改动，cargo 全量测试复用其全绿结果。

## 进度日志
- 2026-07-20/21：读取 TODO.md 确认首个未完成任务为 W4-R；已读 docs/WEB.md §0–§10 全文。
- 4 个 review 子代理已完成分块核查，汇总发现：
  - **Bug 级（修）**：①replaceHistory 抹掉 delegation 消息（子线程在 resume/reconnect 后变空，client 侧保留消息如同 pendingInteractions）；②pivot 成功无本地用户回声（D6 与 CLI 不一致）；③syncSessionInfos 对 awaiting_interaction 不做 idle 校正；④probeSources/local_agents_probed 用局部列表整体替换导致 provider 行消失（改 merge）；⑤stripUnsupported 丢空数组/空表改变 `tools=[]` 语义；⑥tomlToConfigDto 静默丢弃未知 key（加 client 白名单校验）。
  - **小修**：pivot 回落条件加 409 校验 + 负分支测试；DelegateThreadPanel 去掉逐消息 origin 徽标（spec 只要求交互/工具卡）；delegationGroups 渲染期直读 mutable store（改 snapshot 派生）；delegation "latest status" 按更新序；ConfigPage Reload 前 dirty 确认；SourcesPage role="alert"；Composer Cancel 无回调不渲染；composer draft 按会话隔离；sidebar 徽标文案 "awaiting-interaction"；ToolCallCard started 态 spinner；config.ts params 类型漂移注释；选中清理 effect 补测试。
  - **记录在案（不修）**：wire 工具事件无 origin；wire 无 always；delegation 消息不入 history（跨页面加载丢失，wire 限制）；pending 交互在 run 终态后残留（wire 无 resolved 事件）；xl 以下右栏不可达（§0 非目标移动端）；删除确认为 hover 按钮+confirm 两步（满足二次确认意图）；§5.4 最小附件 UI 未做 → 加入 W5-1 范围显式跟踪。
- 下一步：读相关源码，逐项修复 + 补测试。
- 2026-07-21 修复完成：config.ts（空数组/表保留、白名单校验、params 注释、TDZ 修正）、
  store.ts（delegation 消息跨 replace 保留、pivot 本地回声、awaiting idle 校正、probe merge、
  delegationGroups 入 SessionView snapshot、updatedSeq）、App.tsx（409+kind 双判、per-session
  草稿、groups 从 snapshot 派生、latest 按更新序）、ui 四组件（逐消息徽标移除/Cancel 条件渲染/
  started spinner/awaiting-interaction 文案）、ConfigPage dirty confirm、SourcesPage role=alert、
  ConfigEditor stories 虚构 key 修正。
- 测试：client 29→36、ui 28→29、app-web 7→11，全绿；`pnpm -r test`/`pnpm -r build`/
  `build-storybook`/fmt/lint 全过；cargo fmt --check + clippy -D warnings 过；Rust 零改动，
  workspace 测试/doc 复用 W3-4 (0fe4985) 结果。
- TODO.md：W4-R 标记 [DONE] 并写完成记录（§0 逐项 + 修复清单 12 项 + 记录在案偏差 7 项）；
  W5-1 新增 §5.4 附件裁决子项。提交后停止。
