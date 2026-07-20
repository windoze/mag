# Execution Plan

1. Read `TODO.md` first and identify the first task whose title is not prefixed with `[DONE]`.
2. Review that task's requirements, dependencies, validation criteria, and any relevant recent commit context.
3. Inspect only the code and tests needed to implement the selected task; avoid unrelated historical triage.
4. Implement the task completely, adding or updating focused tests where required.
5. Run formatting, linting, and relevant tests in the required order; fix any unscheduled failures or add prerequisite tasks if a concrete blocker prevents completion.
6. Update `TODO.md` by prefixing the completed task title with `[DONE]` and filling its completion record. Update `PLAN.md` only if phase-level planning changes.
7. Commit all changes for this invocation with a clear task-scoped commit message, then stop.

Progress:
- Started invocation and recorded the initial execution plan.
- Identified the first incomplete task as `W2-3 token auth + static assets`.
- Implemented the initial W2-3 changes: resolved token policy, `/api` bearer auth middleware, static file serving, SPA fallback, placeholder page, and focused tests.
- Entered validation: run formatting first, then focused Rust tests, linting, workspace tests, and docs.
- Validation passed: `cargo fmt --all`, `cargo test -p mag-web`, `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo check -p mag-web --release`, `cargo test --workspace`, and `cargo doc --no-deps --workspace`.
- Marked `W2-3` complete in `TODO.md`; next step is git diff review and commit.
