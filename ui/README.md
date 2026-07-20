# mag UI Workspace

`ui/` is the shared frontend workspace for mag web and future desktop shells.
It contains generated protocol types, a transport-independent client package, a
React component package, and the web app shell served by `mag-web`.

## Packages

- `packages/protocol`: generated TypeScript bindings from `mag-service` wire types.
- `packages/client`: transport-independent client and state layer skeleton.
- `packages/ui`: React component library, Tailwind tokens, and Storybook skeleton.
- `apps/web`: Vite web shell that will be served from `ui/apps/web/dist/`.

## Setup

Install dependencies from this directory:

```sh
pnpm install
```

If `pnpm` is not installed globally, use the workspace package manager version:

```sh
npx --yes pnpm@10.14.0 install
```

## Commands

```sh
pnpm dev              # run the web app shell
pnpm build            # build all packages and apps
pnpm test             # run all frontend tests
pnpm lint             # run ESLint across the workspace
pnpm format           # check formatting
pnpm protocol:generate
pnpm protocol:check
pnpm --filter @mag/ui storybook
```

`protocol:generate` runs `cargo test -p mag-service --features ts-export export_ts`
from the repository root and refreshes `packages/protocol/src/generated/`.

## Manual Web Smoke

The app shell is covered by vitest with a scripted transport. For a real `mag-web`
smoke against the W2 server, run from the repository root:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
npx --yes pnpm@10.14.0 --dir ui -r build
cargo run -p mag -- --web --host 127.0.0.1 --port 3000
```

Open the printed URL, including the `#t=<token>` fragment when auth is enabled.
Create or select a session, send a message, approve any interaction card, try a
pivot while a run is active, and press cancel to verify the REST+SSE loop.
