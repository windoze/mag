# Claude 执行计划 — C4-R Review：持久化与凭据安全（已完成）

## 结论
review-only，无源码改动。审计 persistence/driver/session/engine + mag-sources
secret/credentials/registry，逐条对照 DESIGN §3.5/§3.6/§9.5：
- snapshot 无 secret：两处断言（persistence + engine::persist），且 Secret 无 serde/Display 从类型堵死。
- 只在 committed 点取：run_turn 仅 Completed+output 时于 RunFinished 前落库；失败/取消不取。
- 恢复重装配：Agent::restore 重注入 client/tools/policy/IpcApproval；测试证明跨重启审批仍走 IpcApproval。
- id 续号：max_session_id_value 续种 max+1；测试断言新 id > 已存 max。
- 凭据只在 store：provider_config 现场注入；Secret redacted。
**未发现需新建前置任务的缺口。**

## 验证序列 1–5（全绿）
1. fmt --check 干净
2. test -p mag-core persist 10/10 + test -p mag-sources 10/10
3. clippy --all-targets -D warnings（默认 + mag-sources os-keyring）零告警
4. test --workspace：mag-core 46 / mag-service 10 / mag-sources 10 / mag-tools 6 + builtin_tools 13
5. doc --no-deps --workspace（+ os-keyring）通过

## 进度
- [x] 审计 persistence.rs / driver.rs / session.rs / engine.rs
- [x] 审计 mag-sources secret/credentials/registry
- [x] 跑验证序列 1–5（全绿）
- [x] 汇总缺口（无）
- [x] TODO.md C4-R 标 [DONE] + 完成记录
- [x] 提交并停

## 下一个任务
C5-1 端到端离线主干集成测试（本次不做）。
