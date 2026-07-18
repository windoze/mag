# Claude 执行计划 — C4-1 持久化层 + snapshot/restore

## 当前任务
`TODO.md` 首个未完成任务：**[TODO] C4-1 持久化层 + snapshot/restore**（行 937）。

### 目标（DESIGN §3.6 / PLAN R-D / R-B）
- SQLite 持久层：建表 + save_session/load_session/list_sessions/delete_session +
  save_snapshot/load_snapshot。snapshot 存 facade Agent::snapshot() 的 JSON blob。
- actor 每次 run 成功结束（committed 一致点）后取快照写库。
- resume_session：读快照 → Agent::restore() 重注入 provider/工具/approval（IpcApproval 经
  interaction_handler 重注入，R-B 已满足）→ 会话可继续。

## 设计
- 新模块 crates/mag-core/src/persistence.rs：Persistence（Mutex<rusqlite::Connection>，Arc 共享）
  + PersistenceError（公开）。schema：schema_meta(version)、sessions(id PK, config_json, created_at)、
  snapshots(session_id PK, agent_snapshot_json, committed_at)（latest-only）。
  依赖 rusqlite 0.32 features=["bundled"]。
- driver.rs：抽 tool/policy helper；新 restore 构造器；run_turn 增 store 参数，committed 时先
  snapshot 写库再 emit RunFinished（观测到 RunFinished ⟹ 快照已落库）。失败/取消不快照。
- session.rs：actor/session_thread/manager 增 store；session_thread 增 Option<AgentSnapshot>；
  manager::resume_session spawn 恢复线程。
- engine.rs：EngineInner 增 store；新增公开 Engine::with_persistence(client, tools, path)；
  create_session 持久化 config + 构造时按 DB 最大 session id 播种计数器；resume_session 读+restore+spawn；
  delete_session 删库。
- lib.rs：mod persistence + pub use PersistenceError。README 补一句。

## 验证条件
1. run 后 snapshot 写库；load_snapshot round-trip 与内存态一致。
2. 跨重启纯对话：A 两轮→快照→丢 Engine→新 Engine resume→第三轮见前两轮上下文；id 不冲突。
3. snapshot JSON 不含凭据/secret。
4. 跨重启需审批：committed 快照→resume 重注入 IpcApproval→审批走跨进程往返。
5. 聚焦 cargo test -p mag-core persist。
6. 完整序列 fmt→clippy→test--workspace→doc。

## 进度
- [完成] persistence.rs（Persistence + PersistenceError + 6 单测）。
- [完成] driver.rs restore + committed snapshot（先落库再 emit RunFinished）。
- [完成] session.rs actor/manager/session_thread 贯穿 store + resume_session/spawn_session。
- [完成] engine.rs with_persistence/resume_session/delete/list + SessionIdSource 续种。
- [完成] engine::persist 跨重启测试 4 个（含需审批会话）。
- [完成] fmt 干净；clippy -D warnings 零告警；cargo test --workspace 全绿
  （mag-core 46 / mag-service 10 / mag-sources 1 / mag-tools 6 + builtin_tools 13）；doc 通过。
- [完成] TODO.md C4-1 标 [DONE] + 完成记录；README Usage 更新。
- 下一步：提交并停（不进 C4-2）。

## 处置原则
- 无 workaround；spec mismatch 则修或插最小前置任务。完成后标 [DONE]+补记录+提交+停。
