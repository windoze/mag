# Claude Execution Plan

## Scope

- Follow `TODO.md` as the authoritative task list.
- Complete exactly the first incomplete task, then stop after committing the result.
- Current task: `W3-3 [TODO] @mag/ui 核心组件 + Storybook`.
- Do not move to `W3-4` in this invocation.

## Execution Plan

1. Check the latest commit message only for unfinished work directly relevant to `W3-3`.
2. Inspect `docs/WEB.md` sections `§5.2`, `§5.4`, `§6.3`, the existing `@mag/ui` package, Storybook setup, Tailwind/shadcn config, and generated/client types needed only for component props.
3. Design a pure props-and-callback component surface for `ThreadView`, `ToolCallCard`, `InteractionCard`, `Composer`, and `SessionSidebar`, keeping `@mag/ui` transport-free.
4. Implement the components and centralized design tokens with focused patches.
5. Add Storybook stories covering required visual states for messages, tool cards, approval/question/choice interactions, composer idle/running/pivot/cancel states, and sidebar status/entry states.
6. Add component interaction tests, especially interaction-card submit callback payloads.
7. Run formatting first, then focused `@mag/ui` build/test/Storybook build, then required workspace/frontend and Rust validation commands.
8. If any unscheduled test failure appears, fix it or add the minimum prerequisite task before marking `W3-3` complete.
9. Update `TODO.md` by changing the `W3-3` heading to `[DONE]` and adding a completion record. Update `PLAN.md` only if phase-level plan changes.
10. Inspect git status/diff/log, commit all intended changes with a `W3-3` message, and stop.

## Progress Log

- Read `TODO.md` and identified first incomplete task: `W3-3 [TODO] @mag/ui 核心组件 + Storybook`.
- Initial `W3-3` execution plan recorded before running repository commands.
- Latest commit checked: `7ab09f1 [W3-2] Implement frontend client transport and store`; no directly relevant unfinished `W3-3` item was declared.
- Context inspection complete for `docs/WEB.md` §5.2/§5.4/§6.3, existing `@mag/ui`, and client/protocol shapes. Implementation will keep `@mag/ui` independent of `@mag/client`/`@mag/protocol` by defining pure component props that are structurally compatible with the protocol where needed.
- Approval interaction payloads will follow the existing CLI convention of using `call_id` as the placeholder `step_id` because the request kind carries `call_id` but not `step_id`.
- Implemented the first pass of `@mag/ui` core components and exports: design tokens, shared view types, `OriginBadge`, dependency-free `Markdown`, `ToolCallCard`, `InteractionCard`, `Composer`, `SessionSidebar`, and `ThreadView`.
- Added component interaction tests for `InteractionCard` approval/question/choice/permission payloads and Storybook stories covering tool statuses, interaction shapes, composer modes, sidebar states, and thread system states.
- Focused validation passed so far: `npx --yes pnpm@10.14.0 --filter @mag/ui build`, `npx --yes pnpm@10.14.0 --filter @mag/ui test`, and `npx --yes pnpm@10.14.0 --filter @mag/ui build-storybook` after narrowly filtering Storybook core's own `EVAL` warning in `.storybook/main.ts`.
- Full validation passed: `cargo fmt --all`, `npx --yes pnpm@10.14.0 format:write`, `cargo fmt --all -- --check`, `npx --yes pnpm@10.14.0 format`, `npx --yes pnpm@10.14.0 lint`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`, `npx --yes pnpm@10.14.0 -r test`, `npx --yes pnpm@10.14.0 -r build`, `npx --yes pnpm@10.14.0 --filter @mag/ui build-storybook`, and `cargo doc --no-deps --workspace`.
- `TODO.md` updated to mark `W3-3 [DONE]` with completion record. `PLAN.md` was not changed because phase-level sequencing and completion criteria did not change.
- Next step: inspect git status/diff/log, stage intended W3-3 files, commit, and stop.
