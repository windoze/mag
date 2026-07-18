# mag

`mag` is a transport-neutral coding-agent engine built on top of
[`agent-lib`](../agent-lib). This repository currently contains the core Rust
workspace skeleton for the engine and its direct support crates.

## Workspace

- `mag-protocol`: wire protocol types for commands and events.
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

At this stage the crates are empty skeletons. Subsequent tasks will add the
transport-neutral `Command`/`Event` protocol and the `mag-core::Engine` API.
