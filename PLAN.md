# 实施计划：mag-web（单用户 web UI，与 desktop 共用 UI 基础设施）

> **唯一设计输入**：[`docs/WEB.md`](docs/WEB.md)（mag-web 的实现级设计，含决策 D1–D8、§2 REST+SSE
> 协议、§3 service 前置清单、§5 UI 信息架构、§6 共享 UI 基础设施；开放问题已于 2026-07-20 全部
> 拍板）。参照 [`docs/DESIGN.md`](docs/DESIGN.md) §3.0（`MagService` trait）、§4（Command/Event
> wire 编码）、[`docs/CLI.md`](docs/CLI.md)（pivot/配置/origin 语义继承）。
>
> 前置状态：
> - service 主干计划归档在 [`docs/archive/2026-07-19-mag-service/`](docs/archive/2026-07-19-mag-service/)。
> - interface #1 `mag-acp` 归档在 [`docs/archive/2026-07-20-mag-acp/`](docs/archive/2026-07-20-mag-acp/)。
> - interface #2 `mag-cli`（含 pivot、origin 归因、运行时配置系统、delegation 接线、ask_user）
>   已全部 `[DONE]`，归档在 [`docs/archive/2026-07-20-mag-cli/`](docs/archive/2026-07-20-mag-cli/)。
>
> **本计划覆盖 `docs/WEB.md` §3 的 service 前置改动 + `mag-web` crate + `ui/` 前端 monorepo。**
> desktop（Tauri 壳）不在本计划（另立计划，本计划为其铺设可复用基建）。逐任务清单见
> [`TODO.md`](TODO.md)。

## 目标

落地 `docs/WEB.md` 描述的 **mag-web**：单用户经浏览器访问本机 mag 的 channel，UI 参照 codex
desktop，API 为 REST 命令面 + SSE 事件面（gateway/proxy 友好），并与未来 desktop app 共用同一套
UI 基础设施（`@mag/protocol` + `@mag/client` + `@mag/ui` + 双薄壳）。具体交付：

- **service 前置**（`docs/WEB.md` §3）：wire `Command` 补 pivot/配置五变体（P1）；`ServiceError`
  kind 投影（P2）；`get_session_history` + `HistoryEntry`（含 tool call 与 delegation 记录，P3）；
  `SessionInfo` 增强（title/last_active_at/status，P4）。
- **ts-rs 类型生成管线**（`docs/WEB.md` §2.4）：wire 类型加 `#[derive(TS)]`（feature-gated），
  构建期生成 `@mag/protocol` 的 TS 声明，diff 门禁防漂移。
- **`mag-web` crate**（`docs/WEB.md` §1/§2）：纯协议翻译器——REST 路由表（§2.1）→ `MagService`
  方法；SSE 事件面（`subscribe` → SSE 帧广播、heartbeat、慢消费者断连）；token auth（默认生成 /
  `--token` / `--no-auth`，非 loopback 强制）；静态资源（debug 读目录 / release rust-embed 嵌入）；
  bin `mag --web` 子命令同走配置系统。
- **`ui/` 前端 monorepo**（`docs/WEB.md` §5/§6）：pnpm workspace；`@mag/client`（ITransport +
  HttpSseTransport + SessionStore 状态权威 + 历史合并/重连对齐）；`@mag/ui`（React + Tailwind +
  shadcn 组件库 + Storybook：ThreadView/Composer/ToolCallCard/InteractionCard/DelegationCard/
  SessionSidebar/ConfigEditor/SourcesView）；`@mag/app-web` 薄壳打通全部功能。
- **充分的离线测试**（`docs/WEB.md` §8）：Rust 侧内存请求 + 回环 SSE + fake LLM；前端 vitest
  scripted Event 流驱动 SessionStore；Storybook 视觉态；真实浏览器联调 `#[ignore]`。

## 非目标（本计划）

