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
- `mag-sources`: AI source registry and credential storage. Hosts the
  `CredentialStore` trait (in-memory backend for tests; OS keyring backend behind
  the non-default `os-keyring` feature so tests never touch a real keyring), the
  `SourceRegistry` that projects hosted LLM sources to an agent-lib
  `ProviderConfig`, and a reserved slot for future local-agent integration.
- `ui/`: pnpm frontend workspace for shared web/desktop UI infrastructure:
  generated protocol types, the client package, React UI package, and Vite web
  shell.

The web UI and additional transport crates are landing incrementally; the
protocol package is the first shared frontend artifact.

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

Regenerate and check protocol TypeScript bindings with:

```sh
cargo test -p mag-service --features ts-export export_ts
git diff --exit-code -- ui/packages/protocol/src
```

Install and validate the frontend workspace with:

```sh
cd ui
pnpm install
pnpm -r build
pnpm -r test
```

If `pnpm` is not installed globally, use `npx --yes pnpm@10.14.0 <command>`.

## Usage

`mag-core::Engine` implements the transport-neutral `MagService` trait: create
sessions, stream chat turns, gate tool calls behind asynchronous approval, and
persist/resume sessions. Use `Engine::with_persistence(client, tools, path)` for a
durable SQLite-backed store whose committed snapshots survive a restart, or the
in-memory `Engine::with_llm_client(client)` for ephemeral use.
