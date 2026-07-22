//! DTO↔DO conversion tests: resolve validation, round-trip equivalence, and
//! snapshot isolation (`docs/CLI.md` §4.2, decision D4).

use std::sync::Arc;

use mag_config::{
    ApprovalPolicyKind, ConfigDto, ConfigError, ConfigSnapshot, ExternalAgentKind, ProviderWire,
    RoutingModeKind, SecretRef,
};

/// The §4.2 example, copied character-for-character from `docs/CLI.md` (same
/// fixture as `tests/roundtrip.rs`; integration test files cannot share
/// items, so the constant is duplicated deliberately).
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
default_subagent_tools = ["read_file", "list_dir", "grep"]  # 可选：定义未设 tools 的 subagent 缺省工具面（dyn-agents.md §7）
"#;

/// Extracts the `(path, message)` of a `ConfigError::Validation`.
fn validation_path(error: &ConfigError) -> (&str, &str) {
    match error {
        ConfigError::Validation { path, message } => (path.as_str(), message.as_str()),
        other => panic!("expected ConfigError::Validation, got {other:?}"),
    }
}

#[test]
fn example_toml_resolves_into_shared_arc_graph() {
    let dto = ConfigDto::parse_str(EXAMPLE_TOML).expect("§4.2 example must parse");
    let snapshot = ConfigSnapshot::resolve(&dto, 7).expect("§4.2 example must resolve");

    assert_eq!(snapshot.revision(), 7);

    // Provider nodes: enums parsed, secret references intact.
    let anthropic = snapshot.provider("anthropic").expect("providers.anthropic");
    assert_eq!(anthropic.wire(), ProviderWire::Anthropic);
    assert_eq!(anthropic.base_url(), Some("https://api.anthropic.com"));
    assert_eq!(
        anthropic.api_key(),
        Some(&SecretRef::env("ANTHROPIC_API_KEY"))
    );
    let local_proxy = snapshot
        .provider("local_proxy")
        .expect("providers.local_proxy");
    assert_eq!(local_proxy.wire(), ProviderWire::OpenAi);
    assert_eq!(
        local_proxy.api_key(),
        Some(&SecretRef::keyring("mag/local_proxy"))
    );

    // Cross references resolve to the *same* Arc nodes (object tree/forest).
    let default_agent = snapshot.agent("default").expect("agents.default");
    assert!(
        Arc::ptr_eq(default_agent.provider().expect("agent provider"), anthropic),
        "agent provider must share the provider node's Arc"
    );
    let reviewer = snapshot.agent("reviewer").expect("agents.reviewer");
    assert!(Arc::ptr_eq(
        reviewer.provider().expect("agent provider"),
        local_proxy
    ));
    assert_eq!(reviewer.model(), Some("gpt-5-codex"));

    // Explicit tool override is shared with the snapshot's tools map.
    let shell = snapshot.tool("shell").expect("tools.shell");
    assert_eq!(shell.approval(), Some(ApprovalPolicyKind::Ask));
    assert!(shell.is_enabled());
    let agent_tool_names: Vec<&str> = default_agent.tools().iter().map(|t| t.name()).collect();
    assert_eq!(
        agent_tool_names,
        ["read_file", "list_dir", "grep", "shell", "ask_user"]
    );
    let shell_ref = default_agent
        .tools()
        .iter()
        .find(|t| t.name() == "shell")
        .expect("shell in agent tools");
    assert!(
        Arc::ptr_eq(shell_ref, shell),
        "agent tool ref must share the explicit override's Arc"
    );

    // Tools without a [tools.<name>] entry resolve to shared default nodes:
    // enabled, no approval override, and one Arc per name across agents.
    let agent_read = default_agent
        .tools()
        .iter()
        .find(|t| t.name() == "read_file")
        .expect("read_file in agent tools");
    assert_eq!(agent_read.approval(), None);
    assert!(agent_read.is_enabled());
    let reviewer_read = reviewer
        .tools()
        .iter()
        .find(|t| t.name() == "read_file")
        .expect("read_file in reviewer tools");
    assert!(
        Arc::ptr_eq(agent_read, reviewer_read),
        "implicit tool nodes must be shared per name"
    );
    // Implicit nodes never leak into the explicit tools map.
    assert!(snapshot.tool("read_file").is_none());

    // External agent, session, approval nodes.
    let peer = snapshot
        .external_agent("peer_acp")
        .expect("external_agents.peer_acp");
    assert_eq!(peer.kind(), Some(ExternalAgentKind::Acp));
    assert_eq!(peer.command(), ["peer-agent", "--acp"]);
    assert_eq!(
        snapshot.session_defaults().routing(),
        Some(RoutingModeKind::ModelRouted)
    );
    assert_eq!(
        snapshot
            .session_defaults()
            .budget()
            .and_then(|b| b.max_tokens()),
        Some(200000)
    );
    assert_eq!(
        snapshot.session_defaults().default_subagent_tools(),
        Some(
            ["read_file", "list_dir", "grep"]
                .map(str::to_string)
                .as_slice()
        )
    );
    // No [approval] section: effective default matches agent-lib's default tier.
    assert_eq!(snapshot.approval().default_policy(), None);
    assert_eq!(
        snapshot.approval().effective_default_policy(),
        ApprovalPolicyKind::Allow
    );
}

