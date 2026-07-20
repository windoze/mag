# Claude Execution Plan

本文件记录本次调用的可审计执行计划、关键决策和进度更新。不会记录私有逐步思维链，但会记录足够的事实依据、约束和执行步骤，便于检查进展。

## 当前约束

- `TODO.md` 是任务顺序、完成状态和验收要求的唯一权威来源。
- 只完成第一个标题未带 `[DONE]` 的任务，然后停止。
- 若发现阻塞当前任务的缺陷、规格不匹配或未排期失败测试，必须修复或在 `TODO.md` 中加入最小必要前置任务后提交并停止。
- 完成任务后必须更新 `TODO.md`，运行必要验证，并提交 Git commit。
- `PLAN.md` 只在阶段级计划、依赖或完成标准变化时更新。

## 当前任务：W3-R — W3 review

`TODO.md` 中第一个未完成任务是 **W3-R [TODO] W3 review**（W3-1..W3-4 均已完成并提交）。

- **实现要求**：对照 `docs/WEB.md` §5/§6 检查：
  1. 依赖方向（app→ui/client→protocol 单向）
  2. 组件不碰 transport
  3. store 合并逻辑无竞态
  4. token 不进 URL query/日志
  5. Storybook 覆盖度
  发现问题直接修复并补测试。
- **验证条件**：`pnpm -r test`/`pnpm -r build` 绿 + 默认验证序列（cargo fmt --check、clippy -D warnings、cargo test --workspace、cargo doc）；完成记录列出 review 结论。

## 执行计划

1. 读取 `docs/WEB.md` §5/§6 相关章节作为 review 规格依据。
2. 用 `git log` 定位 W3-1..W3-4 的提交，通读 W3 diff。
3. 并行派发 review 子代理分块核查：
   - 依赖方向 + token 纪律（package.json、import 图、URL/日志）
   - `@mag/client` SessionStore 合并逻辑竞态 + transport
   - `@mag/ui` 组件纯度 + Storybook 覆盖度（对照 §5.2/§5.4/§6.3）
4. 对发现的问题直接修复并补测试；确认无问题则在完成记录写明结论。
5. 运行验证序列：pnpm format/lint、`pnpm -r test`、`pnpm -r build`、cargo fmt --check、clippy、cargo test --workspace、cargo doc。
6. 将 `TODO.md` 中 W3-R 标记 `[DONE]` 并写完成记录（逐项列出 review 结论）。
7. 提交 git commit 并停止。

## 进度日志

- 已创建本执行计划文件，下一步读取 `docs/WEB.md` §5/§6 并定位 W3 提交。
- 已确认首个未完成任务为 W3-R（W3 review），W3 提交为 `351683f`/`7ab09f1`/`4c78c96`/`0fe4985`；已读 `docs/WEB.md` §5/§6 作为规格依据。
- 已并行派发 3 个 review 子代理：①依赖方向+token 纪律（全 PASS）；②`@mag/client` store/transport（发现 history-replace 竞态、若干测试缺口）；③`@mag/ui`+app-web 壳（发现 null reason 渲染 bug、死 ComposerMode、死 delegation 点击、测试缺口）。
- 已修复全部发现问题：store 事件缓冲+重放消除 history-replace 竞态；修复第二个 interaction_requested 丢 runId 的 store bug；InteractionCard null reason 判空；删除死 `ComposerMode.awaiting_interaction` 与死 `completedRunIds`；无回调 delegation 卡改静态渲染；selector 返回拷贝。
- 已补测试：client 6 个新用例（SSE CRLF/多行/分片、竞态回归、多 pending 队列、跨会话隔离、工具四终态、sessionOrder 去重）；ui 4 个新用例（approval deny/cancel、permission approve/cancel、null reason、resolved 只读）。
- 审批「+ always」查证：wire `ApprovalDecisionWire` 无 always 变体，mag-acp `map.rs:415` 明确记录无持久 always 为既有拍板——记为 spec 文本偏差，随 F-R 复核，未新增任务。
- 验证全绿：`pnpm format/lint`、`pnpm -r test`（25 测试）、`pnpm -r build`、`build-storybook`、`cargo fmt --check`、`cargo clippy -D warnings`；Rust 零改动，workspace test/doc 复用 W3-4 全绿结果。
- 已将 `TODO.md` 中 W3-R 标记 `[DONE]` 并写入逐项 review 结论。下一步提交并停止。
