# 执行计划 — W5-1 [TODO] 端到端加固 + Storybook 补全

## 任务定义（TODO.md W5-1）
1. 协议级 e2e 补场景（`crates/mag/tests/web_e2e.rs`，真实 Engine + 回环 server）：
   - SSE 断连重连全量对齐（杀连接→重连→history 对齐无重复无丢失）
   - 多连接（双标签模拟）各自收全量事件互不干扰
   - 长 run 中 heartbeat 保活
2. 前端 e2e：可选 Playwright；若成本高则 scripted store 级覆盖 + 手动联调说明（完成记录注明取舍）。
   路径：建会话→对话→审批→委派→pivot→cancel→config→sources。
3. Storybook 视觉态补全：错误态/空态/长会话性能基线（大列表虚拟化视需要，不过度设计）。
4. 真实浏览器联调 `#[ignore]` 脚本与说明。
5. 裁决 §5.4「最小文件附加（路径文本）」：要么实现最小路径文本附件 UI，要么修订 `docs/WEB.md`
   §5.4 移除该承诺；不允许悬空。前提调查：wire `send_message` 是否有 attachments 字段、引擎是否消费。

## 执行步骤
1. [ ] 探索现状：
   - `crates/mag/tests/web_e2e.rs` 现有 e2e 结构与 SSE 客户端 helper
   - `crates/mag-web` SSE heartbeat 间隔/测试方式（heartbeat ~15s，e2e 怎么等？）
   - mag-service wire `send_message`/UserInput 是否含 attachments；mag-core 是否消费
   - app-web 现有壳级测试覆盖 vs 全路径清单（委派覆盖？）
   - Storybook 现有 stories 清单（错误态/空态缺口）
   - `ui/README.md` 手动 smoke 说明现状
2. [ ] Rust e2e 补三场景（重连对齐 / 多连接 / 长 run heartbeat）。
3. [ ] 前端：补齐全路径 scripted 覆盖缺口（如有）；Storybook 补错误/空态。
4. [ ] §5.4 附件裁决并闭环（实现 UI 或修订 WEB.md）。
5. [ ] 真实浏览器联调说明（`#[ignore]` 或手动脚本）。
6. [ ] 验证序列：cargo fmt → clippy -D warnings → 聚焦测试 → cargo test --workspace →
   cargo doc；pnpm format/lint/-r test/-r build/build-storybook。
7. [ ] TODO.md：W5-1 标记 [DONE] + 完成记录；提交并停止。

## 进度日志
- 2026-07-21：确认首个未完成任务为 W5-1；git 工作区干净，HEAD=e2afc5c（W4-R）。
- 调查结论：wire `UserInput.attachments` 存在但引擎丢弃（`engine.rs:343` 只取 text，history
  attachments 恒空，CLI 同样未消费）→ §5.4 裁决为**修订 docs/WEB.md 移除首版附件承诺**（做 UI
  等于发死功能）；前端壳级测试已覆盖全路径八环；Playwright 不引入（成本高+违离线纪律），
  取舍记完成记录；heartbeat 通过给 `ServeOptions` 加 `heartbeat_interval: Option<Duration>`
  公开旋钮（默认 None→15s）供 e2e 加速。
- 已完成实现：
  - mag-web：`ServeOptions.heartbeat_interval`；main.rs 与三处测试字面量同步。
  - web_e2e.rs 新增 3 测试：`reconnects_and_aligns_history_without_loss`（断连→run 完成→
    history 轮询对齐 exactly-once→重连后新 run 事件正常）、
    `broadcasts_full_event_stream_to_multiple_connections`（双连接事件序列完全相等）、
    `keeps_long_runs_alive_with_heartbeat_comments`（200ms heartbeat + stall run + cancel
    收尾）。修复一处竞态：history 轮询先于新 SSE 订阅，避免吃到第一个 run 的迟到终态事件。
  - cli.rs 新增 bin 级 web smoke（真实二进制、离线、非 ignored）：401 无 token/错 token、
    200 对 token、SPA 占位页免 auth。ui/README.md Manual Web Smoke 扩为 7 步并引用该 smoke。
  - 前端（子代理 + 本人）：ThreadView `LongThread` story（300 条渲染基线，不虚拟化）；
    App.test.tsx 壳级重连对齐测试；SourcesView 错误行从 app 层下沉为组件 `error` prop
    （补 `ProbeFailed` story + 组件测试），SourcesPage 同步。
  - docs/WEB.md §5.4 附件条目修订。
- 聚焦测试全绿：web_e2e 4/4（0.42s）、cli web_binary（2s）、pnpm 各包（子代理门禁全绿）。
- 2026-07-21 全量验证序列全绿：cargo fmt --check / clippy -D warnings / cargo test --workspace
  （唯一 ignored 为既有 ACP 骨架）/ cargo doc；pnpm format / lint / -r test（client 36 + ui 30 +
  app-web 12）/ -r build / build-storybook。
- TODO.md：W5-1 标记 [DONE] 并写完成记录（含 Playwright 取舍与 §5.4 附件裁决证据）。提交后停止。

