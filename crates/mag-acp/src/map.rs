//! Pure, IO-free mappings between ACP protocol types and mag-service wire types.
//!
//! Everything here is a pure function so it can be unit-tested fully offline
//! (`docs/ACP.md` §4). This module currently covers the two directions of the
//! `SessionId` mapping (`docs/ACP.md` §4 "SessionId") and the conservative
//! agent-capability declaration used by `initialize` (`docs/ACP.md` §3.1/§7).

use std::fmt;

use agent_client_protocol::schema::v1 as acp;

/// Converts a mag [`SessionId`](mag_service::SessionId) into an ACP
/// [`SessionId`](acp::SessionId).
///
/// The first version deliberately avoids a bidirectional lookup table by using
/// the string form of the mag `SessionId` (a UUID) directly as the ACP
/// `SessionId` value (`docs/ACP.md` §4).
#[must_use]
pub fn mag_session_id_to_acp(id: mag_service::SessionId) -> acp::SessionId {
    acp::SessionId::new(id.to_string())
}

/// Error returned when an inbound ACP [`SessionId`](acp::SessionId) does not
/// correspond to a valid mag [`SessionId`](mag_service::SessionId).
///
/// A mag `SessionId` is a UUID, so an ACP session id string that does not parse
/// as a UUID is a protocol error. The offending string is preserved so callers
/// can surface a meaningful diagnostic. This local type keeps mag-acp's public
/// API free of the underlying `uuid` crate's error type, preserving the crate's
/// tight dependency boundary (`mag-service` + `agent-client-protocol` only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidSessionId {
    /// The ACP session id string that failed to parse as a mag `SessionId`.
    pub value: String,
}

impl fmt::Display for InvalidSessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid ACP session id (not a valid mag session id): {:?}",
            self.value
        )
    }
}

impl std::error::Error for InvalidSessionId {}

/// Converts an ACP [`SessionId`](acp::SessionId) back into a mag
/// [`SessionId`](mag_service::SessionId).
///
/// This is the inverse of [`mag_session_id_to_acp`]: it parses the ACP session
/// id string as a UUID (`docs/ACP.md` §4).
///
/// # Errors
///
/// Returns [`InvalidSessionId`] when `id` is not a valid mag `SessionId`
/// (i.e. not a UUID string), which the caller should treat as a protocol error.
pub fn acp_session_id_to_mag(
    id: &acp::SessionId,
) -> Result<mag_service::SessionId, InvalidSessionId> {
    let value: &str = id.0.as_ref();
    mag_service::SessionId::parse_str(value).map_err(|_| InvalidSessionId {
        value: value.to_owned(),
    })
}

/// Builds the [`AgentCapabilities`](acp::AgentCapabilities) mag advertises during
/// `initialize`.
///
/// Capabilities are declared honestly and conservatively (`docs/ACP.md`
/// §3.1/§7): only bits mag actually supports are turned on. The first version is
/// text-first and does not yet advertise session resumption:
///
/// - `load_session = false` (revisited in M4-2 once mag-core restore readiness is
///   confirmed);
/// - `prompt_capabilities.image` / `.audio` / `.embedded_context` all `false`
///   (multimodal input is out of scope for the first version);
/// - `mcp_capabilities` / `session_capabilities` / `auth` are left at their
///   conservative defaults (unset), and no `auth_methods` are offered.
#[must_use]
pub fn agent_capabilities() -> acp::AgentCapabilities {
    acp::AgentCapabilities::new()
        .load_session(false)
        .prompt_capabilities(
            acp::PromptCapabilities::new()
                .image(false)
                .audio(false)
                .embedded_context(false),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    // A fixed, valid UUID string so tests can build a mag `SessionId` without
    // pulling in the `uuid` crate (keeps mag-acp's dependency boundary intact).
    const SAMPLE_UUID: &str = "550e8400-e29b-41d4-a716-446655440000";

    #[test]
    fn session_id_round_trips_mag_to_acp_to_mag() {
        let mag_id = mag_service::SessionId::parse_str(SAMPLE_UUID).expect("valid uuid");

        let acp_id = mag_session_id_to_acp(mag_id);
        assert_eq!(acp_id.0.as_ref(), SAMPLE_UUID);

        let back = acp_session_id_to_mag(&acp_id).expect("round-trip parses");
        assert_eq!(back, mag_id);
    }

    #[test]
    fn acp_session_id_to_mag_rejects_non_uuid() {
        let bogus = acp::SessionId::new("not-a-uuid");
        let err = acp_session_id_to_mag(&bogus).expect_err("non-uuid must be rejected");
        assert_eq!(err.value, "not-a-uuid");
        // Display should mention the offending value for diagnostics.
        assert!(err.to_string().contains("not-a-uuid"));
    }

    #[test]
    fn agent_capabilities_are_conservative() {
        let caps = agent_capabilities();
        assert!(!caps.load_session, "load_session must start disabled");
        assert!(!caps.prompt_capabilities.image, "image must be disabled");
        assert!(!caps.prompt_capabilities.audio, "audio must be disabled");
        assert!(
            !caps.prompt_capabilities.embedded_context,
            "embedded_context must be disabled"
        );
    }
}
