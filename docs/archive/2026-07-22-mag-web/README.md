# 归档：mag-web 计划（单用户 web UI，与 desktop 共用 UI 基础设施）

本目录保存 mag **第三个 interface：`mag-web`**（单用户 web UI + `ui/` 前端 monorepo）及其前置
（service wire 补全、ts-rs 类型生成管线）的**已完成**实施计划快照，全部里程碑（W1–W5 + F-R
全计划 review）均已 `[DONE]`。此后进入下一份计划：**动态 subagent**（定义/实例分离、统一
`agent` 工具、local/external 统一实例化），其新计划见仓库根 `PLAN.md` / `TODO.md`，设计输入为
[`docs/dyn-agents.md`](../../dyn-agents.md)。

- [`PLAN.md`](PLAN.md)：mag-web 的实施计划（唯一设计输入为 [`docs/WEB.md`](../../WEB.md)）。
- [`TODO.md`](TODO.md)：mag-web 的逐任务清单，保留全部 `[DONE]` 完成记录作为历史。

设计源文档（仍在维护）：

- 全局设计：[`docs/DESIGN.md`](../../DESIGN.md)。
- web/desktop UI 设计：[`docs/WEB.md`](../../WEB.md)。
- 动态 subagent 设计：[`docs/dyn-agents.md`](../../dyn-agents.md)。

更早的归档：[`../2026-07-19-mag-service/`](../2026-07-19-mag-service/)（service 主干）、
[`../2026-07-20-mag-acp/`](../2026-07-20-mag-acp/)（interface #1 ACP）、
[`../2026-07-20-mag-cli/`](../2026-07-20-mag-cli/)（interface #2 CLI）。
