# Claude Execution Plan

## Scope
- Follow `TODO.md` as the authoritative task list.
- Identify and complete exactly the first incomplete task, where incomplete means the task heading is not prefixed with `[DONE]`.
- Stop after documenting, validating, and committing that one task.

## Execution Plan
1. Read `TODO.md` and identify the first incomplete task before doing broader triage.
2. Inspect only the files and tests relevant to that task, plus recent commit context if it is directly relevant.
3. Implement the task exactly as specified, adding prerequisite tasks to `TODO.md` only if a concrete blocker makes direct completion impossible.
4. Run formatting, linting, and relevant tests in the required order; run the full suite when code changes require it.
5. Update `TODO.md` with a `[DONE]` prefix and completion record if the task is fully complete; update this plan file at key milestones.
6. Review the git diff, then commit all task-related changes with a descriptive message.

## Progress
- Initial plan recorded before task execution.
- Identified first incomplete task: `W2-2 [TODO] SSE 事件面`.
- Current focus: inspect existing `mag-web` REST crate, `MagService::subscribe`, and `docs/WEB.md` §2.2 before implementing `/api/events`.
- Implemented `/api/events` SSE routing draft with per-connection bounded forwarding, monotonic event IDs, comment heartbeat, and subscription cleanup on disconnect.
- Added loopback-server SSE tests plus direct queue-overflow coverage; next step is formatting and focused validation.
- Validation passed: `cargo fmt --all`, `cargo test -p mag-web`, `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`, and `cargo doc --no-deps --workspace`.
- Marked `W2-2` complete in `TODO.md`; next step is git diff review and commit.