#[test]
fn dto_do_dto_round_trip_is_lossless() {
    let dto = ConfigDto::parse_str(EXAMPLE_TOML).expect("§4.2 example must parse");
    let snapshot = ConfigSnapshot::resolve(&dto, 1).expect("resolve");
    let projected = snapshot.project();
    assert_eq!(projected, dto, "DTO→DO→DTO must be lossless");

    // The projected DTO still serializes with secret *references*, and the
    // serialized form parses back to the same DTO.
    let toml = projected
        .to_string_pretty()
        .expect("serialize projected DTO");
    assert!(toml.contains("env = \"ANTHROPIC_API_KEY\""));
    assert!(toml.contains("keyring = \"mag/local_proxy\""));
    let reparsed = ConfigDto::parse_str(&toml).expect("reparse projected TOML");
    assert_eq!(reparsed, dto);
}

#[test]
fn session_default_agent_resolves_round_trips_and_must_exist() {
    let dto = ConfigDto::parse_str(
        r#"
[agents.default]
model = "model-d"

[agents.reviewer]
model = "model-r"

[session]
default_agent = "reviewer"
"#,
    )
    .expect("parse");
    let snapshot = ConfigSnapshot::resolve(&dto, 1).expect("resolve");
    assert_eq!(
        snapshot.session_defaults().default_agent(),
        Some("reviewer")
    );
    assert_eq!(snapshot.project(), dto, "DTO→DO→DTO must be lossless");

    // A dangling default_agent reference is rejected like a dangling
    // agents.<name>.provider reference.
    let dto = ConfigDto::parse_str(
        r#"
[agents.default]

[session]
default_agent = "ghost"
"#,
    )
    .expect("parse");
    let error = ConfigSnapshot::resolve(&dto, 1).expect_err("dangling default_agent must fail");
    let (path, message) = validation_path(&error);
    assert_eq!(path, "session.default_agent");
    assert!(message.contains("ghost"), "message: {message}");
}

