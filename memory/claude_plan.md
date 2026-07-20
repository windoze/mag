# Claude Execution Plan

## Scope

This invocation will complete exactly the first incomplete task in `TODO.md`, then stop after committing the result.

## Plan

1. Read `TODO.md` to identify the first task whose heading is not prefixed with `[DONE]`.
2. Review the selected task requirements, dependencies, validation instructions, and any relevant recent commit notes.
3. Inspect only the code and tests needed for the selected task.
4. Implement the task completely, or add the minimum prerequisite task to `TODO.md` if a concrete blocker makes completion impossible.
5. Update this file when key execution steps complete or the plan materially changes.
6. Run required formatting, linting, and tests in the required order unless only documentation changed and a prior green full run can be reused.
7. Mark the selected task `[DONE]` in `TODO.md` and update its completion record after successful validation.
8. Commit all relevant changes with a descriptive task-scoped message.
9. Stop without starting the next task.

## Progress

- Initial execution plan recorded.
- Selected first incomplete task: `W2-1 [TODO] crate 骨架 + REST 路由 + 错误投影`.
- Reviewed `docs/WEB.md` §2.1/§2.3 and `MagService`; implementation will add a new `mag-web` crate with REST-only routes for W2-1, leaving SSE/static/auth for later W2 tasks.
- Added the `mag-web` crate, REST router/serve entry, error projection, and handler-level scripted service tests.
- Focused validation passed: `cargo test -p mag-web`.
- Default validation passed: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`, and `cargo doc --no-deps --workspace`.
- `TODO.md` now marks `W2-1` as `[DONE]` with completion details.
- Runtime dependency check `cargo tree -p mag-web -e normal` shows no `mag-core` or `agent-lib`; `mag-config` appears only transitively through `mag-service`'s existing `ConfigDto` re-export.
- Next step: stage the W2-1 changes, inspect the staged diff, commit, then stop.
