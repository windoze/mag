# Claude 执行计划 — H-1 归档 service 计划 + 起草 ACP interface 的 PLAN.md/TODO.md

## 任务
`TODO.md` 首个未完成任务 = **H-1**（纯计划编制任务，不写产品代码）。
- 唯一设计输入：`docs/ACP.md`（+ `docs/DESIGN.md` §5 概览、§3.0 `MagService` trait）。
- 范围：**mag-acp crate**（mag 第一个 interface，ACP agent 端）。

## 做什么（三步）
1. 归档旧计划：建 `docs/archive/2026-07-19-mag-service/`，把当前 `PLAN.md` + `TODO.md`（含全部 `[DONE]`）
   `git mv` 进去；加 `README.md`（一句话 + 链接回 docs/DESIGN.md / docs/ACP.md）。
   先把 H-1 在旧 TODO.md 标 `[DONE]` + 完成记录，再移动。
2. 写新根 `PLAN.md`（ACP interface）：唯一设计输入 = docs/ACP.md；范围/非目标/锚点/里程碑/约束/测试/验证序列。
3. 写新根 `TODO.md`（ACP interface）：M1..M5，每任务 `[TODO]` + 三段式 + 精确锚点 + 完整验证序列；
   每 milestone 末尾 `M<n>-R` review；顶部通用执行规则块。

## 里程碑划分（照 docs/ACP.md）
- M1 crate 骨架 + `map` 纯函数 + `initialize` + `session/new` + stdio/内存管道跑通（§1/§2/§3.1/§3.2）
  - M1-1 crate 骨架 + acp/mag-service 依赖 + `map`（session_id 映射 + agent_capabilities 保守宣告）纯函数
  - M1-2 bin 装配 + `initialize` handler + 内存管道 e2e 骨架（initialize 往返）
  - M1-3 `session/new` handler → create_session（cwd→SessionConfig）
  - M1-R review
- M2 `session/prompt` 泵（§3.4）+ 类型映射（§4）
  - M2-1 map: ServiceEvent→SessionUpdate + ContentBlock→UserInput + stop reason（纯函数）
  - M2-2 prompt handler 泵（subscribe→send_message→loop→PromptResponse）
  - M2-R review
- M3 审批桥接（§5）
  - M3-1 map: InteractionKindWire→RequestPermissionRequest + outcome→InteractionResponseWire
  - M3-2 bridge_permission 接入泵
  - M3-R review
- M4 cancel（§3.5）+ session/load（§3.3）+ 能力宣告收口（§7）
  - M4-1 session/cancel notification + 挂起权限 cancel 收尾
  - M4-2 session/load → resume_session + load_session/prompt 能力宣告收口
  - M4-R review
- M5 协议级 e2e（§9）+ 收官验收
  - M5-1 全回合内存管道 e2e（含流式+权限+cancel），真实 Zed `#[ignore]`
  - M5-R 收官 review

## 已核对事实（避免悬空引用）
- acp crate = `agent-client-protocol` v1.2.0，schema = `agent-client-protocol-schema` v1.4.0
  （经 `agent_client_protocol::schema::v1::*`），已在 ../agent-lib Cargo.lock 确认真实存在。
- MagService trait 方法（service.rs）：create_session/list_sessions/resume_session/delete_session/
  send_message→RunId/cancel/respond_interaction(id,request_id,resp)/subscribe(Option<SessionId>)→
  BoxStream<ServiceEvent>/list_sources/probe_local_agents。object-safe，`Arc<dyn MagService>`。
- ServiceEvent 变体：SessionCreated/RunStarted/RunFinished{output:RunOutput}/RunError{message}/
  TextDelta{text}/ToolStarted{trace:ToolTrace}/ToolFinished/InteractionRequested{request_id,kind:
  InteractionKindWire}/Delegation{Started,Finished,Failed,Message}/LocalAgentsProbed。
- wire 类型：SessionId/RunId/RequestId(uuid transparent)、UserInput{text,attachments}、
  InteractionKindWire{Approval{call_id,requirement},Question,Choice,Permission{...}}、
  InteractionResponseWire{Approval{step_id,call_id,decision,message},Answer,Choice,Permission}、
  ApprovalDecisionWire{Approve,Deny,Timeout,Cancel}、SourceInfo、SessionConfig{provider,model,
  tool_profile,routing}、ToolTrace{call_id,name,input,output,status,message}。
- mag-acp 依赖边界：只 mag-service + acp crate，**不** mag-core/agent-lib；装配 Engine 的 bin 单独放。

## 进度
- [x] 写 memory 计划（本文件）
- [x] 旧 TODO.md 标 H-1 [DONE] + 完成记录
- [x] git mv PLAN.md/TODO.md → docs/archive/2026-07-19-mag-service/ + 归档 README.md
- [x] 新根 PLAN.md（ACP）
- [x] 新根 TODO.md（ACP，M1..M5 + M*-R）
- [x] git diff --check 干净（CHECK_CLEAN）；提交并停

## 验证
纯文档任务，无 cargo 验证；`git diff --check` 干净（无行尾空白/冲突标记）。
