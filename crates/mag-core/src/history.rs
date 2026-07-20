//! Projection from persisted agent snapshots into service history entries.

use std::collections::{HashMap, HashSet};

use agent_lib::{
    conversation::{Conversation, ConversationMessage, MessageId, ToolPairing},
    facade::AgentSnapshot,
    model::{
        content::ContentBlock,
        message::{Message, Role},
        tool::ToolStatus,
    },
};
use mag_service::{
    DelegationStatusWire, DelegationTrace, HistoryEntry, MessageAttachment, ServiceError,
    ToolCallIdWire, ToolStatusWire, ToolTrace,
};
use serde_json::Value;

#[derive(Clone, Debug)]
struct ToolUseRecord {
    name: String,
    input: Value,
}

/// Reconstructs committed service history from an agent snapshot.
pub(crate) fn entries_from_snapshot(
    snapshot: &AgentSnapshot,
) -> Result<Vec<HistoryEntry>, ServiceError> {
    let conversation = Conversation::restore(snapshot.supervisor.clone()).map_err(|error| {
        ServiceError::Backend {
            message: format!("restore session history snapshot: {error}"),
        }
    })?;
    let delegates = delegate_names(snapshot);
    let mut entries = Vec::new();

    for turn in conversation.turns() {
        let tool_uses = tool_uses_by_provider_id(turn.messages());
        for message in turn.messages() {
            append_message_entries(
                message,
                turn.pairings(),
                &tool_uses,
                &delegates,
                &mut entries,
            )?;
        }
    }

    Ok(entries)
}

fn delegate_names(snapshot: &AgentSnapshot) -> HashSet<String> {
    snapshot
        .delegates
        .iter()
        .map(|delegate| delegate.name.clone())
        .chain(
            snapshot
                .external_delegates
                .iter()
                .map(|delegate| delegate.name.clone()),
        )
        .collect()
}

fn tool_uses_by_provider_id(messages: &[ConversationMessage]) -> HashMap<String, ToolUseRecord> {
    let mut uses = HashMap::new();
    for message in messages {
        if message.payload().role != Role::Assistant {
            continue;
        }
        for block in &message.payload().content {
            if let ContentBlock::ToolUse {
                id, name, input, ..
            } = block
            {
                uses.insert(
                    id.clone(),
                    ToolUseRecord {
                        name: name.clone(),
                        input: input.clone(),
                    },
                );
            }
        }
    }
    uses
}

fn append_message_entries(
    message: &ConversationMessage,
    pairings: &[ToolPairing],
    tool_uses: &HashMap<String, ToolUseRecord>,
    delegates: &HashSet<String>,
    entries: &mut Vec<HistoryEntry>,
) -> Result<(), ServiceError> {
    match message.payload().role {
        Role::User => entries.push(HistoryEntry::UserMessage {
            text: message_text(message.payload()),
            attachments: Vec::<MessageAttachment>::new(),
        }),
        Role::Assistant => {
            let text = message_text(message.payload());
            if !text.is_empty() {
                entries.push(HistoryEntry::AssistantMessage { text });
            }
        }
        Role::Tool => append_tool_results(message, pairings, tool_uses, delegates, entries)?,
        Role::System => {}
    }
    Ok(())
}

fn append_tool_results(
    message: &ConversationMessage,
    pairings: &[ToolPairing],
    tool_uses: &HashMap<String, ToolUseRecord>,
    delegates: &HashSet<String>,
    entries: &mut Vec<HistoryEntry>,
) -> Result<(), ServiceError> {
    for block in &message.payload().content {
        let ContentBlock::ToolResult {
            tool_use_id,
            content,
            status,
            ..
        } = block
        else {
            continue;
        };

        let pairing = pairing_for_result(pairings, message.id(), tool_use_id).ok_or_else(|| {
            ServiceError::Backend {
                message: format!(
                    "session history tool result `{tool_use_id}` has no committed pairing"
                ),
            }
        })?;
        let tool_use = tool_uses
            .get(tool_use_id)
            .ok_or_else(|| ServiceError::Backend {
                message: format!(
                    "session history tool result `{tool_use_id}` has no matching tool use"
                ),
            })?;

        if let Some(delegate) = delegation_name(&tool_use.name, delegates) {
            entries.push(HistoryEntry::Delegation {
                trace: delegation_trace(&delegate, &tool_use.input, content, *status),
            });
        } else {
            entries.push(HistoryEntry::ToolCall {
                trace: tool_trace(pairing, tool_use, content, *status)?,
            });
        }
    }

    Ok(())
}