/// `[session].default_subagent_tools` is a plain name list: resolve keeps it
/// verbatim (unknown-name validation is deferred to surface-build time,
/// `docs/dyn-agents.md` §7) and `project()` round-trips it losslessly.
#[test]
fn session_default_subagent_tools_resolves_and_round_trips() {
    let dto = ConfigDto::parse_str(
        r#"
[session]
default_subagent_tools = ["read_file", "ghost"]
"#,
    )
    .expect("parse");
    let snapshot = ConfigSnapshot::resolve(&dto, 1).expect("resolve");
    assert_eq!(
        snapshot.session_defaults().default_subagent_tools(),
        Some(["read_file", "ghost"].map(str::to_string).as_slice()),
        "even an unknown name passes resolve; it is dropped with a warning at surface build"
    );
    assert_eq!(snapshot.project(), dto, "DTO→DO→DTO must be lossless");

    // An unset key stays unset through resolve and projection.
    let dto = ConfigDto::parse_str("[session]\nrouting = \"model_routed\"\n").expect("parse");
    let snapshot = ConfigSnapshot::resolve(&dto, 1).expect("resolve");
    assert_eq!(snapshot.session_defaults().default_subagent_tools(), None);
    assert_eq!(snapshot.project(), dto);
}

/// `[agents.<name>].allow_subagents` resolves verbatim (absent → `None`,
/// i.e. the default `true` at definition-build time) and `project()`
/// round-trips it losslessly (`docs/dyn-agents.md` §5.4).
#[test]
fn agent_allow_subagents_resolves_and_round_trips() {
    let dto = ConfigDto::parse_str(
        r#"
[agents.leaf]
allow_subagents = false

[agents.plain]
"#,
    )
    .expect("parse");
    let snapshot = ConfigSnapshot::resolve(&dto, 1).expect("resolve");
    assert_eq!(
        snapshot
            .agent("leaf")
            .expect("agents.leaf")
            .allow_subagents(),
        Some(false)
    );
    assert_eq!(
        snapshot
            .agent("plain")
            .expect("agents.plain")
            .allow_subagents(),
        None
    );
    assert_eq!(snapshot.project(), dto, "DTO→DO→DTO must be lossless");
}

#[test]
fn explicit_empty_tool_list_is_distinct_from_no_tool_list() {
    let dto = ConfigDto::parse_str(
        r#"
[agents.bare]

[agents.empty]
tools = []
"#,
    )
    .expect("parse");
    let snapshot = ConfigSnapshot::resolve(&dto, 1).expect("resolve");

    // No `tools` key: unconstrained (`None`); the slice accessor serves empty.
    let bare = snapshot.agent("bare").expect("agents.bare");
    assert_eq!(bare.tools_list(), None);
    assert!(bare.tools().is_empty());

    // Explicit `tools = []`: a real constraint to *no* tools.
    let empty = snapshot.agent("empty").expect("agents.empty");
    assert_eq!(empty.tools_list(), Some(&[][..]));

    // The distinction survives projection (lossless round-trip).
    let projected = snapshot.project();
    assert_eq!(projected.agent("bare").and_then(|a| a.tools.clone()), None);
    assert_eq!(
        projected.agent("empty").and_then(|a| a.tools.clone()),
        Some(Vec::new())
    );
    assert_eq!(projected, dto);
}

#[test]
fn dangling_provider_reference_reports_field_path() {
    let mut dto = ConfigDto::default();
    dto.agents.insert(
        "reviewer".to_string(),
        mag_config::AgentDto {
            provider: Some("local_poxy".to_string()),
            ..Default::default()
        },
    );
    let error = ConfigSnapshot::resolve(&dto, 1).expect_err("dangling provider must fail");
    let (path, message) = validation_path(&error);
    assert_eq!(path, "agents.reviewer.provider");
    assert!(message.contains("local_poxy"), "message: {message}");
    assert!(message.contains("unknown provider"), "message: {message}");
}

