# @mag/protocol

Generated TypeScript protocol bindings for mag service wire types.

## Regenerate

Run from the repository root:

```sh
cargo test -p mag-service --features ts-export export_ts
git diff --exit-code -- ui/packages/protocol/src
```

The first command regenerates `src/generated/` and `src/index.ts` from Rust `ts-rs` derives. The second command is the local/CI drift gate. Do not hand-write wire types in this package.

## Validate

```sh
cd ui/packages/protocol
npm install
npm run build
```

Until the full `ui/` workspace lands, this package can also be checked directly with `npx tsc --noEmit -p ui/packages/protocol/tsconfig.json`.
