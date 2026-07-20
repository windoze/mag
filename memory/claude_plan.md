# 当前执行计划（W4-3）

## 任务
W4-3 [TODO] ConfigEditor（文本形态）+ Sources 页（TODO.md:548）
依据 `docs/WEB.md` §5.5（Q5：文本形态先行）。

## 步骤
1. 勘察：`docs/WEB.md` §5.5；`@mag/client` store 中 config/sources 现状（W3-2 已提
   config revision、sources state）；`@mag/ui` 组件惯例；app-web Sources/Config 占位路由。
2. `@mag/ui` 新增：
   - `ConfigEditor`（纯 props+回调）：TOML 文本展示/编辑（textarea 代码编辑器）、保存
     （PUT）、Reload、Apply（按钮旁注 D2 生效时机文案）、secret 引用原样显示、
     text/graph mode 插槽（graph 占位「后续版本」）。
   - `SourcesView`：表格 + Probe 按钮 + `local_agents_probed` 刷新。
3. `@mag/client`：核查/补齐 store 对 config（get/put/reload/apply）与 sources（list/probe）
   的辅助方法与 selector（W3-2 已有部分基础，尽量复用）。
4. app-web：Config/Sources 占位路由替换为实际页面，接 store；`config_changed` toast 已有。
5. 测试：
   - vitest：mock transport 断言 PUT 载荷、Reload/Apply/Probe 按钮行为、`local_agents_probed`
     刷新；
   - Storybook：ConfigEditor/SourcesView 新增视觉态。
6. 门禁：`pnpm format:write` → `pnpm format` → `pnpm lint` → `pnpm -r test` →
   `pnpm -r build` → `build-storybook`；cargo fmt --check + clippy（若 Rust 零改动，
   workspace test/doc 复用上次全绿结果并注明）。
7. TODO.md 标记 [DONE] + 完成记录；提交 `[W4-3] ...`；停止。

## 进度
- [x] 读取 TODO.md，确认首个未完成任务为 W4-3
- [x] 勘察现状（§5.5、store/transport 锚点、App.tsx 占位、SourceInfo/ConfigDto 生成类型）
- [x] 实现（委派 coder 子代理：client TOML+store 方法、ui 两组件+story、app-web 两页面、测试）
- [x] 测试 + 门禁（pnpm format/lint/test/build、build-storybook、cargo fmt+clippy 全绿；
      Rust 零改动，workspace test/doc 复用 W3-4 全绿结果）
- [x] TODO.md 标记 [DONE] + 完成记录
- [ ] 提交 `[W4-3] ...` 并停止

## 结果
W4-3 完成：ConfigEditor（文本形态，smol-toml 转换层在 @mag/client）+ SourcesView +
app-web Config/Sources 页接线；64 个前端测试全绿。下一个任务：W4-R。