#[test]
fn invalid_enum_values_report_field_paths() {
    // [tools.<name>].approval
    let mut dto = ConfigDto::default();
    dto.tools.insert(
        "shell".to_string(),
        mag_config::ToolDto {
            approval: Some("maybe".to_string()),
            ..Default::default()
        },
    );
    let error = ConfigSnapshot::resolve(&dto, 1).expect_err("bad tool approval must fail");
    let (path, message) = validation_path(&error);
    assert_eq!(path, "tools.shell.approval");
    assert!(message.contains("maybe"), "message: {message}");

    // [approval].default_policy
    let dto = ConfigDto {
        approval: Some(mag_config::ApprovalSectionDto {
            default_policy: Some("yolo".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let error = ConfigSnapshot::resolve(&dto, 1).expect_err("bad default_policy must fail");
    let (path, message) = validation_path(&error);
    assert_eq!(path, "approval.default_policy");
    assert!(message.contains("yolo"), "message: {message}");

    // [session].routing
    let dto = ConfigDto {
        session: Some(mag_config::SessionDefaultsDto {
            routing: Some("random".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let error = ConfigSnapshot::resolve(&dto, 1).expect_err("bad routing must fail");
    let (path, message) = validation_path(&error);
    assert_eq!(path, "session.routing");
    assert!(message.contains("random"), "message: {message}");

    // [external_agents.<name>].kind
    let mut dto = ConfigDto::default();
    dto.external_agents.insert(
        "peer".to_string(),
        mag_config::ExternalAgentDto {
            kind: Some("mcp".to_string()),
            ..Default::default()
        },
    );
    let error = ConfigSnapshot::resolve(&dto, 1).expect_err("bad kind must fail");
    let (path, message) = validation_path(&error);
    assert_eq!(path, "external_agents.peer.kind");
    assert!(message.contains("mcp"), "message: {message}");
}

#[test]
fn provider_wire_is_required_and_validated() {
    // Missing wire.
    let mut dto = ConfigDto::default();
    dto.providers.insert(
        "x".to_string(),
        mag_config::ProviderDto {
            base_url: Some("https://example.com".to_string()),
            ..Default::default()
        },
    );
    let error = ConfigSnapshot::resolve(&dto, 1).expect_err("missing wire must fail");
    let (path, message) = validation_path(&error);
    assert_eq!(path, "providers.x.wire");
    assert!(
        message.contains("missing wire protocol"),
        "message: {message}"
    );

    // Unknown wire.
    let mut dto = ConfigDto::default();
    dto.providers.insert(
        "x".to_string(),
        mag_config::ProviderDto {
            wire: Some("grok".to_string()),
            ..Default::default()
        },
    );
    let error = ConfigSnapshot::resolve(&dto, 1).expect_err("unknown wire must fail");
    let (path, message) = validation_path(&error);
    assert_eq!(path, "providers.x.wire");
    assert!(message.contains("grok"), "message: {message}");
}

#[test]
fn structural_validation_runs_before_semantic_resolve() {
    let mut dto = ConfigDto::default();
    dto.providers.insert(
        "anthropic".to_string(),
        mag_config::ProviderDto {
            wire: Some("anthropic".to_string()),
            ..Default::default()
        },
    );
    dto.agents.insert(
        "default".to_string(),
        mag_config::AgentDto {
            provider: Some("anthropic".to_string()),
            model: Some("   ".to_string()),
            ..Default::default()
        },
    );
    let error = ConfigSnapshot::resolve(&dto, 1).expect_err("structural error must fail resolve");
    let (path, _) = validation_path(&error);
    assert_eq!(path, "agents.default.model");
}

#[test]
fn empty_dto_resolves_to_empty_snapshot_with_defaults() {
    let dto = ConfigDto::default();
    let snapshot = ConfigSnapshot::resolve(&dto, 42).expect("empty DTO must resolve");

    assert_eq!(snapshot.revision(), 42);
    assert!(snapshot.providers().is_empty());
    assert!(snapshot.agents().is_empty());
    assert!(snapshot.external_agents().is_empty());
    assert!(snapshot.tools().is_empty());
    // Default-filled accessors still serve usable values.
    assert_eq!(
        snapshot.session_defaults().effective_routing(),
        RoutingModeKind::ModelRouted
    );
    assert_eq!(
        snapshot.approval().effective_default_policy(),
        ApprovalPolicyKind::Allow
    );
    // Lossless projection of the empty graph.
    assert_eq!(snapshot.project(), ConfigDto::default());
}

#[test]
fn defaults_fill_through_effective_accessors_without_ghost_projection() {
    let toml = r#"
[agents.default]
tools = ["read_file"]

[external_agents.peer]
command = ["peer-agent"]

[tools.off]
enabled = false
"#;
    let dto = ConfigDto::parse_str(toml).expect("parse");
    let snapshot = ConfigSnapshot::resolve(&dto, 1).expect("resolve");

    // Implicit tool node: enabled by default, no approval override.
    let agent = snapshot.agent("default").expect("agents.default");
    assert!(agent.provider().is_none());
    let read_file = &agent.tools()[0];
    assert_eq!(read_file.enabled(), None, "raw field stays unset");
    assert!(read_file.is_enabled(), "effective default is enabled");

    // Explicit enabled=false round-trips as false.
    let off = snapshot.tool("off").expect("tools.off");
    assert_eq!(off.enabled(), Some(false));
    assert!(!off.is_enabled());

    // External agent kind defaults to acp through the effective accessor.
    let peer = snapshot
        .external_agent("peer")
        .expect("external_agents.peer");
    assert_eq!(peer.kind(), None, "raw field stays unset");
    assert_eq!(peer.effective_kind(), ExternalAgentKind::Acp);

    // Nothing defaulted was materialized back into the DTO.
    assert_eq!(snapshot.project(), dto);
}

#[test]
fn snapshot_isolation_keeps_pinned_snapshot_unchanged() {
    let dto_v1 = ConfigDto::parse_str(EXAMPLE_TOML).expect("parse v1");
    let pinned = Arc::new(ConfigSnapshot::resolve(&dto_v1, 1).expect("resolve v1"));
    // A session pins the snapshot by cloning the Arc (cheap handle copy).
    let session_handle = Arc::clone(&pinned);

    // A later update produces a *new* snapshot at a bumped revision.
    let mut dto_v2 = dto_v1.clone();
    dto_v2
        .providers
        .get_mut("anthropic")
        .expect("providers.anthropic")
        .base_url = Some("https://proxy.example.com".to_string());
    dto_v2
        .agents
        .get_mut("default")
        .expect("agents.default")
        .model = Some("claude-opus-4-8".to_string());
    dto_v2.tools.insert(
        "shell".to_string(),
        mag_config::ToolDto {
            approval: Some("deny".to_string()),
            ..Default::default()
        },
    );
    let updated = ConfigSnapshot::resolve(&dto_v2, 2).expect("resolve v2");

    // New snapshot carries the new values.
    assert_eq!(updated.revision(), 2);
    assert_eq!(
        updated.provider("anthropic").expect("provider").base_url(),
        Some("https://proxy.example.com")
    );
    assert_eq!(
        updated.agent("default").expect("agent").model(),
        Some("claude-opus-4-8")
    );
    assert_eq!(
        updated.tool("shell").expect("tool").approval(),
        Some(ApprovalPolicyKind::Deny)
    );

    // The pinned snapshot observed by the live session is unchanged.
    assert_eq!(session_handle.revision(), 1);
    assert_eq!(
        session_handle
            .provider("anthropic")
            .expect("provider")
            .base_url(),
        Some("https://api.anthropic.com")
    );
    assert_eq!(
        session_handle.agent("default").expect("agent").model(),
        Some("claude-sonnet-4-5")
    );
    assert_eq!(
        session_handle.tool("shell").expect("tool").approval(),
        Some(ApprovalPolicyKind::Ask)
    );
    // Whole-tree clone is handle-cheap and still points at the same nodes.
    let cloned = session_handle.as_ref().clone();
    assert!(Arc::ptr_eq(
        cloned.provider("anthropic").expect("provider"),
        session_handle.provider("anthropic").expect("provider")
    ));
}
