# mag

`mag` is a transport-neutral coding-agent engine built on top of
[`agent-lib`](../agent-lib). This repository currently contains the core Rust
workspace skeleton for the engine and its direct support crates.

## Workspace

- `mag-service`: transport-neutral service protocol types (commands and events);
  later hosts the `MagService` trait.
- `mag-core`: engine entry point, session management, event bus, persistence,
  and driver integration.
- `mag-tools`: tool registry and built-in tool integration points.
- `mag-sources`: AI source and credential integration points.

Frontend and transport crates are intentionally out of scope for this workspace
stage.

## Setup

The workspace expects `agent-lib` to be available next to this repository:

```sh
../agent-lib
```

Build and test the current skeleton with:

```sh
cargo build --workspace
cargo test --workspace
```

## Usage

`mag-core::Engine` implements the transport-neutral `MagService` trait: create
sessions, stream chat turns, gate tool calls behind asynchronous approval, and
persist/resume sessions. Use `Engine::with_persistence(client, tools, path)` for a
durable SQLite-backed store whose committed snapshots survive a restart, or the
in-memory `Engine::with_llm_client(client)` for ephemeral use.
