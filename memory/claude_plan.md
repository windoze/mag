# Claude 执行计划 — C5-R Review：mag-core 整体验收 + 契约冻结

## 任务
`TODO.md` 首个未完成任务 = **C5-R**（review 任务，不得跳过）。
对照 `docs/DESIGN.md` §3/§4/§9 逐条验收 service 主干，冻结 `MagService` 契约（+ Command/Event wire 编码），
汇总遗留缺口，确认无未调度失败测试，跑完整验证序列 1–5。

## 交付物（来自验证条件）
1. `docs/DESIGN.md` §3/§4/§9 逐条对照表（写入 TODO.md 完成记录）。
2. `MagService` 方法 vs §3.0 对照表。
3. 契约冻结说明：MagService trait + Command/Event 冻结（后续只加方法/变体/字段，不改既有语义）。
   - 在 mag-service crate 级 rustdoc 里加一段持久的「Contract stability」说明（durable in-repo 记录）。
4. R 风险消解 / 转上层记录（R-B 已消解=agent-lib M7-F1；R-C=§10 I2）。
5. 放行判据：C0–C5 全 [DONE]、验证序列全绿、e2e_offline 稳定通过。
6. TS codegen：mag-service 无 ts-rs/schemars 依赖 → 未启用，跳过（设计里是「若启用」）。

## 已核对事实
- 依赖边界：mag-service 不依赖 agent-lib（只 async-trait/futures/serde/serde_json/uuid）✓；
  mag-core 依赖 agent-lib/mag-service/mag-sources/mag-tools/rusqlite/tokio，无 tauri/axum/ACP ✓。
- MagService object-safe：e2e_offline 经 `Arc<dyn MagService>` 驱动 ✓。
- trait 方法 = §3.0 近全集：create/list/resume/delete_session、send_message、cancel、
  respond_interaction、subscribe、list_sources、probe_local_agents ✓（send_message 用 UserInput 富化）。
- ServiceEvent / Event 变体一一对应（From<Event> for ServiceEvent）✓；#[non_exhaustive] 兼容机制在位。
- Command/Event 全变体 serde round-trip 测试在 lib.rs。

## 验证序列（release 判据必须全绿）
1. cargo fmt --all -- --check
2. cargo test -p mag-core --test e2e_offline（端到端离线主干）
3. cargo clippy --all-targets -- -D warnings
4. cargo test --workspace（≤30min）
5. cargo doc --no-deps --workspace

## 进度
- [x] 读 TODO/PLAN/memory + DESIGN §3/§4/§8/§9/§10/§11 + mag-service 源码 + 依赖边界
- [x] 加 mag-service crate 级「Contract stability」冻结说明
- [x] 跑验证序列 1–5 全绿（fmt/e2e/clippy/workspace/doc 全绿）
- [x] TODO.md C5-R 标 [DONE] + 完成记录（含对照表 + 冻结说明 + 缺口汇总）
- [x] 提交并停

## 下一个任务
H-1 归档 service 计划并为 ACP interface 起草新 PLAN.md + TODO.md（本次不做）。
