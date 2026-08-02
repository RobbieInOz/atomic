//! Auto-generated conversation titles.
//!
//! A conversation is created untitled and the UI falls back to "New
//! Conversation" for every one of them, which makes the list unreadable the
//! moment it holds more than a couple of entries. After a turn completes,
//! [`spawn_generation`] names the conversation from its first exchange.
//!
//! Three properties matter:
//!
//! - **It never affects the turn.** Generation runs in a detached task, so a
//!   slow or dead title model costs the message flow nothing. Every failure
//!   is a log line; the title simply stays `NULL` and the next turn retries.
//! - **It never overwrites a title.** A non-`NULL` title — auto-generated
//!   earlier or typed by the user — ends generation before the model call.
//! - **It rides the tagging model**, not the chat model: naming a
//!   conversation is a cheap utility completion, the same class of work as
//!   auto-tagging, and shouldn't bill at agentic-model rates.

use std::collections::HashMap;

use crate::agent::{ChatEvent, ChatEventCallback};
use crate::models::ChatMessageWithContext;
use crate::providers::traits::LlmConfig;
use crate::providers::types::{GenerationParams, Message};
use crate::providers::{create_llm_provider, ProviderConfig};
use crate::storage::StorageBackend;

/// Words a generated title may keep. Titles are read in a narrow list
/// column, so they earn their space by being scannable, not complete.
const MAX_TITLE_WORDS: usize = 6;

/// Defensive character cap for a model that answers with six very long
/// words. Applied after the word limit.
const MAX_TITLE_CHARS: usize = 60;

/// How much of each message the model gets to name the conversation.
/// The opening exchange establishes the subject; the rest is elaboration
/// we don't need to pay for.
const MAX_EXCERPT_CHARS: usize = 1000;

const TITLE_SYSTEM_PROMPT: &str = "You write a conversation title: at most 6 words \
naming what the exchange is about. Reply with the title alone — no quotes, no \
trailing punctuation, no explanation.";

/// Name `conversation_id` from its first exchange, in a detached task.
///
/// Fire-and-forget by construction: the caller gets no handle and no error.
/// See the module docs for what that buys.
pub(crate) fn spawn_generation(
    storage: StorageBackend,
    settings: HashMap<String, String>,
    conversation_id: String,
    on_event: ChatEventCallback,
) {
    tokio::spawn(async move {
        let title = match generate(&storage, &settings, &conversation_id).await {
            Ok(Some(title)) => title,
            // Already titled, or the model produced nothing usable.
            Ok(None) => return,
            Err(error) => {
                tracing::debug!(
                    conversation_id = %conversation_id,
                    error = %error,
                    "chat: conversation title generation failed; leaving it untitled"
                );
                return;
            }
        };

        match storage
            .update_conversation_sync(&conversation_id, Some(&title), None)
            .await
        {
            Ok(_) => on_event(ChatEvent::ConversationUpdated {
                conversation_id,
                title,
            }),
            Err(error) => tracing::warn!(
                conversation_id = %conversation_id,
                error = %error,
                "chat: failed to persist generated conversation title"
            ),
        }
    });
}

/// `Ok(None)` means "nothing to do" (already titled, no exchange yet, or an
/// unusable answer); `Err` means the attempt itself failed.
async fn generate(
    storage: &StorageBackend,
    settings: &HashMap<String, String>,
    conversation_id: &str,
) -> Result<Option<String>, String> {
    let Some(conversation) = storage
        .get_conversation_sync(conversation_id)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };

    // The only guard that matters: a title, however it got there, is the
    // user's to change from here on.
    if conversation.conversation.title.is_some() {
        return Ok(None);
    }

    let (Some(user), Some(assistant)) = (
        first_message(&conversation.messages, "user"),
        first_message(&conversation.messages, "assistant"),
    ) else {
        return Ok(None);
    };

    let provider_config = ProviderConfig::from_settings(settings);
    let model = provider_config.llm_model().to_string();
    let provider = create_llm_provider(&provider_config).map_err(|e| e.to_string())?;
    let config = LlmConfig::new(&model).with_params(
        GenerationParams::new()
            .with_temperature(0.3)
            .with_max_tokens(32)
            .with_minimize_reasoning(true),
    );
    let messages = vec![
        Message::system(TITLE_SYSTEM_PROMPT),
        Message::user(format!(
            "Message:\n{user}\n\nReply:\n{assistant}\n\nTitle:"
        )),
    ];

    let response = provider
        .complete(&messages, &config)
        .await
        .map_err(|e| e.to_string())?;

    Ok(sanitize(&response.content))
}

fn first_message(messages: &[ChatMessageWithContext], role: &str) -> Option<String> {
    messages
        .iter()
        .find(|m| m.message.role == role && !m.message.content.trim().is_empty())
        .map(|m| truncate_chars(m.message.content.trim(), MAX_EXCERPT_CHARS))
}

/// Reduce a model's answer to something that belongs in a list row, or
/// `None` when there's nothing left worth persisting.
fn sanitize(raw: &str) -> Option<String> {
    let first_line = raw.lines().map(str::trim).find(|line| !line.is_empty())?;
    // Models like to wrap titles in quotes or markdown emphasis, and to end
    // them like sentences. All decoration, none of it wanted in a list row.
    let trimmed = first_line
        .trim_matches(['"', '\'', '“', '”', '‘', '’', '`', '*', '#'])
        .trim_end_matches(['.', ',', ';', ':', '!', '?'])
        .trim();

    let title = trimmed
        .split_whitespace()
        .take(MAX_TITLE_WORDS)
        .collect::<Vec<_>>()
        .join(" ");
    let title = truncate_chars(&title, MAX_TITLE_CHARS);

    (!title.is_empty()).then_some(title)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars()
        .take(max_chars)
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_quotes_and_trailing_punctuation() {
        assert_eq!(
            sanitize("\"Pelican migration notes.\""),
            Some("Pelican migration notes".to_string())
        );
        assert_eq!(
            sanitize("**Rust ownership rules!**"),
            Some("Rust ownership rules".to_string())
        );
    }

    #[test]
    fn keeps_only_the_first_line_and_six_words() {
        assert_eq!(
            sanitize("Seven word titles get cut down here\n\nExplanation the model added"),
            Some("Seven word titles get cut down".to_string())
        );
    }

    #[test]
    fn truncates_absurdly_long_words_on_a_char_boundary() {
        let title = sanitize(&"ü".repeat(200)).expect("a title");
        assert_eq!(title.chars().count(), MAX_TITLE_CHARS);
    }

    #[test]
    fn rejects_answers_with_nothing_left() {
        assert_eq!(sanitize(""), None);
        assert_eq!(sanitize("   \n  "), None);
        assert_eq!(sanitize("\"...\""), None);
    }
}
