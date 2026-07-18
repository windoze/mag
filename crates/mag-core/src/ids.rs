//! Monotonic identity source used when wiring agent-lib machines.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use agent_lib::{
    agent::{
        AgentId, RequirementError, RequirementId, RequirementIds, RequirementKindTag, RunId,
        StepId, ToolExecutionIds, ToolRuntimeError, ToolSetId, TraceNodeId,
    },
    conversation::{ConversationId, MessageId, ToolCallId, TurnId},
    model::tool::ToolCall,
};
use uuid::Uuid;

/// Cloneable, per-session monotonic identity source for agent-lib IDs.
///
/// Clones share one counter, so a session can pass this source to the machine,
/// handlers, and future driver code without risking duplicate IDs.
#[derive(Clone, Debug)]
pub struct MagIds {
    counter: Arc<AtomicU64>,
}

impl MagIds {
    /// Creates a fresh identity source whose first generated UUID has value `1`.
    #[must_use]
    pub fn new() -> Self {
        Self::seeded(1)
    }

    /// Creates an identity source whose next generated UUID starts at `start`.
    ///
    /// A zero seed is clamped to `1` so the nil UUID is never produced.
    #[must_use]
    pub fn seeded(start: u64) -> Self {
        Self {
            counter: Arc::new(AtomicU64::new(start.max(1))),
        }
    }

    /// Creates an identity source that continues after a restored high-water mark.
    ///
    /// For example, `continuing_after(42)` makes the next generated UUID wrap
    /// integer value `43`.
    #[must_use]
    pub fn continuing_after(high_water: u64) -> Self {
        Self::seeded(high_water.saturating_add(1))
    }

    fn next_uuid(&self) -> Uuid {
        let value = self
            .counter
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                current.checked_add(1)
            })
            .expect("mag id counter exhausted");
        Uuid::from_u128(u128::from(value))
    }

    /// Mints the next agent identity.
    #[must_use]
    pub fn agent_id(&self) -> AgentId {
        AgentId::new(self.next_uuid())
    }

    /// Mints the next agent run identity.
    #[must_use]
    pub fn run_id(&self) -> RunId {
        RunId::new(self.next_uuid())
    }

    /// Mints the next tool-set identity.
    #[must_use]
    pub fn tool_set_id(&self) -> ToolSetId {
        ToolSetId::new(self.next_uuid())
    }

    /// Mints the next conversation identity.
    #[must_use]
    pub fn conversation_id(&self) -> ConversationId {
        ConversationId::new(self.next_uuid())
    }

    /// Mints the next conversation turn identity.
    #[must_use]
    pub fn turn_id(&self) -> TurnId {
        TurnId::new(self.next_uuid())
    }

    /// Mints the next conversation message identity.
    #[must_use]
    pub fn message_id(&self) -> MessageId {
        MessageId::new(self.next_uuid())
    }

    /// Mints the next framework tool-call identity.
    #[must_use]
    pub fn fresh_tool_call_id(&self) -> ToolCallId {
        ToolCallId::new(self.next_uuid())
    }

    /// Mints the next agent step identity.
    #[must_use]
    pub fn step_id(&self) -> StepId {
        StepId::new(self.next_uuid())
    }

    /// Mints the next trace node identity.
    #[must_use]
    pub fn trace_node_id(&self) -> TraceNodeId {
        TraceNodeId::new(self.next_uuid().to_string())
    }

    /// Mints the root trace node identity for a run.
    #[must_use]
    pub fn trace_root(&self) -> TraceNodeId {
        self.trace_node_id()
    }
}

impl Default for MagIds {
    fn default() -> Self {
        Self::new()
    }
}

impl RequirementIds for MagIds {
    fn next_requirement_id(
        &self,
        _kind_tag: RequirementKindTag,
    ) -> Result<RequirementId, RequirementError> {
        Ok(RequirementId::new(self.next_uuid()))
    }
}

impl ToolExecutionIds for MagIds {
    fn tool_call_id(&self, _call: &ToolCall) -> Result<ToolCallId, ToolRuntimeError> {
        Ok(ToolCallId::new(self.next_uuid()))
    }

    fn tool_result_message_id(
        &self,
        _call_id: ToolCallId,
        _call: &ToolCall,
    ) -> Result<MessageId, ToolRuntimeError> {
        Ok(MessageId::new(self.next_uuid()))
    }

    fn next_assistant_message_id(&self) -> Result<MessageId, ToolRuntimeError> {
        Ok(MessageId::new(self.next_uuid()))
    }

    fn next_step_id(&self) -> Result<StepId, ToolRuntimeError> {
        Ok(StepId::new(self.next_uuid()))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use agent_lib::{
        agent::{RequirementIds, RequirementKindTag, ToolExecutionIds},
        model::tool::ToolCall,
    };
    use serde_json::{Map, Value};

    use super::MagIds;

    fn sample_call() -> ToolCall {
        ToolCall {
            id: "provider-call-1".to_owned(),
            name: "noop".to_owned(),
            input: Value::Object(Map::new()),
        }
    }

    #[test]
    fn ids_are_unique_across_supported_uuid_families() {
        let ids = MagIds::new();
        let call = sample_call();
        let mut seen = HashSet::new();

        let raw = [
            ids.agent_id().into_uuid(),
            ids.run_id().into_uuid(),
            ids.tool_set_id().into_uuid(),
            ids.conversation_id().into_uuid(),
            ids.turn_id().into_uuid(),
            ids.message_id().into_uuid(),
            ids.fresh_tool_call_id().into_uuid(),
            ids.step_id().into_uuid(),
            ids.next_requirement_id(RequirementKindTag::Llm)
                .expect("requirement id")
                .into_uuid(),
            ToolExecutionIds::tool_call_id(&ids, &call)
                .expect("tool call id")
                .into_uuid(),
            ids.tool_result_message_id(ids.fresh_tool_call_id(), &call)
                .expect("tool result message id")
                .into_uuid(),
            ids.next_assistant_message_id()
                .expect("assistant message id")
                .into_uuid(),
            ids.next_step_id().expect("step id").into_uuid(),
        ];

        for id in raw {
            assert_ne!(id, uuid::Uuid::nil());
            assert!(seen.insert(id), "duplicate id minted: {id}");
        }

        let trace_a = ids.trace_node_id();
        let trace_b = ids.trace_node_id();
        assert_ne!(trace_a, trace_b);
    }

    #[test]
    fn clones_share_the_same_counter() {
        let ids = MagIds::new();
        let clone = ids.clone();

        let first = ids.turn_id().into_uuid();
        let second = clone.turn_id().into_uuid();

        assert_ne!(first, second);
    }

    #[test]
    fn continuing_after_resumes_past_the_high_water_mark() {
        let ids = MagIds::continuing_after(42);

        assert_eq!(ids.agent_id().into_uuid(), uuid::Uuid::from_u128(43));
        assert_eq!(ids.run_id().into_uuid(), uuid::Uuid::from_u128(44));
    }

    #[test]
    fn seeded_clamps_zero_to_avoid_nil_uuid() {
        let ids = MagIds::seeded(0);

        assert_eq!(ids.message_id().into_uuid(), uuid::Uuid::from_u128(1));
    }
}
