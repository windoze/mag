# Execution Plan

## Scope

- Follow `TODO.md` as the authoritative task list.
- Identify the first task whose heading is not prefixed with `[DONE]`.
- Complete exactly that task, then stop after committing the result.

## Step-by-Step Plan

1. Read `TODO.md` first and identify the first incomplete task by heading prefix.
2. Check the latest commit message for a directly relevant unfinished issue only after selecting the current task.
3. Inspect the files and tests relevant to the selected task.
4. Implement the smallest correct change that fully satisfies the task requirements.
5. Update this plan file when a key step is completed or if the plan changes.
6. Run required validation in order: `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, then the relevant/full test suite as required by the task and current changes.
7. If validation reveals unscheduled failures, fix them when in scope or add the minimum prerequisite task(s) to `TODO.md` before marking the current task done.
8. Mark the completed task heading in `TODO.md` with `[DONE]` and update its completion record.
9. Inspect git status, diff, and recent log; commit all intended changes with a descriptive task-scoped message.
10. Stop without starting the next task.

## Current Status

- Selected first incomplete task: `W1-1 [TODO] mag-service：Command 补 pivot/配置变体 + ServiceError::kind`.
- Latest commit checked: no directly relevant unfinished W1-1 issue found.
- Current worktree before code changes: only this plan file is modified.
- Implemented W1-1 in `mag-service`: added five `Command` variants, added `ServiceError::kind()`, and added coverage for command tags plus all error kinds.
- Checked `Command` consumers: no dispatcher consumes `mag_service::Command`; search hits outside `mag-service` are unrelated `SessionCommand` or `std::process::Command` usages.
- Validation passed: `cargo fmt --all`, `cargo test -p mag-service`, `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`, and `cargo doc --no-deps --workspace`.
- `TODO.md` updated: `W1-1` is marked `[DONE]` with completion record.
- Next step: inspect git diff/status/log and commit the W1-1 changes only.
