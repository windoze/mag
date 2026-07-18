# Claude 执行计划 — C5-1 端到端离线主干集成测试

## 任务
`TODO.md` 首个未完成任务 = **C5-1**。写一个真正的集成测试（`crates/mag-core/tests/e2e_offline.rs`），
**只用 mag-core 公有 API、经 `Arc<dyn MagService>` 驱动**，把 C1–C4 串成一条离线全链路。

## 约束（来自 TODO 通用规则 + C5-1）
- 全程经 `Arc<dyn MagService>`，不碰 Engine 内部（tests/ 是独立 crate，本就只能用公有 API）。
- 离线：自建 fake `LlmClient`（脚本化 stream 事件）+ 脚本化 stub 工具 + 临时文件 SQLite + 内存审批 channel。
- 无网络/凭据/CLI；每个用例 1 分钟内完成。
- 聚焦命令：`cargo test -p mag-core --test e2e_offline`。
- 完整验证序列 1–5：fmt --check → 聚焦测试 → clippy -D warnings → test --workspace → doc。

## 设计
tests/ 是独立 crate，无法用 `pub(crate)` 的 `test_support`，因此在测试文件内自建最小 fixtures：
- `FakeLlm`：impl `agent_lib::client::LlmClient`；FIFO 脚本队列 + 按 `request.model` 路由的脚本队列
  （并发用例按 model 确定性取脚本，与拉取顺序无关）；记录 stream_requests 供恢复上下文断言。
- stream 事件构造器：`text_stream` / `stalling_text_stream` / `tool_use_stream`（复刻 test_support 形状）。
- `StubTool`：impl `mag_tools::ToolPlugin`，忽略入参返回固定文本；`read_file`(auto)、`shell`(gated)。
- `TempDb`：临时文件 SQLite，Drop 删文件。

### 用例 1：full_offline_backbone_through_service（主干全链路）
单会话 s，engine1 = Engine::with_persistence(fake1, {shell,read_file}, db)，Arc<dyn MagService>：
1. create_session → 断言 SessionCreated。
2. 纯文本流式对话（消费 subscribe）→ RunStarted/TextDelta/RunFinished。
3. read 工具（auto，无审批）→ ToolStarted/ToolFinished(read_file)、无 InteractionRequested、RunFinished。
4. shell 工具（审批 approve）→ InteractionRequested(Approval) → respond_interaction(approve)
   → ToolStarted/ToolFinished(shell) → RunFinished。
5. 一次 deny → InteractionRequested → respond(deny) → 无工具事件 → RunFinished。
6. run 中途 cancel → RunStarted+TextDelta 后 cancel(s) → RunError "run cancelled"（不落库）。
drop engine1（join 线程，快照落盘）。
engine2 = 同 db 新 Engine（fake2 脚本 turn7）：
7. resume_session(s) → 续对话 → RunFinished；断言 fake2 的请求携带 turns 2–5 的历史片段（cancel 那轮不在）。

### 用例 2：concurrent_sessions_are_isolated_by_subscription（并发多会话隔离）
同一 engine 两会话 a/b，subscribe(Some(a))/subscribe(Some(b))，同时 send_message；
按 model 路由脚本使各自文本确定；断言各订阅只见本会话事件（session_id 过滤）、各自恰好一条 RunStarted+RunFinished、
文本与本会话对应。

## 进度
- [x] 读 TODO/PLAN/memory、engine/service/tools/approval/test_support、既有测试模式
- [x] 写 tests/e2e_offline.rs
- [x] 聚焦测试通过（2/2, 0.01s）
- [x] fmt / clippy / test --workspace / doc 全绿
- [x] TODO.md 标 [DONE] + 完成记录
- [x] 提交并停

## 下一个任务
C5-R Review：mag-core 整体验收 + 契约冻结（本次不做）。