fn pairing_for_result<'a>(
    pairings: &'a [ToolPairing],
    result_msg: MessageId,
    provider_call_id: &str,
) -> Option<&'a ToolPairing> {
    pairings
        .iter()
        .find(|pairing| {
            pairing.result_msg() == result_msg
                && pairing.provider_call_id() == Some(provider_call_id)
        })
        .or_else(|| {
            let mut candidates = pairings
                .iter()
                .filter(|pairing| pairing.result_msg() == result_msg);
            let first = candidates.next()?;
            candidates.next().is_none().then_some(first)
        })
}

fn delegation_name(tool_name: &str, delegates: &HashSet<String>) -> Option<String> {
    let delegate = tool_name.strip_prefix("ask_")?;
    delegates.contains(delegate).then(|| delegate.to_owned())
}

fn tool_trace(
    pairing: &ToolPairing,
    tool_use: &ToolUseRecord,
    output: &[ContentBlock],
    status: ToolStatus,
) -> Result<ToolTrace, ServiceError> {
    Ok(ToolTrace {
        run_id: None,
        call_id: ToolCallIdWire::new(pairing.call_id().into_uuid()),
        name: tool_use.name.clone(),
        input: Some(tool_use.input.clone()),
        output: tool_output(output)?,
        status: tool_status(status),
        message: tool_status_message(status, output),
    })
}

fn delegation_trace(
    delegate: &str,
    input: &Value,
    output: &[ContentBlock],
    status: ToolStatus,
) -> DelegationTrace {
    let text = text_from_blocks(output);
    DelegationTrace {
        run_id: None,
        delegate: delegate.to_owned(),
        status: delegation_status(status),
        task: input.get("task").and_then(Value::as_str).map(str::to_owned),
        output: (status == ToolStatus::Ok && !text.is_empty()).then_some(text.clone()),
        message: (status != ToolStatus::Ok && !text.is_empty()).then_some(text),
        usage: None,
    }
}

fn tool_output(output: &[ContentBlock]) -> Result<Option<Value>, ServiceError> {
    if output.is_empty() {
        return Ok(None);
    }
    serde_json::to_value(output)
        .map(Some)
        .map_err(|error| ServiceError::Backend {
            message: format!("serialize session history tool output: {error}"),
        })
}

fn tool_status(status: ToolStatus) -> ToolStatusWire {
    match status {
        ToolStatus::Ok => ToolStatusWire::Finished,
        ToolStatus::Error => ToolStatusWire::Failed,
        ToolStatus::Denied => ToolStatusWire::Denied,
        ToolStatus::Cancelled => ToolStatusWire::Cancelled,
    }
}

fn delegation_status(status: ToolStatus) -> DelegationStatusWire {
    match status {
        ToolStatus::Ok => DelegationStatusWire::Finished,
        ToolStatus::Error | ToolStatus::Denied | ToolStatus::Cancelled => {
            DelegationStatusWire::Failed
        }
    }
}

fn tool_status_message(status: ToolStatus, output: &[ContentBlock]) -> Option<String> {
    if status == ToolStatus::Ok {
        return None;
    }
    let text = text_from_blocks(output);
    (!text.is_empty()).then_some(text)
}

fn message_text(message: &Message) -> String {
    text_from_blocks(&message.content)
}

fn text_from_blocks(blocks: &[ContentBlock]) -> String {
    let mut text = String::new();
    push_text_blocks(blocks, &mut text);
    text
}

fn push_text_blocks(blocks: &[ContentBlock], text: &mut String) {
    for block in blocks {
        match block {
            ContentBlock::Text { text: chunk, .. } => text.push_str(chunk),
            ContentBlock::ToolResult { content, .. } => push_text_blocks(content, text),
            ContentBlock::Image { .. }
            | ContentBlock::ToolUse { .. }
            | ContentBlock::Thinking { .. }
            | ContentBlock::Unknown { .. } => {}
        }
    }
}
