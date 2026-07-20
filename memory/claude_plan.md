# 当前任务计划（2026-07-21）

## 任务：W4-2 pivot/cancel 完备 + run 状态

来源：TODO.md 首个未完成任务（W4-1 [DONE]，W4-2 为下一个）。

### 要求（TODO.md 原文要点）

- `pivot_queued/applied/dropped` 渲染为轻量系统消息
- `run_error` 按 `RunErrorKind` 分类渲染（cancelled/budget/loop 非错误色）
- composer 状态机完备（idle/running/pending 交互三态文案与按钮）
- 多会话并行时右栏全局 running 列表

### 验证

- vitest + Storybook 新增态
- `pnpm -r test` / `pnpm -r build` 绿
- 默认验证序列（fmt/clippy；Rust 若无改动则复用上次全量结果）

### 执行步骤

1. 探查现状：
   - wire 类型：`pivot_queued/applied/dropped` 事件形状、`RunErrorKind` 变体
     （ui/packages/protocol/src/generated/）。
   - `@mag/client` store：pivot notice 现状（W3-2 已有 pivot 提示/去重）、run 状态机、
     run_error 处理、多会话 running 派生。
   - `@mag/ui`：ThreadView 中 pivot/run error 投影现状（W3-3 已有基础投影）、
     Composer 状态机现状（idle/running/pending/disabled）。
   - app-web：右栏现状（W3-4 已有 run 摘要/全局 running 会话摘要，W4-1 加了
     Delegates 列表与折叠）。
2. 缺口分析后按最小改动补齐：
   - store：确保 pivot 三态（queued/applied/dropped）都有 notice；run_error 保留 kind；
     右栏全局 running 列表 selector（若缺）。
   - ui：pivot 系统消息样式统一为轻量系统消息；run_error 按 kind 分类着色
     （cancelled/budget/loop 非 destructive）；Composer 三态文案/按钮核对。
   - app-web：右栏全局 running 列表（多会话）。
3. 测试：vitest fixture（pivot 三态事件流、各 RunErrorKind、多会话 running）；
   Storybook 新增视觉态。
4. 验证序列：pnpm format:write → pnpm format → pnpm lint → pnpm -r test →
   pnpm -r build → build-storybook；cargo fmt --check + clippy（Rust 未改则全量测试
   复用上次结果并注明）。
5. TODO.md 标记 W4-2 [DONE] + 完成记录，提交 git。

### 现状探查结论（2026-07-21）

- pivot 三态：store（`PivotNotice` + 去重）与 ThreadView `SystemNotice` 已实现；
  client 测试只断言 queued/applied，缺 dropped/reason/去重断言。
- run_error：store 保留 `RunErrorKind`，ThreadView `runErrorTone`（other→danger，
  cancelled/budget/loop→muted）已实现；缺 vitest 断言与 loop_limit_exceeded story。
- Composer：idle/running 文案 + cancel 按钮 + pending 徽标已实现，story 覆盖四态；
  缺 Composer 组件测试。
- 右栏全局 running 列表存在但为静态 div——§5.3 要求「跨会话跳转」，**缺口**：
  需可点击跳转对应会话。

### 缺口实施

1. app-web：RightRail running sessions 改按钮 + `onSelectSession` 跳转（复用
   openSession 路径）；壳级测试（两会话 running，点击跳转）。
2. client 测试：pivot dropped+reason+连续重复去重；run_error → run state error +
   kind 保留 + thread item。
3. ui 新增 `ThreadView.test.tsx`（pivot 三态文案、run_error tone 分类）与
   `Composer.test.tsx`（idle/running/pending 三态文案与按钮、cancel 回调）。
4. Storybook：PivotAndErrors 补 `loop_limit_exceeded`，覆盖全部四个 RunErrorKind。

### 进展日志

- [x] 读取 TODO.md，确认首个未完成任务为 W4-2。
- [x] 现状探查
- [x] 缺口补齐：app-web 右栏 running 列表改可点按钮跨会话跳转（App.tsx + 壳级测试）
- [x] 测试与 Storybook：client pivot 全生命周期+run_error kind 测试；ui 新增
  ThreadView/Composer 测试；PivotAndErrors story 补 loop_limit_exceeded
- [x] 验证序列：pnpm format:write/format/lint、pnpm -r test（client 17/ui 18/app-web 5 全绿）、
  pnpm -r build、build-storybook、cargo fmt --check、clippy 全绿；Rust 零改动，workspace
  测试/doc 复用 W3-4（0fe4985）结果按规则跳过
- [x] TODO.md 完成记录 + commit（注意：编辑时误删 W4-3 标题，已当场恢复并 grep 核实）

## 任务完成（2026-07-21）

W4-2 已标记 [DONE] 并提交。唯一实质缺口是右栏全局 running 列表缺跨会话跳转（§5.3），
已修复；其余三项（pivot 系统消息、run_error 分类渲染、composer 三态）W3 已落地，本任务
补齐了测试与 Storybook 覆盖。下一任务：W4-3 ConfigEditor（文本形态）+ Sources 页。
