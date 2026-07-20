# mag

`mag` is a transport-neutral coding-agent engine built on top of
[`agent-lib`](../agent-lib). This repository contains the core Rust workspace
(engine and support crates), three interfaces (terminal CLI, ACP bridge, web
UI), and the shared web/desktop frontend infrastructure.

## Workspace

- `mag-service`: transport-neutral service protocol — the `MagService` trait,
  wire command/event types, and the ts-rs export pipeline for TypeScript
  bindings.
- `mag-core`: engine entry point, session management, event bus, persistence,
  and driver integration.
- `mag-config`: runtime configuration system (file watch, snapshots, DTOs).
- `mag-tools`: tool registry and built-in tool integration points.
- `mag-sources`: AI source registry and credential storage. Hosts the
  `CredentialStore` trait (in-memory backend for tests; OS keyring backend behind
  the non-default `os-keyring` feature so tests never touch a real keyring), the
  `SourceRegistry` that projects hosted LLM sources to an agent-lib
  `ProviderConfig`, and a reserved slot for future local-agent integration.
- `mag-cli`: terminal CLI interface (read-eval loop over `MagService`).
- `mag-acp`: ACP (Agent Client Protocol) bridge over stdio.
- `mag-web`: REST + SSE web interface — a pure protocol translator facing
  `Arc<dyn MagService>`; serves the SPA build output and enforces bearer-token
  auth.
- `mag`: the `mag` binary assembling the engine and the three interfaces.
- `ui/`: pnpm frontend workspace for shared web/desktop UI infrastructure:
  generated protocol types, the client package, React UI package, and Vite web
  shell.

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

The `mag` binary exposes three interfaces over the same engine:

```sh
mag                       # terminal CLI
mag --acp                 # ACP bridge over stdio
mag --web                 # web UI at http://127.0.0.1:8080/ (prints a URL whose
                          #   #t=<token> fragment carries the generated bearer token)
mag --web --host 127.0.0.1 --port 9090 --token <t>   # explicit bind/token
mag --web --no-auth       # loopback only; ignored (with a warning) otherwise
```

The web UI is a single-page app served by `mag-web`: in debug builds it reads
`ui/apps/web/dist/` (run `pnpm -r build` under `ui/` first), in release builds
the assets are embedded. All `/api` requests require
`Authorization: Bearer <token>`; the SPA reads the token from the printed URL's
`#t=` fragment automatically.
