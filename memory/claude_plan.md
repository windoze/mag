# 执行计划 — F-R [TODO] 全计划 review

## 任务定义（TODO.md F-R）
全部里程碑（W1–W5）完成后对整轮改动做一次完整 review（可分子代理分块），对照
`docs/WEB.md` 全节（决策 D1–D8）逐条核对。重点清单：
1. 冻结契约只加不改（mag-acp/mag-cli 不受影响）
2. REST 路由表与 §2.1 逐条一致
3. SSE 可靠性（heartbeat/背压/清理/帧格式）
4. auth 三态 + 非 loopback 强制（§4）
5. ts-rs 无手写漂移（§2.4）
6. 依赖边界（cargo tree -p mag-web 白名单 + pnpm 依赖方向）
7. secret 纪律（引用形态、不物化、不进日志/测试）
8. 离线测试纪律（无网络/凭据/真 LLM；单测试 <1min；真实浏览器 #[ignore] 干净跳过）
9. rustdoc/TSDoc 完整性
10. §0 目标清单逐项达成
发现的问题直接修复并补测试。验证：默认验证序列 + `pnpm -r test`/`pnpm -r build` 全绿；
完成记录列出 review 发现与修复清单。

## 现状
- HEAD=48b801a（W5-1 DONE），git 工作区干净。
- 各里程碑 review（W1-R/W2-R/W3-R/W4-R）均已 DONE，发现项已当场修复；记录在案的偏差
  全部显式列出（见 TODO.md 各完成记录），F-R 需复核这些偏差是否仍准确、是否需闭环。
- W1-R/W4-R 遗留「随 F-R 复核/评估」事项：
  a. §5.2「批准/拒绝（+ always）」与 wire 不符（无 always 变体，mag-acp 已拍板无持久
     always 语义）——复核是否修订 docs/WEB.md 文本。
  b. delegation 消息不入 history（跨页面加载子线程消息丢失）——wire 模型限制，评估是否
     接受为首版限制。
  c. ts-rs 漂移门禁无 CI 承载（无 CI 环境，记录在案）。
  d. history 投影对受损快照硬错误（单条坏记录可拖垮 GET /api/sessions）——W1-R 建议
     W2/W5 加固，需确认是否已处理或仍开放。

## 执行步骤
1. [ ] 派 4 个并行 review 子代理（explore，只读）分块核查：
   - A：后端契约 + ts-rs（§3 P1–P4、§2.4、只加不改、rustdoc、serde roundtrip 覆盖）
   - B：mag-web + bin（§2.1/§2.2/§2.3/§4/§1.3、依赖边界、纯翻译器纪律）
   - C：前端（§5 信息架构逐项、§6 分层/纯度、token 纪律、store、Storybook、TSDoc、
     pnpm 依赖方向）
   - D：§0 目标清单端到端 + D1–D8 逐条 + secret/离线测试纪律 + 文档完整性 +
     上述 a–d 遗留事项复核
2. [ ] 汇总发现，按 bug 级/小问题分级，逐一修复并补测试（本人在主上下文或派 coder
   子代理修复）。
3. [ ] 验证序列：cargo fmt --check → 聚焦测试 → clippy -D warnings → cargo test
   --workspace → cargo doc；pnpm format/lint/-r test/-r build/build-storybook。
   （若仅文档改动可复用既有绿结果。）
4. [ ] TODO.md：F-R 标记 [DONE] + 完成记录（review 发现与修复清单、§0 逐项结论）。
   PLAN.md 仅在阶段计划实际变化时更新。
5. [ ] 提交并停止（F-R 是最后一个任务；全部 DONE 后的 endtag 由后续 invocation 处理）。

## 进度日志
- 2026-07-21：确认首个未完成任务为 F-R（唯一未 DONE 项）。已读 TODO.md/PLAN.md/
  docs/WEB.md 全文。计划派 4 个并行 review 子代理。
- 4 个 review 子代理（后端契约+ts-rs / mag-web+bin / 前端 / §0+D1–D8+横切）全部结论
  **放行，无 bug 级发现**；建议级清单汇总后逐项处置如下（全部本轮闭环）：
  1. `mag --acp --web` 互斥分支补单测（main.rs `acp_and_web_are_mutually_exclusive`）✅
  2. loopback `--no-auth` 提示改为警告措辞（"warning: mag web auth is disabled (--no-auth)…"）
     + `web_startup_warns_when_auth_is_disabled` 测试 ✅
  3. mag-service 补小枚举全变体 roundtrip（RunErrorKind/RoutingMode/ToolStatusWire/
     DelegationStatusWire/ApprovalDecisionWire/PermissionDecisionWire/SourceKindWire/
     SessionStatusWire）+ ServiceError 全 7 变体 roundtrip ✅
  4. web_e2e 新增 `web_protocol_e2e_covers_resume_update_config_and_delete`（resume 204 +
     history 存续、PUT config 204 + config_changed + GET 回读 + 落盘、delete 204 + 列表
     消失 + history 404）✅
  5. cli.rs flaky 修复：`free_port()` 先绑后放 TOCTOU → 改为 `--port 0` 让子进程自选端口，
     从 stderr 启动行解析实际端口（mpsc + 超时读取）✅
  6. docs/WEB.md 修订三处 spec 文本滞后：§5.2 删「+ always」（注明 wire 无该变体）、
     §5.1「hover 菜单」→「hover 删除按钮」、§2.3 补 JsonRejection 非标准体注记 ✅
  7. 根 README.md 重写 Workspace 清单（补 mag-config/mag-cli/mag-acp/mag-web/mag bin）+
     Usage 补三 interface 用法（含 `--web` flags 与 token 模型）✅
- 记录在案不修复项（review 结论一致）：list_sessions 标题投影对受损快照硬错误（仅外部
  破坏触发，W1-R 起在案）；Capabilities 对象未预留（kind 字段已构成探测基础，desktop
  阶段第一批任务立案）；delegation 消息不入 history（wire 限制，首版接受）；
  `get_session_history` 无默认 trait 实现；selector 浅拷贝共享 item；跨标签页陈旧
  pending 卡；pivot notice 短暂性；JsonRejection 非标准体（已在 WEB.md §2.3 注记）。
- 验证：聚焦测试全绿（mag-service round_trip 15、mag bin 6、web_e2e 5/5 含新测试、
  cli web_binary）；pnpm format/lint/-r test（app-web 12 等全绿）/-r build 通过。
- 全量验证序列全绿：cargo fmt --check / clippy -D warnings / cargo test --workspace /
  cargo doc --no-deps --workspace；pnpm format/lint/-r test/-r build。
- TODO.md：F-R 标记 [DONE] 并写完成记录（逐项结论 + 修复 7 项 + 记录在案 12 项）。
  TODO.md 全部 23 个任务标题均已 [DONE]（唯一 [TODO] 命中为通用规则文本）。
  提交 [F-R] commit 并按 Completion & Release 创建 `endtag` 标签后停止。
