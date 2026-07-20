# 归档：mag-acp 计划（mag 作为 ACP server，第一个 interface）

本目录保存 mag **第一个 interface：`mag-acp`**（把冻结的 `mag-service::MagService` 经 ACP agent
端暴露给 Zed 等 client）的**已完成**实施计划快照，全部里程碑（M1–M5）均已 `[DONE]`、放行判据
全部达成。此后进入第二个 interface：**mag-cli**（GUI/web 前置验证原型 + 运行时配置系统），
其新计划见仓库根 `PLAN.md` / `TODO.md`，设计输入为 [`docs/CLI.md`](../../CLI.md)。

- [`PLAN.md`](PLAN.md)：mag-acp 的实施计划（唯一设计输入为 [`docs/ACP.md`](../../ACP.md)）。
- [`TODO.md`](TODO.md)：mag-acp 的逐任务清单，保留全部 `[DONE]` 完成记录作为历史。

设计源文档（仍在维护）：

- 全局设计：[`docs/DESIGN.md`](../../DESIGN.md)。
- ACP interface 实现级设计：[`docs/ACP.md`](../../ACP.md)。
- CLI interface 与配置系统设计：[`docs/CLI.md`](../../CLI.md)。

更早的归档：service 主干（`mag-service` + `mag-core`）计划见
[`../2026-07-19-mag-service/`](../2026-07-19-mag-service/)。
