//! Round-trip tests against the `docs/CLI.md` §4.2 example TOML, verbatim.

use mag_config::{BudgetDto, ConfigDto, SecretRef};

/// The §4.2 example, copied character-for-character from `docs/CLI.md`
/// (including comments). This is the minimal complete configuration surface.
const EXAMPLE_TOML: &str = r#"
# ~/.config/mag/config.toml（DTO 的 TOML 形态）
[providers.anthropic]
wire = "anthropic"
base_url = "https://api.anthropic.com"
api_key = { env = "ANTHROPIC_API_KEY" }     # secret 引用，不落盘

[providers.local_proxy]
wire = "openai"
base_url = "http://127.0.0.1:8317"
api_key = { keyring = "mag/local_proxy" }

[agents.default]
provider = "anthropic"
model = "claude-sonnet-4-5"
tools = ["read_file", "list_dir", "grep", "shell", "ask_user"]

[agents.reviewer]
provider = "local_proxy"
model = "gpt-5-codex"
tools = ["read_file", "grep"]

[external_agents.peer_acp]                  # external ACP agent 来源（决策 D3）
kind = "acp"
command = ["peer-agent", "--acp"]           # spawn 命令行；工作目录隔离由 agent-lib 负责

[tools.shell]
approval = "ask"                            # ask | allow | deny（→ ApprovalPolicy 映射）

[session]
routing = "model_routed"
budget = { max_tokens = 200000 }
"#;

#[test]
fn example_toml_parses_into_expected_dto() {
    let dto = ConfigDto::parse_str(EXAMPLE_TOML).expect("§4.2 example must parse");
    dto.validate().expect("§4.2 example must validate");

    // providers
    let anthropic = dto.provider("anthropic").expect("providers.anthropic");
    assert_eq!(anthropic.wire.as_deref(), Some("anthropic"));
    assert_eq!(
        anthropic.base_url.as_deref(),
        Some("https://api.anthropic.com")
    );
    assert_eq!(anthropic.api_key, Some(SecretRef::env("ANTHROPIC_API_KEY")));

    let local_proxy = dto.provider("local_proxy").expect("providers.local_proxy");
    assert_eq!(local_proxy.wire.as_deref(), Some("openai"));
    assert_eq!(
        local_proxy.base_url.as_deref(),
        Some("http://127.0.0.1:8317")
    );
    assert_eq!(
        local_proxy.api_key,
        Some(SecretRef::keyring("mag/local_proxy"))
    );

    // agents
    let default = dto.agent("default").expect("agents.default");
    assert_eq!(default.provider.as_deref(), Some("anthropic"));
    assert_eq!(default.model.as_deref(), Some("claude-sonnet-4-5"));
    assert_eq!(
        default.tools.as_deref(),
        Some(
            ["read_file", "list_dir", "grep", "shell", "ask_user"]
                .map(str::to_string)
                .as_slice()
        )
    );

    let reviewer = dto.agent("reviewer").expect("agents.reviewer");
    assert_eq!(reviewer.provider.as_deref(), Some("local_proxy"));
    assert_eq!(reviewer.model.as_deref(), Some("gpt-5-codex"));
    assert_eq!(
        reviewer.tools.as_deref(),
        Some(["read_file", "grep"].map(str::to_string).as_slice())
    );

    // external agents
    let peer = dto
        .external_agent("peer_acp")
        .expect("external_agents.peer_acp");
    assert_eq!(peer.kind.as_deref(), Some("acp"));
    assert_eq!(
        peer.command.as_deref(),
        Some(["peer-agent", "--acp"].map(str::to_string).as_slice())
    );

    // tools
    let shell = dto.tool("shell").expect("tools.shell");
    assert_eq!(shell.approval.as_deref(), Some("ask"));

    // session
    let session = dto.session.as_ref().expect("[session]");
    assert_eq!(session.routing.as_deref(), Some("model_routed"));
    assert_eq!(
        session.budget,
        Some(BudgetDto {
            max_tokens: Some(200_000),
            ..BudgetDto::default()
        })
    );
}

#[test]
fn example_toml_round_trips_semantically() {
    let dto = ConfigDto::parse_str(EXAMPLE_TOML).expect("parse example");
    let serialized = dto.to_string_pretty().expect("serialize");
    let reparsed = ConfigDto::parse_str(&serialized).expect("re-parse serialized output");
    assert_eq!(
        dto, reparsed,
        "DTO -> TOML -> DTO must be semantically equal"
    );
}

#[test]
fn serialized_output_keeps_secret_references_unmaterialized() {
    let dto = ConfigDto::parse_str(EXAMPLE_TOML).expect("parse example");
    let serialized = dto.to_string_pretty().expect("serialize");
    assert!(serialized.contains("[providers.anthropic]"));
    assert!(serialized.contains("[agents.reviewer]"));
    assert!(serialized.contains("[external_agents.peer_acp]"));
    assert!(serialized.contains("[tools.shell]"));
    assert!(serialized.contains("[session]"));
    // References stay references in the persisted form.
    assert!(serialized.contains("env = \"ANTHROPIC_API_KEY\""));
    assert!(serialized.contains("keyring = \"mag/local_proxy\""));
}

#[test]
fn example_dto_round_trips_through_json_for_gui_sources() {
    // Decision D4: the same DTO serves non-TOML sources (GUI patches use JSON).
    let dto = ConfigDto::parse_str(EXAMPLE_TOML).expect("parse example");
    let json = serde_json::to_string(&dto).expect("serialize to JSON");
    let back: ConfigDto = serde_json::from_str(&json).expect("deserialize from JSON");
    assert_eq!(dto, back);
}

#[test]
fn empty_document_parses_to_default() {
    let dto = ConfigDto::parse_str("").expect("empty document parses");
    assert_eq!(dto, ConfigDto::default());
    assert!(dto.is_empty());
    dto.validate().expect("empty config is valid");
    let serialized = dto.to_string_pretty().expect("serialize default");
    assert!(serialized.trim().is_empty());
}

#[test]
fn partial_config_is_legal() {
    // A fragment setting only some sections must parse and validate.
    let dto = ConfigDto::parse_str(
        r#"
[session]
routing = "model_routed"

[tools.shell]
approval = "deny"
"#,
    )
    .expect("partial config parses");
    assert!(dto.provider("anthropic").is_none());
    assert_eq!(dto.tool("shell").unwrap().approval.as_deref(), Some("deny"));
    assert_eq!(
        dto.session.as_ref().unwrap().routing.as_deref(),
        Some("model_routed")
    );
    dto.validate().expect("partial config validates");
    // And round-trips without growing phantom sections.
    let reparsed = ConfigDto::parse_str(&dto.to_string_pretty().unwrap()).expect("re-parse");
    assert_eq!(dto, reparsed);
}
