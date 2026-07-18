# 归档：mag service 主干计划（`mag-service` + `mag-core`）

本目录保存 mag **service 主干**（`mag-service` 抽象接口 + `mag-core` 引擎实现，含直接依赖的
`mag-tools` / `mag-sources` 核心）的**已完成**实施计划快照，全部里程碑（C0–C5）均已 `[DONE]`、
`MagService` 契约已冻结。此后进入第一个 interface：**mag-acp**（ACP agent 端），其新计划见仓库根
`PLAN.md` / `TODO.md`。

- [`PLAN.md`](PLAN.md)：service 主干的分阶段实施计划（唯一设计输入为 [`docs/DESIGN.md`](../../DESIGN.md)）。
- [`TODO.md`](TODO.md)：service 主干的逐任务清单，保留全部 `[DONE]` 完成记录作为历史。

设计源文档（仍在维护）：

- 全局设计：[`docs/DESIGN.md`](../../DESIGN.md)。
- ACP interface 实现级设计：[`docs/ACP.md`](../../ACP.md)。
