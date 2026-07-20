# Claude Execution Plan

## Scope

- Follow `TODO.md` as the authoritative task list.
- Identify and complete exactly the first incomplete task whose heading is not prefixed with `[DONE]`.
- Stop after committing that task, or after committing any required prerequisite/blocker update.

## Execution Plan

1. Read `TODO.md` to find the first incomplete task and its validation requirements.
2. Check the latest commit message only for unfinished work directly relevant to that task.
3. Inspect the files and tests relevant to the selected task.
4. Implement the task with minimal, targeted changes.
5. Run formatting first, then linting, then the task-required tests/full suite as applicable.
6. If any unscheduled test failure appears, fix it or add the minimum prerequisite task before marking the current task complete.
7. Update `TODO.md` by prefixing the completed task heading with `[DONE]` and filling its completion record.
8. Update `PLAN.md` only if phase-level sequencing or completion criteria change.
9. Inspect git status and diff, then commit all intended changes with a task-specific message.
10. Stop without starting the next task.

## Progress Log

- Initial execution plan recorded before running repository commands.
- Identified first incomplete task: `W3-2 [TODO] @mag/client: ITransport + HttpSseTransport + SessionStore`.
- Next step: inspect only W3-2-relevant docs, generated protocol types, current `@mag/client` skeleton, and latest commit message for directly relevant unfinished work.
- Context check complete: latest commit is W3-1 scaffolding and no directly relevant unfinished issue was found.
- Implementation approach: add focused `transport` and `store` modules under `ui/packages/client/src`, keep HTTP mapping inside `HttpSseTransport`, and drive `SessionStore` with protocol fixtures in Vitest.
- Implementation files added: `transport.ts`, `store.ts`, client package exports, and W3-2 Vitest JSON fixtures/tests.
- Next step: run focused `@mag/client` typecheck/tests, then fix any compile or behavioral failures before broader formatting/lint/test validation.
- Focused validation passed after correcting `run_finished` handling to match generated protocol (`run_finished` carries output but no `run_id`).
- Next step: run formatting, linting, workspace frontend validation, and Rust default validation commands required by the task discipline.
- Formatting completed (`cargo fmt --all`, `pnpm format:write`), and checks passed for `cargo fmt --all -- --check`, `pnpm format`, `pnpm lint`, and `cargo clippy --all-targets -- -D warnings`.
- Next step: run full workspace tests/builds (`cargo test --workspace`, `pnpm -r test`, `pnpm -r build`) followed by `cargo doc --no-deps --workspace`.
- Full validation passed: `cargo test --workspace`, `pnpm -r test`, `pnpm -r build`, and `cargo doc --no-deps --workspace`.
- `TODO.md` updated to mark `W3-2 [DONE]` with completion record. `PLAN.md` was not changed because no phase-level sequencing or criteria changed.
- Next step: inspect git status/diff/log, then commit the W3-2 changes only.
