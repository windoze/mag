# 当前任务计划（2026-07-21）

## 任务：W4-1 delegation 可视化

来源：TODO.md 首个未完成任务（W3 全部 [DONE]，W4-1 为下一个）。

### 要求（TODO.md 原文要点）

- `DelegationCard`（内联：delegate 名/状态/usage）
- 右栏 delegate 子线程视图（origin.delegate 匹配的事件汇聚；工具卡/交互卡带
  `[from <delegate>@depth<n>]` 徽标）
- 右栏可折叠
- `@mag/client` store 补 delegation 分组 selector

### 验证

- vitest fixture（含两级 delegate 事件流）断言分组与徽标
- Storybook 新增视觉态
- `pnpm -r test` / `pnpm -r build` 绿
- 默认验证序列（fmt/clippy；Rust 若无改动则复用上次全量结果）

### 执行步骤

1. 探查现状：
   - `ui/packages/client/src/`：store 中 delegation 现状（W3-2 已有 delegation 分组、
     `delegation run/delegate key`）、selectors、origin 字段来源（wire Event 的
     origin 字段形态）。
   - `ui/packages/ui/src/`：ThreadView 中 delegation 卡现状（W3-3/W3-R 已有基础
     delegation 卡 + 无回调时静态 div）、InteractionCard origin 徽标、ToolCallCard。
   - `ui/apps/web/src/`：右栏现状（W3-4 已有 run 摘要 + delegation drill-down 插槽）。
2. `@mag/client`：补 delegation 分组 selector（按 run/delegate 聚合其来源事件：
   消息/工具/交互，支持两级 delegate 嵌套）。
3. `@mag/ui`：
   - `DelegationCard` 补 usage 展示（若缺）。
   - ToolCallCard/InteractionCard/消息项支持 `[from <delegate>@depth<n>]` origin 徽标
     （InteractionCard 已有 origin 徽标，核对 ToolCallCard 与消息项）。
   - 子线程视图组件（纯 props：事件/消息列表 + 标题 + 折叠）。
4. app-web：右栏接入 delegation drill-down（点 delegation 卡 → 右栏显示该 delegate
   子线程），右栏可折叠。
5. 测试：vitest 两级 delegate fixture 断言分组与徽标；Storybook 新增视觉态。
6. 验证序列：pnpm format:write → pnpm format → pnpm lint → pnpm -r test →
   pnpm -r build → build-storybook；cargo fmt --check + clippy（Rust 未改则全量测试
   复用上次结果并注明）。
7. TODO.md 标记 W4-1 [DONE] + 完成记录，提交 git。

### 现状探查结论（2026-07-21）

- wire 上 delegate 活动的可见面：`delegation_message`（文本）、`interaction_requested`
  带 `origin{delegate,depth}`、`delegation_*` 生命周期 trace（无 depth 字段）。工具事件
  无 origin——工具卡徽标渲染已就绪（ToolCallCard.origin prop），但 wire 不产生工具归因，
  完成记录中记为偏差（D5 origin 本就只定义在交互上，docs/CLI.md §3.3）。
- 现状：`@mag/ui` ThreadView 内联 delegation 卡（含 usage）、InteractionCard/ToolCallCard
  origin 徽标均已存在；app-web 右栏是「Delegate drill-down lands in W4」占位；
  ThreadView.onOpenDelegation 未接线；`@mag/client` 无分组 selector。

### 实施设计

- `@mag/client`：`InteractionView` 加 `seq`；`DelegationView.messages` 改为
  `StoredDelegationMessage{seq,message}`；新增 `selectDelegationGroups(sessionId)` 按
  delegate 名分组（delegation trace + 其 messages + origin.delegate 匹配的交互，按 seq
  交错排序；depth 取匹配交互的最大 origin.depth；无 trace 的 origin delegate 合成组）。
- `@mag/ui`：抽出公开 `DelegationCard`（ThreadView 复用）；新增 `DelegateThreadPanel`
  （右栏子线程：头 delegate/status/usage/close，items=消息+交互卡带徽标）；Storybook 两
  组件新故事；`DelegateThreadPanel` 渲染/回调测试。
- app-web：右栏 Delegates 列表 + drill-down 面板接线（thread 卡 onOpen → 选中）；右栏
  可折叠（header 切换按钮）；壳级测试。

### 进展日志

- [x] 读取 TODO.md，确认首个未完成任务为 W4-1。
- [x] 现状探查
- [x] client selector（`selectDelegationGroups`，seq 交错排序，16 测试绿）
- [x] ui 组件 + Storybook（`DelegationCard` 抽出 + `DelegateThreadPanel`，12 测试绿，storybook 构建绿）
- [x] app-web 右栏（Delegates 列表 + drill-down + 折叠切换，4 测试绿）
- [x] 测试与验证：pnpm format/lint/test/build、build-storybook、cargo fmt --check、clippy 全绿；
  Rust 零改动，workspace 测试/doc 复用 W3-4 结果按规则跳过
- [x] TODO.md 完成记录 + commit

## 任务完成（2026-07-21）

W4-1 已标记 [DONE] 并提交。记录在案的偏差：wire 工具事件无 origin，delegate 内部工具调用无法
归因（D5 origin 只覆盖交互），ToolCallCard 徽标渲染已就绪待 wire 扩展。下一任务：W4-2
pivot/cancel 完备 + run 状态。
