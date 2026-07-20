# Execution Plan

I will follow `TODO.md` as the authoritative task list and complete exactly the first task whose heading is not prefixed with `[DONE]`.

Planned steps:
1. Read `TODO.md` to identify the first incomplete task and its validation requirements.
2. Check recent Git context only insofar as it directly affects that task.
3. Inspect the relevant code and tests for the selected task.
4. Implement the task without workarounds or scope narrowing.
5. Run formatting, linting, and the task-required tests in the required order.
6. Update `TODO.md` with a `[DONE]` prefix and completion record if the task is fully complete.
7. Commit all relevant changes with a clear task-specific message, then stop.

If a concrete blocker prevents completion, I will add the minimum prerequisite task to `TODO.md`, keep the current task incomplete, commit the bookkeeping change, and stop.

## Current Task

Selected task: `M6-5 [TODO] bin 装配 + 端到端验证`.

Task-specific plan:
1. Inspect the latest commit, worktree status, `crates/mag/src/main.rs`, `crates/mag-cli`, `Engine::from_config`, and existing e2e helpers.
2. Wire the default `mag` binary path to load configuration, build `ConfigService`, create `Engine::from_config`, and run `mag-cli`; preserve `--acp`, `--config`, and add `--resume <id>` behavior.
3. Add offline e2e coverage using fake/scripted components to validate the binary/CLI/Engine integration required by M6-5.
4. Run formatting, focused tests, clippy, workspace tests, and docs in the required order.
5. Mark M6-5 `[DONE]` with a completion record, commit all relevant files, then stop.

Progress update:
- Identified that `crates/mag/src/main.rs` still treats `--acp` as mandatory, so M6-5 requires changing the default path to the terminal CLI.
- Identified that `mag-cli` always creates a new session at startup; `mag --resume <id>` needs a small `CliOptions` extension so startup can resume an existing session.
- The e2e strategy is to keep production assembly on `Engine::from_config`, and use in-process tests with real `Engine` + `mag-cli` plus local fake `LlmClient` / fake ACP process for offline coverage.
- Implemented the bin default CLI path and startup resume option, preserving `--acp` and `--config`.
- Added bin smoke coverage plus real Engine + mag-cli offline e2e coverage for conversation, ask_user, local delegation, external ACP delegation, pivot, cancel, config reload, and resume.
- Focused checks so far: `cargo test -p mag-cli` passed; `cargo test -p mag` passed after adjusting the resume smoke to use a provider-backed config so driver restore is valid without network calls.
- Full validation passed: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`, and `cargo doc --no-deps --workspace`.
- `TODO.md` now marks M6-5 as `[DONE]` and records the implementation, tests, dependency boundary notes, and validation results.