- **多用户/多租户/内建 TLS**：单用户单实例；多用户 auth 属未来外部 gateway（`docs/WEB.md` §0/§4）。
- **desktop 本机专属功能**：桌面通知、keyring、菜单栏/托盘等——只预留 capability 插槽（§6.4），
  不实现；Tauri 壳本身也不在本计划。
- **移动端布局、富附件、文件树/diff/终端模拟器**（`docs/WEB.md` §0 非目标）。
- **WebSocket**：拍板不采用（决策 D2）。
- **slash 命令、`@` 提及**：web 命令面即 GUI 元素 + REST 路由（决策 D7）。
- **不改已冻结契约的既有语义**：对 `MagService`/wire 只做向后兼容新增（方法/变体/字段）。
- **mag-acp / mag-cli 行为变更**：两个既有 interface 一行不改（共享的 wire 新增对它们透明）。

## 里程碑

逐层增加概念、每层可独立**离线**验证。顺序照 `docs/WEB.md` §9。

### W1 — service 前置 + ts-rs 管线（`docs/WEB.md` §3/§2.4）

wire `Command` 补 pivot/配置变体；`ServiceError::kind()`；`get_session_history` + `HistoryEntry`
（含 ToolCall/Delegation 变体）；`SessionInfo` 增强；ts-rs feature + `@mag/protocol` 生成与 diff 门禁。
重点文件：`crates/mag-service/src/{lib.rs,service.rs}`、`crates/mag-core/src/engine.rs`（历史查询
实现）、`ui/packages/protocol/`（新）。

### W2 — mag-web crate（`docs/WEB.md` §1/§2/§4）

REST 路由表 + 错误投影；SSE 事件面（广播/heartbeat/背压断连）；token auth 中间件 + 静态资源
（debug 目录/release 嵌入）；bin `mag --web [--host] [--port] [--token] [--no-auth]` 装配 +
协议级 e2e。重点文件：`crates/mag-web/src/*`（新）、`crates/mag/src/main.rs`。

### W3 — 前端核心（`docs/WEB.md` §6 + §5.1/§5.2/§5.4）

`ui/` monorepo 脚手架（pnpm/Vite/Tailwind/shadcn/vitest/Storybook）；`@mag/client`（ITransport +
HttpSseTransport + SessionStore）；`@mag/ui` 核心组件；app-web 壳打通「会话列表 + 流式对话 +
审批交互」闭环。重点文件：`ui/`（新）。

### W4 — 功能完备（`docs/WEB.md` §5.2–§5.5）

delegation 可视化（内联卡 + 右栏子线程、origin 徽标）；composer pivot/cancel 两层语义 + Pivot
事件渲染；ConfigEditor（文本形态）+ Sources 页。重点文件：`ui/packages/{client,ui}/`、`ui/apps/web/`。

### W5 — e2e 加固（`docs/WEB.md` §8）

断线重连全量对齐、多标签页、历史合并的端到端加固；Storybook 视觉态补全；全计划 review。

## 完成定义

- `TODO.md` 中全部任务（含各 `W<n>-R` review 任务与 `F-R`）标 `[DONE]` 且有完成记录。
- 每任务通过门禁序列：`cargo fmt --all -- --check` → 聚焦测试 → `cargo clippy --all-targets --
  -D warnings` → `cargo test --workspace` → `cargo doc --no-deps --workspace`；前端任务追加
  `pnpm -r test` / `pnpm -r build` 绿。
- 全部测试离线（scripted `Arc<dyn MagService>` / fake LlmClient / 内存 HTTP / vitest fixture），
  单测试 < 1 分钟；真实浏览器联调 `#[ignore]` 干净跳过。
- 对冻结契约只加不改；公开 API 全部带 rustdoc；`@mag/protocol` 全部 ts-rs 生成、无手写漂移。
- 全部完成后做一次完整 review（`F-R`），修复发现的问题。
