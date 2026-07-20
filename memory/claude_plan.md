# Execution Plan

1. Read `TODO.md` and identify the first task whose title is not prefixed with `[DONE]`.
2. Review that task's details, dependencies, validation requirements, and the latest commit only if it directly mentions unfinished work relevant to the selected task.
3. Inspect the minimal code, tests, and docs needed for the selected task; avoid broad historical triage.
4. Implement the task as written, or add the minimum prerequisite task to `TODO.md` and stop if a concrete blocker makes correct execution impossible.
5. Run validation in the required order: formatting first, linting next, then the relevant/full test suite unless only documentation changed since the last successful full run.
6. Update `TODO.md` by adding `[DONE]` to the completed task title and refreshing its completion record. Update `PLAN.md` only if phase-level sequencing or completion criteria changed.
7. Inspect git status/diff/log, commit all intended changes with a task-scoped message, then stop without starting the next task.

Progress:
- Started this invocation and refreshed the public execution plan before implementation work.
- Identified the first incomplete task as `W2-4 bin mag --web + 协议级 e2e`; scope is limited to CLI web startup wiring and protocol-level offline e2e validation.
- Implemented the bin-side web wiring: `mag --web` flags, mode validation, token URL printing, and a `mag-web` listener handoff that lets the bin print the actual bound address.
- Added the protocol-level web e2e test using a real `Engine`, scripted fake LLM, loopback HTTP/SSE clients, tool approval, pivot, cancel, history, config, and sources routes; entered validation starting with `cargo fmt --all`.
- Validation passed: `cargo fmt --all`, `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, focused `cargo test -p mag --bin mag`, `cargo test -p mag --test web_e2e`, `cargo test -p mag-web`, full `cargo test --workspace`, and `cargo doc --no-deps --workspace`.
- Marked `W2-4` complete in `TODO.md` with completion details; only documentation/progress files changed after validation, so the prior green validation remains applicable.
