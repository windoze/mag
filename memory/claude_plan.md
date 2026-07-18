# Claude 执行计划 — C4-2 凭据存储（mag-sources）

## 当前任务
`TODO.md` 首个未完成任务：**[TODO] C4-2 凭据存储（`mag-sources`）**（行 1018）。
下一个是 C4-R（review），本次只做 C4-2。

## 规格（DESIGN §3.5 / PLAN R-C / TODO C4-2）
- `CredentialStore` trait + 内存实现（测试用）+ keyring 实现（生产，feature 隔离，测试不依赖真 keyring）。
- 凭据绝不进 snapshot；恢复时从 store 重注入 `ProviderConfig`。
- `mag-sources` 提供 `provider_config(source_id, creds) -> agent-lib ProviderConfig`。
- source registry：`register_llm(..)` + 预留 `register_local_agent(..)` 占位（返回未实现/留 trait 槽）。

## 关键事实
- agent-lib `ProviderConfig` 在 `agent_lib::facade::{ProviderConfig, FacadeError}`；builder
  `ProviderConfig::anthropic()/openai()` → `.base_url().api_key().api_version().build()`。
- `ProviderId`（`agent_lib::model::extras`，`#[non_exhaustive]`，Anthropic / OpenAiResp）。
- ProviderConfig 的 Debug 已 redacted、无 Serialize（凭据不落 snapshot）。
- keyring v3.6 需平台 backend feature；Entry::new/get_password/set_password/delete_credential，
  `keyring::Error::NoEntry` 表未找到。→ 把 keyring 设为 optional dep，feature `os-keyring`（非默认），
  默认 test/clippy/doc 不编译 keyring，彻底满足「测试不依赖真 keyring」。

## 设计（新模块）
- Cargo.toml：keyring optional=true；`[features] default=[]; os-keyring=["dep:keyring"]`。
- src/secret.rs：`Secret`（redacted Debug、无 serde/Display、expose()）。
- src/credentials.rs：`Credentials{api_key}`、`CredentialError`、`CredentialStore` trait、
  `MemoryCredentialStore`、`KeyringCredentialStore`（cfg os-keyring）。
- src/registry.rs：`SourceRegistry`、`LlmSource`、`LocalAgentSlot`/`LocalAgentKind`、
  `LocalAgentBackend`（预留 trait 槽）、`SourceError`、`provider_config`、
  `register_llm`/`register_local_agent`/`connect_local_agent`(→ LocalAgentUnsupported)。
- src/lib.rs：模块声明 + 公开再导出 + crate 文档。
- README：mag-sources 描述补一句。

## 验证
1. cargo fmt --all -- --check
2. cargo test -p mag-sources（聚焦）
3. cargo clippy --all-targets -- -D warnings（+ 额外 `-p mag-sources --features os-keyring`）
4. cargo test --workspace
5. cargo doc --no-deps --workspace（+ 额外 features os-keyring 检查）

## 进度
- [x] 写模块（secret/credentials/registry + Cargo feature os-keyring）
- [x] 聚焦测试 `cargo test -p mag-sources` 10/10 绿
- [x] 完整序列：fmt 干净；clippy 默认 + os-keyring 零告警；test --workspace 全绿；doc(默认+os-keyring, -D warnings) 通过
- [x] TODO.md C4-2 标 [DONE] + 完成记录；README mag-sources 描述更新
- [ ] 提交并停（不进 C4-R）

## 处置原则
无 workaround；spec mismatch 则修或插最小前置任务。完成后标 [DONE]+补记录+提交+停。
