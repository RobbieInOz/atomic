//! Context-window management for the agent loop.
//!
//! Tool results make a run's prompt grow without bound, so every call is
//! trimmed to what the model can actually hold. Trimming works on groups —
//! an assistant message and the tool results answering it are one unit,
//! because splitting them produces an API-invalid request.

use crate::chunking::count_tokens;
use crate::providers::types::{Message, MessageRole};

/// Estimate token count for a message, including tool call content.
fn estimate_message_tokens(m: &Message) -> usize {
    let content_tokens = count_tokens(m.content.as_deref().unwrap_or(""));
    let tool_call_tokens = m.tool_calls.as_ref().map_or(0, |tcs| {
        tcs.iter()
            .map(|tc| {
                let args = tc.get_arguments().unwrap_or("");
                let name = tc.get_name().unwrap_or("");
                count_tokens(name) + count_tokens(args) + 10
            })
            .sum()
    });
    content_tokens + tool_call_tokens
}

/// A group of messages that must be kept together for API validity.
/// Either a single user/system message, or an assistant message followed
/// by its tool-result messages.
struct MessageGroup {
    start: usize,
    end: usize, // exclusive
    tokens: usize,
}

/// Group messages into atomic units that can't be split.
/// An assistant message with tool_calls and its subsequent tool-result messages
/// form one group. Everything else is its own group.
fn group_messages(messages: &[Message], message_tokens: &[usize]) -> Vec<MessageGroup> {
    let mut groups = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        if messages[i].role == MessageRole::Assistant && messages[i].tool_calls.is_some() {
            // Start of a tool round: assistant + following tool results
            let start = i;
            let mut tokens = message_tokens[i];
            i += 1;
            while i < messages.len() && messages[i].role == MessageRole::Tool {
                tokens += message_tokens[i];
                i += 1;
            }
            groups.push(MessageGroup {
                start,
                end: i,
                tokens,
            });
        } else {
            groups.push(MessageGroup {
                start: i,
                end: i + 1,
                tokens: message_tokens[i],
            });
            i += 1;
        }
    }
    groups
}

/// Truncate message history to fit within the provider's context window.
/// Keeps the system prompt (first group) and the most recent group,
/// then includes as many recent groups as fit in the remaining budget.
/// Never splits assistant+tool-result pairs to maintain API validity.
/// Reserves ~30% of context for the assistant's response and tool results.
///
/// `context_length` of `None` — an unknown model, or a caller that opts out
/// — returns the history untouched.
pub fn truncate_messages_to_context(
    messages: Vec<Message>,
    context_length: Option<usize>,
) -> Vec<Message> {
    let max_tokens = match context_length {
        Some(ctx_len) => (ctx_len as f64 * 0.7) as usize,
        None => return messages,
    };

    if messages.len() <= 2 {
        return messages;
    }

    let message_tokens: Vec<usize> = messages.iter().map(estimate_message_tokens).collect();
    let total: usize = message_tokens.iter().sum();
    if total <= max_tokens {
        return messages;
    }

    let groups = group_messages(&messages, &message_tokens);
    if groups.len() <= 2 {
        return messages; // System + one group, nothing safe to drop
    }

    // Always keep first group (system) and last group (most recent)
    let first_tokens = groups[0].tokens;
    let last_tokens = groups[groups.len() - 1].tokens;
    let mut budget = max_tokens.saturating_sub(first_tokens + last_tokens);

    // Work backwards through middle groups, keeping as many as fit
    let mut keep_from_group = groups.len() - 1;
    for gi in (1..groups.len() - 1).rev() {
        if groups[gi].tokens > budget {
            break;
        }
        budget -= groups[gi].tokens;
        keep_from_group = gi;
    }

    // Build result from kept groups
    let mut result: Vec<Message> = messages[groups[0].start..groups[0].end].to_vec();
    for g in &groups[keep_from_group..] {
        result.extend(messages[g.start..g.end].to_vec());
    }

    tracing::info!(
        original_messages = messages.len(),
        truncated_messages = result.len(),
        groups_kept = groups.len() - keep_from_group + 1,
        max_tokens,
        "[agent_runtime] Truncated message history to fit context window"
    );

    result
}
