# Claude 执行计划 — C3-R Review：工具 + 审批正确性（重点）

## 当前任务
`TODO.md` 首个未完成任务：**C3-R Review：工具 + 审批正确性（重点）**（行 880）。

### 任务要求
**做什么**：重点核对审批暂停语义（machine 真停到 resolve，非 facade 式先 emit 再同步决策）；工具
worktree/cancel 约束生效；`PermissionDecider` 钩子留位正确（§8.1）；auto/ask 策略与 plugin 元数据一致。汇总缺口。

**验证条件**：完整验证序列 1–5 全绿；审批三路径（approve/deny/cancel）+ auto/ask 均有测试。

## 审查清单（逐项核对）
1. [ ] 审批暂停语义：IpcApproval::fulfill 真正 .await park 到 resolve，而非 facade 同步「先 emit 再决策」。
2. [ ] 工具 worktree 约束：safe_join/path 归一，禁 .. 逃逸与绝对路径。
3. [ ] 工具 cancel 约束：shell cancel token → 中断子进程。
4. [ ] PermissionDecider 钩子留位（§8.1）：trait + 默认 AskFrontendDecider 正确。
5. [ ] auto/ask 策略与 plugin permission() 元数据一致：read/list/grep auto-allow、shell ask。
6. [ ] 三路径测试覆盖：approve / deny / cancel。
7. [ ] auto/ask 均有测试。
8. [ ] 汇总前向缺口（非阻塞）。

## 验证序列
1. cargo fmt --all -- --check
2. 聚焦测试（审批 + tool_turn）
3. cargo clippy --all-targets -- -D warnings
4. cargo test --workspace
5. cargo doc --no-deps --workspace

## 进度
- [进行中] 阅读 C3 源码（approval.rs / driver.rs / plugin.rs / registry.rs / tools / path.rs）。

## 处置原则
- 纯 review 任务；若发现 spec mismatch/bug 则在当前任务修复或插入最小前置任务。
- 若无阻塞缺口，核对 + 跑全序列 + 写完成记录 + 标 [DONE] + 提交。

## 结果（2026-07-19）
- C3-R 纯 review 完成：审查清单 1–8 全部核对通过，无阻塞缺口/spec 偏离，未改源码。
- 关键确认：SessionActor::run 对 RespondInteraction 在 run 在飞时即时路由 → parked driver 必唤醒（无死锁不变量）。
- 验证序列 1–5 全绿：fmt ✓ / approval 11 + tool_turn 3 ✓ / clippy 0 警告 ✓ / workspace（core 34, service 10, sources 1, tools 6+13）✓ / doc 无警告 ✓。
- TODO.md 已把 C3-R 标 [DONE] 并补完成记录。下一未完成任务：C4-1 持久化层 + snapshot/restore。
