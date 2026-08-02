//! Chat conversation CRUD operations
//!
//! Provides all conversation, message, tool call, and citation management
//! for the chat system. Used by both Tauri and standalone server.

use crate::error::AtomicCoreError;
use crate::models::{
    ChatCitation, ChatMessage, ChatMessageWithContext, ChatToolCall, Conversation,
    ConversationWithMessages, ConversationWithTags, Tag,
};
use rusqlite::Connection;

// ==================== Helper Functions ====================

/// Truncate a string to at most `max_chars` characters, appending `...` if truncated.
///
/// Operates on character boundaries (not bytes) so it is safe for any UTF-8 input,
/// including multi-byte scripts like Cyrillic, CJK, or emoji.
pub(crate) fn truncate_preview(s: &str, max_chars: usize) -> String {
    let mut iter = s.chars();
    let head: String = iter.by_ref().take(max_chars).collect();
    if iter.next().is_some() {
        format!("{}...", head)
    } else {
        head
    }
}

/// Get tags for a conversation
pub fn get_conversation_tags(
    conn: &Connection,
    conversation_id: &str,
) -> Result<Vec<Tag>, AtomicCoreError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.name, t.parent_id, t.created_at, t.is_autotag_target, t.autotag_description
         FROM tags t
         JOIN conversation_tags ct ON ct.tag_id = t.id
         WHERE ct.conversation_id = ?1
         ORDER BY t.name",
    )?;

    let tags = stmt
        .query_map([conversation_id], |row| {
            Ok(Tag {
                id: row.get(0)?,
                name: row.get(1)?,
                parent_id: row.get(2)?,
                created_at: row.get(3)?,
                is_autotag_target: row.get::<_, i32>(4)? != 0,
                autotag_description: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tags)
}

/// Get message count and last message preview for a conversation
pub fn get_conversation_summary(
    conn: &Connection,
    conversation_id: &str,
) -> Result<(i32, Option<String>), AtomicCoreError> {
    let message_count: i32 = conn.query_row(
        "SELECT COUNT(*) FROM chat_messages WHERE conversation_id = ?1",
        [conversation_id],
        |row| row.get(0),
    )?;

    let last_message_preview: Option<String> = conn
        .query_row(
            "SELECT content FROM chat_messages
             WHERE conversation_id = ?1
             ORDER BY message_index DESC
             LIMIT 1",
            [conversation_id],
            |row| {
                let content: String = row.get(0)?;
                Ok(truncate_preview(&content, 100))
            },
        )
        .ok();

    Ok((message_count, last_message_preview))
}

/// Get tool calls for a message
pub fn get_message_tool_calls(
    conn: &Connection,
    message_id: &str,
) -> Result<Vec<ChatToolCall>, AtomicCoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, message_id, tool_name, tool_input, tool_output, status, created_at, completed_at
         FROM chat_tool_calls
         WHERE message_id = ?1
         ORDER BY created_at",
    )?;

    let tool_calls = stmt
        .query_map([message_id], |row| {
            let tool_input_str: String = row.get(3)?;
            let tool_output_str: Option<String> = row.get(4)?;

            Ok(ChatToolCall {
                id: row.get(0)?,
                message_id: row.get(1)?,
                tool_name: row.get(2)?,
                tool_input: serde_json::from_str(&tool_input_str)
                    .unwrap_or(serde_json::Value::Null),
                tool_output: tool_output_str
                    .map(|s| serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)),
                status: row.get(5)?,
                created_at: row.get(6)?,
                completed_at: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tool_calls)
}

/// Get citations for a message
pub fn get_message_citations(
    conn: &Connection,
    message_id: &str,
) -> Result<Vec<ChatCitation>, AtomicCoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, message_id, citation_index, atom_id, chunk_index, excerpt, relevance_score
         FROM chat_citations
         WHERE message_id = ?1
         ORDER BY citation_index",
    )?;

    let citations = stmt
        .query_map([message_id], |row| {
            Ok(ChatCitation {
                id: row.get(0)?,
                message_id: row.get(1)?,
                citation_index: row.get(2)?,
                atom_id: row.get(3)?,
                chunk_index: row.get(4)?,
                excerpt: row.get(5)?,
                relevance_score: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(citations)
}

/// Get messages with context for a conversation
pub fn get_messages_with_context(
    conn: &Connection,
    conversation_id: &str,
) -> Result<Vec<ChatMessageWithContext>, AtomicCoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, conversation_id, role, content, created_at, message_index
         FROM chat_messages
         WHERE conversation_id = ?1
         ORDER BY message_index",
    )?;

    let messages: Vec<ChatMessage> = stmt
        .query_map([conversation_id], |row| {
            Ok(ChatMessage {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                created_at: row.get(4)?,
                message_index: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    if messages.is_empty() {
        return Ok(Vec::new());
    }

    // Batch fetch tool_calls and citations for all messages (avoids N+1)
    let msg_ids: Vec<String> = messages.iter().map(|m| m.id.clone()).collect();
    let tool_calls_map = batch_fetch_tool_calls(conn, &msg_ids)?;
    let citations_map = batch_fetch_citations(conn, &msg_ids)?;

    let messages_with_context = messages
        .into_iter()
        .map(|message| {
            let tool_calls = tool_calls_map.get(&message.id).cloned().unwrap_or_default();
            let citations = citations_map.get(&message.id).cloned().unwrap_or_default();
            ChatMessageWithContext {
                message,
                tool_calls,
                citations,
            }
        })
        .collect();

    Ok(messages_with_context)
}

/// Batch fetch tool calls for multiple messages in a single query
fn batch_fetch_tool_calls(
    conn: &Connection,
    message_ids: &[String],
) -> Result<std::collections::HashMap<String, Vec<ChatToolCall>>, AtomicCoreError> {
    if message_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let placeholders = message_ids
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let query = format!(
        "SELECT id, message_id, tool_name, tool_input, tool_output, status, created_at, completed_at
         FROM chat_tool_calls
         WHERE message_id IN ({})
         ORDER BY created_at",
        placeholders
    );
    let mut stmt = conn.prepare(&query)?;
    let mut map: std::collections::HashMap<String, Vec<ChatToolCall>> =
        std::collections::HashMap::new();
    let rows = stmt.query_map(rusqlite::params_from_iter(message_ids.iter()), |row| {
        let tool_input_str: String = row.get(3)?;
        let tool_output_str: Option<String> = row.get(4)?;
        Ok(ChatToolCall {
            id: row.get(0)?,
            message_id: row.get(1)?,
            tool_name: row.get(2)?,
            tool_input: serde_json::from_str(&tool_input_str).unwrap_or(serde_json::Value::Null),
            tool_output: tool_output_str
                .map(|s| serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)),
            status: row.get(5)?,
            created_at: row.get(6)?,
            completed_at: row.get(7)?,
        })
    })?;
    for row in rows {
        let tc = row?;
        map.entry(tc.message_id.clone()).or_default().push(tc);
    }
    Ok(map)
}

/// Batch fetch citations for multiple messages in a single query
fn batch_fetch_citations(
    conn: &Connection,
    message_ids: &[String],
) -> Result<std::collections::HashMap<String, Vec<ChatCitation>>, AtomicCoreError> {
    if message_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let placeholders = message_ids
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let query = format!(
        "SELECT id, message_id, citation_index, atom_id, chunk_index, excerpt, relevance_score
         FROM chat_citations
         WHERE message_id IN ({})
         ORDER BY citation_index",
        placeholders
    );
    let mut stmt = conn.prepare(&query)?;
    let mut map: std::collections::HashMap<String, Vec<ChatCitation>> =
        std::collections::HashMap::new();
    let rows = stmt.query_map(rusqlite::params_from_iter(message_ids.iter()), |row| {
        Ok(ChatCitation {
            id: row.get(0)?,
            message_id: row.get(1)?,
            citation_index: row.get(2)?,
            atom_id: row.get(3)?,
            chunk_index: row.get(4)?,
            excerpt: row.get(5)?,
            relevance_score: row.get(6)?,
        })
    })?;
    for row in rows {
        let c = row?;
        map.entry(c.message_id.clone()).or_default().push(c);
    }
    Ok(map)
}

/// Batch fetch tags for multiple conversations in a single query
fn batch_fetch_conversation_tags(
    conn: &Connection,
    conv_ids: &[String],
) -> Result<std::collections::HashMap<String, Vec<Tag>>, AtomicCoreError> {
    if conv_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let placeholders = conv_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let query = format!(
        "SELECT ct.conversation_id, t.id, t.name, t.parent_id, t.created_at, t.is_autotag_target, t.autotag_description
         FROM conversation_tags ct
         JOIN tags t ON ct.tag_id = t.id
         WHERE ct.conversation_id IN ({})
         ORDER BY t.name",
        placeholders
    );
    let mut stmt = conn.prepare(&query)?;
    let mut map: std::collections::HashMap<String, Vec<Tag>> = std::collections::HashMap::new();
    let rows = stmt.query_map(rusqlite::params_from_iter(conv_ids.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            Tag {
                id: row.get(1)?,
                name: row.get(2)?,
                parent_id: row.get(3)?,
                created_at: row.get(4)?,
                is_autotag_target: row.get::<_, i32>(5)? != 0,
                autotag_description: row.get(6)?,
            },
        ))
    })?;
    for row in rows {
        let (conv_id, tag) = row?;
        map.entry(conv_id).or_default().push(tag);
    }
    Ok(map)
}

/// Batch fetch message counts and last message previews for multiple conversations
fn batch_fetch_conversation_summaries(
    conn: &Connection,
    conv_ids: &[String],
) -> Result<std::collections::HashMap<String, (i32, Option<String>)>, AtomicCoreError> {
    if conv_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let placeholders = conv_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");

    // Get counts
    let count_query = format!(
        "SELECT conversation_id, COUNT(*) FROM chat_messages WHERE conversation_id IN ({}) GROUP BY conversation_id",
        placeholders
    );
    let mut count_stmt = conn.prepare(&count_query)?;
    let mut map: std::collections::HashMap<String, (i32, Option<String>)> =
        std::collections::HashMap::new();
    let count_rows = count_stmt.query_map(rusqlite::params_from_iter(conv_ids.iter()), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i32>(1)?))
    })?;
    for row in count_rows {
        let (conv_id, count) = row?;
        map.insert(conv_id, (count, None));
    }

    // Get last message previews using window function
    let preview_query = format!(
        "SELECT conversation_id, content FROM (
            SELECT conversation_id, content,
                   ROW_NUMBER() OVER (PARTITION BY conversation_id ORDER BY message_index DESC) as rn
            FROM chat_messages
            WHERE conversation_id IN ({})
        ) WHERE rn = 1",
        placeholders
    );
    let mut preview_stmt = conn.prepare(&preview_query)?;
    let preview_rows = preview_stmt
        .query_map(rusqlite::params_from_iter(conv_ids.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
    for row in preview_rows {
        let (conv_id, content) = row?;
        let preview = truncate_preview(&content, 100);
        map.entry(conv_id).or_insert((0, None)).1 = Some(preview);
    }

    // Ensure all conv_ids have entries (even those with 0 messages)
    for id in conv_ids {
        map.entry(id.clone()).or_insert((0, None));
    }

    Ok(map)
}

// ==================== CRUD Operations ====================

/// Create a new conversation
pub fn create_conversation(
    conn: &Connection,
    tag_ids: &[String],
    title: Option<&str>,
) -> Result<ConversationWithTags, AtomicCoreError> {
    let now = chrono::Utc::now().to_rfc3339();
    let id = uuid::Uuid::new_v4().to_string();

    conn.execute(
        "INSERT INTO conversations (id, title, created_at, updated_at, is_archived)
         VALUES (?1, ?2, ?3, ?4, 0)",
        rusqlite::params![&id, &title, &now, &now],
    )?;

    for tag_id in tag_ids {
        conn.execute(
            "INSERT INTO conversation_tags (conversation_id, tag_id) VALUES (?1, ?2)",
            rusqlite::params![&id, tag_id],
        )?;
    }

    let tags = get_conversation_tags(conn, &id)?;

    Ok(ConversationWithTags {
        conversation: Conversation {
            id,
            title: title.map(String::from),
            created_at: now.clone(),
            updated_at: now,
            is_archived: false,
        },
        tags,
        message_count: 0,
        last_message_preview: None,
    })
}

/// Get all conversations, optionally filtered by tag. Archived conversations
/// are hidden unless `include_archived` — the comparison against
/// `max_archived` keeps both cases on one query shape.
pub fn get_conversations(
    conn: &Connection,
    filter_tag_id: Option<&str>,
    limit: i32,
    offset: i32,
    include_archived: bool,
) -> Result<Vec<ConversationWithTags>, AtomicCoreError> {
    let max_archived = i32::from(include_archived);
    let conversations: Vec<Conversation> = if let Some(tag_id) = filter_tag_id {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT c.id, c.title, c.created_at, c.updated_at, c.is_archived
             FROM conversations c
             JOIN conversation_tags ct ON ct.conversation_id = c.id
             WHERE ct.tag_id = ?1 AND c.is_archived <= ?4
             ORDER BY c.updated_at DESC
             LIMIT ?2 OFFSET ?3",
        )?;

        let results = stmt
            .query_map(
                rusqlite::params![tag_id, limit, offset, max_archived],
                |row| {
                    Ok(Conversation {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        created_at: row.get(2)?,
                        updated_at: row.get(3)?,
                        is_archived: row.get::<_, i32>(4)? != 0,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        results
    } else {
        let mut stmt = conn.prepare(
            "SELECT id, title, created_at, updated_at, is_archived
             FROM conversations
             WHERE is_archived <= ?3
             ORDER BY updated_at DESC
             LIMIT ?1 OFFSET ?2",
        )?;

        let results = stmt
            .query_map(rusqlite::params![limit, offset, max_archived], |row| {
                Ok(Conversation {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    created_at: row.get(2)?,
                    updated_at: row.get(3)?,
                    is_archived: row.get::<_, i32>(4)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        results
    };

    if conversations.is_empty() {
        return Ok(Vec::new());
    }

    let conv_ids: Vec<String> = conversations.iter().map(|c| c.id.clone()).collect();

    // Batch fetch tags for all conversations
    let tag_map = batch_fetch_conversation_tags(conn, &conv_ids)?;

    // Batch fetch summaries (message count + last preview) for all conversations
    let summary_map = batch_fetch_conversation_summaries(conn, &conv_ids)?;

    let result = conversations
        .into_iter()
        .map(|conversation| {
            let tags = tag_map.get(&conversation.id).cloned().unwrap_or_default();
            let (message_count, last_message_preview) = summary_map
                .get(&conversation.id)
                .cloned()
                .unwrap_or((0, None));
            ConversationWithTags {
                conversation,
                tags,
                message_count,
                last_message_preview,
            }
        })
        .collect();

    Ok(result)
}

/// Get a single conversation with all messages
pub fn get_conversation(
    conn: &Connection,
    conversation_id: &str,
) -> Result<Option<ConversationWithMessages>, AtomicCoreError> {
    let conversation: Option<Conversation> = conn
        .query_row(
            "SELECT id, title, created_at, updated_at, is_archived
             FROM conversations
             WHERE id = ?1",
            [conversation_id],
            |row| {
                Ok(Conversation {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    created_at: row.get(2)?,
                    updated_at: row.get(3)?,
                    is_archived: row.get::<_, i32>(4)? != 0,
                })
            },
        )
        .ok();

    match conversation {
        Some(conv) => {
            let tags = get_conversation_tags(conn, &conv.id)?;
            let messages = get_messages_with_context(conn, &conv.id)?;

            Ok(Some(ConversationWithMessages {
                conversation: conv,
                tags,
                messages,
            }))
        }
        None => Ok(None),
    }
}

/// Update a conversation (title, archive status)
pub fn update_conversation(
    conn: &Connection,
    id: &str,
    title: Option<&str>,
    is_archived: Option<bool>,
) -> Result<Conversation, AtomicCoreError> {
    let now = chrono::Utc::now().to_rfc3339();

    if let Some(t) = title {
        conn.execute(
            "UPDATE conversations SET title = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![t, &now, id],
        )?;
    }

    if let Some(archived) = is_archived {
        conn.execute(
            "UPDATE conversations SET is_archived = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![if archived { 1 } else { 0 }, &now, id],
        )?;
    }

    conn.query_row(
        "SELECT id, title, created_at, updated_at, is_archived
         FROM conversations
         WHERE id = ?1",
        [id],
        |row| {
            Ok(Conversation {
                id: row.get(0)?,
                title: row.get(1)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
                is_archived: row.get::<_, i32>(4)? != 0,
            })
        },
    )
    .map_err(|_| AtomicCoreError::NotFound(format!("Conversation not found: {}", id)))
}

/// Delete a conversation
pub fn delete_conversation(conn: &Connection, id: &str) -> Result<(), AtomicCoreError> {
    conn.execute("DELETE FROM conversations WHERE id = ?1", [id])?;
    Ok(())
}

// ==================== Scope Management ====================

/// Set the full scope (replace all tags)
pub fn set_conversation_scope(
    conn: &Connection,
    conversation_id: &str,
    tag_ids: &[String],
) -> Result<ConversationWithTags, AtomicCoreError> {
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "DELETE FROM conversation_tags WHERE conversation_id = ?1",
        [conversation_id],
    )?;

    for tag_id in tag_ids {
        conn.execute(
            "INSERT INTO conversation_tags (conversation_id, tag_id) VALUES (?1, ?2)",
            rusqlite::params![conversation_id, tag_id],
        )?;
    }

    conn.execute(
        "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![&now, conversation_id],
    )?;

    let conversation = conn
        .query_row(
            "SELECT id, title, created_at, updated_at, is_archived
             FROM conversations WHERE id = ?1",
            [conversation_id],
            |row| {
                Ok(Conversation {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    created_at: row.get(2)?,
                    updated_at: row.get(3)?,
                    is_archived: row.get::<_, i32>(4)? != 0,
                })
            },
        )
        .map_err(|_| {
            AtomicCoreError::NotFound(format!("Conversation not found: {}", conversation_id))
        })?;

    let tags = get_conversation_tags(conn, conversation_id)?;
    let (message_count, last_message_preview) = get_conversation_summary(conn, conversation_id)?;

    Ok(ConversationWithTags {
        conversation,
        tags,
        message_count,
        last_message_preview,
    })
}

/// Add a single tag to scope
pub fn add_tag_to_scope(
    conn: &Connection,
    conversation_id: &str,
    tag_id: &str,
) -> Result<ConversationWithTags, AtomicCoreError> {
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT OR IGNORE INTO conversation_tags (conversation_id, tag_id) VALUES (?1, ?2)",
        rusqlite::params![conversation_id, tag_id],
    )?;

    conn.execute(
        "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![&now, conversation_id],
    )?;

    let conversation = conn
        .query_row(
            "SELECT id, title, created_at, updated_at, is_archived
             FROM conversations WHERE id = ?1",
            [conversation_id],
            |row| {
                Ok(Conversation {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    created_at: row.get(2)?,
                    updated_at: row.get(3)?,
                    is_archived: row.get::<_, i32>(4)? != 0,
                })
            },
        )
        .map_err(|_| {
            AtomicCoreError::NotFound(format!("Conversation not found: {}", conversation_id))
        })?;

    let tags = get_conversation_tags(conn, conversation_id)?;
    let (message_count, last_message_preview) = get_conversation_summary(conn, conversation_id)?;

    Ok(ConversationWithTags {
        conversation,
        tags,
        message_count,
        last_message_preview,
    })
}

/// Remove a single tag from scope
pub fn remove_tag_from_scope(
    conn: &Connection,
    conversation_id: &str,
    tag_id: &str,
) -> Result<ConversationWithTags, AtomicCoreError> {
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "DELETE FROM conversation_tags WHERE conversation_id = ?1 AND tag_id = ?2",
        rusqlite::params![conversation_id, tag_id],
    )?;

    conn.execute(
        "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![&now, conversation_id],
    )?;

    let conversation = conn
        .query_row(
            "SELECT id, title, created_at, updated_at, is_archived
             FROM conversations WHERE id = ?1",
            [conversation_id],
            |row| {
                Ok(Conversation {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    created_at: row.get(2)?,
                    updated_at: row.get(3)?,
                    is_archived: row.get::<_, i32>(4)? != 0,
                })
            },
        )
        .map_err(|_| {
            AtomicCoreError::NotFound(format!("Conversation not found: {}", conversation_id))
        })?;

    let tags = get_conversation_tags(conn, conversation_id)?;
    let (message_count, last_message_preview) = get_conversation_summary(conn, conversation_id)?;

    Ok(ConversationWithTags {
        conversation,
        tags,
        message_count,
        last_message_preview,
    })
}

// ==================== Message DB Operations ====================

/// Save a message and return (message_id, message_index)
pub fn save_message(
    conn: &Connection,
    conversation_id: &str,
    role: &str,
    content: &str,
) -> Result<(String, i32), AtomicCoreError> {
    let message_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    let message_index: i32 = conn.query_row(
        "SELECT COALESCE(MAX(message_index), -1) + 1 FROM chat_messages WHERE conversation_id = ?1",
        [conversation_id],
        |row| row.get(0),
    )?;

    conn.execute(
        "INSERT INTO chat_messages (id, conversation_id, role, content, created_at, message_index)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            &message_id,
            conversation_id,
            role,
            content,
            &now,
            message_index
        ],
    )?;

    conn.execute(
        "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![&now, conversation_id],
    )?;

    Ok((message_id, message_index))
}

/// Save tool calls for a message
pub fn save_tool_calls(
    conn: &Connection,
    message_id: &str,
    tool_calls: &[ChatToolCall],
) -> Result<(), AtomicCoreError> {
    for tool_call in tool_calls {
        conn.execute(
            "INSERT INTO chat_tool_calls (id, message_id, tool_name, tool_input, tool_output, status, created_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                &tool_call.id,
                message_id,
                &tool_call.tool_name,
                serde_json::to_string(&tool_call.tool_input).unwrap_or_default(),
                tool_call.tool_output.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default()),
                &tool_call.status,
                &tool_call.created_at,
                &tool_call.completed_at,
            ],
        )?;
    }
    Ok(())
}

/// Save citations for a message
pub fn save_citations(
    conn: &Connection,
    message_id: &str,
    citations: &[ChatCitation],
) -> Result<(), AtomicCoreError> {
    for citation in citations {
        conn.execute(
            "INSERT INTO chat_citations (id, message_id, citation_index, atom_id, chunk_index, excerpt, relevance_score)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                &citation.id,
                message_id,
                citation.citation_index,
                &citation.atom_id,
                citation.chunk_index,
                &citation.excerpt,
                citation.relevance_score,
            ],
        )?;
    }
    Ok(())
}

/// Get scope tag IDs for a conversation
pub fn get_scope_tag_ids(
    conn: &Connection,
    conversation_id: &str,
) -> Result<Vec<String>, AtomicCoreError> {
    let mut stmt =
        conn.prepare("SELECT tag_id FROM conversation_tags WHERE conversation_id = ?1")?;

    let tag_ids = stmt
        .query_map([conversation_id], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tag_ids)
}

/// Get scope description for system prompt
pub fn get_scope_description(conn: &Connection, tag_ids: &[String]) -> String {
    if tag_ids.is_empty() {
        return "You have access to ALL atoms in the knowledge base.".to_string();
    }

    let placeholders: Vec<String> = (0..tag_ids.len()).map(|i| format!("?{}", i + 1)).collect();
    let query = format!(
        "SELECT name FROM tags WHERE id IN ({})",
        placeholders.join(", ")
    );

    let mut stmt = match conn.prepare(&query) {
        Ok(s) => s,
        Err(_) => return "You have access to a scoped set of atoms.".to_string(),
    };

    let params: Vec<&dyn rusqlite::ToSql> =
        tag_ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let names: Vec<String> = stmt
        .query_map(params.as_slice(), |row| row.get(0))
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();

    if names.is_empty() {
        "You have access to a scoped set of atoms.".to_string()
    } else {
        format!(
            "You have access to atoms tagged with: {}. Focus your search on these topics.",
            names.join(", ")
        )
    }
}

/// Get conversation messages in provider Message format
pub fn get_conversation_messages(
    conn: &Connection,
    conversation_id: &str,
) -> Result<Vec<crate::providers::types::Message>, AtomicCoreError> {
    use crate::providers::types::{Message, MessageRole};

    let mut stmt = conn.prepare(
        "SELECT role, content FROM chat_messages WHERE conversation_id = ?1 ORDER BY message_index",
    )?;

    let messages = stmt
        .query_map([conversation_id], |row| {
            let role_str: String = row.get(0)?;
            let content: Option<String> = row.get(1)?;
            let role = match role_str.as_str() {
                "system" => MessageRole::System,
                "assistant" => MessageRole::Assistant,
                "tool" => MessageRole::Tool,
                _ => MessageRole::User,
            };
            Ok(Message {
                role,
                content,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use tempfile::NamedTempFile;

    fn setup_db() -> (Database, NamedTempFile) {
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open_or_create(temp_file.path()).unwrap();
        (db, temp_file)
    }

    #[test]
    fn test_create_conversation() {
        let (db, _temp) = setup_db();
        let conn = db.conn.lock().unwrap();

        let result = create_conversation(&conn, &[], Some("Test Chat")).unwrap();
        assert_eq!(result.conversation.title, Some("Test Chat".to_string()));
        assert_eq!(result.message_count, 0);
        assert!(result.tags.is_empty());
    }

    #[test]
    fn test_create_conversation_with_tags() {
        let (db, _temp) = setup_db();
        let conn = db.conn.lock().unwrap();

        // Create a tag first
        let tag_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO tags (id, name, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![&tag_id, "TestTag", &now],
        )
        .unwrap();

        let result = create_conversation(&conn, &[tag_id.clone()], Some("Tagged Chat")).unwrap();
        assert_eq!(result.tags.len(), 1);
        assert_eq!(result.tags[0].name, "TestTag");
    }

    #[test]
    fn test_get_conversations() {
        let (db, _temp) = setup_db();
        let conn = db.conn.lock().unwrap();

        create_conversation(&conn, &[], Some("Chat 1")).unwrap();
        create_conversation(&conn, &[], Some("Chat 2")).unwrap();

        let conversations = get_conversations(&conn, None, 10, 0, false).unwrap();
        assert_eq!(conversations.len(), 2);
    }

    #[test]
    fn test_get_conversations_archive_visibility() {
        let (db, _temp) = setup_db();
        let conn = db.conn.lock().unwrap();

        let kept = create_conversation(&conn, &[], Some("Kept")).unwrap();
        let archived = create_conversation(&conn, &[], Some("Archived")).unwrap();
        update_conversation(&conn, &archived.conversation.id, None, Some(true)).unwrap();

        let visible = get_conversations(&conn, None, 10, 0, false).unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].conversation.id, kept.conversation.id);

        let all = get_conversations(&conn, None, 10, 0, true).unwrap();
        assert_eq!(all.len(), 2);
        assert!(all
            .iter()
            .any(|c| c.conversation.id == archived.conversation.id && c.conversation.is_archived));
    }

    #[test]
    fn test_get_conversation_with_messages() {
        let (db, _temp) = setup_db();
        let conn = db.conn.lock().unwrap();

        let conv = create_conversation(&conn, &[], Some("Chat")).unwrap();
        save_message(&conn, &conv.conversation.id, "user", "Hello").unwrap();
        save_message(&conn, &conv.conversation.id, "assistant", "Hi there!").unwrap();

        let result = get_conversation(&conn, &conv.conversation.id).unwrap();
        assert!(result.is_some());
        let conv_with_msgs = result.unwrap();
        assert_eq!(conv_with_msgs.messages.len(), 2);
        assert_eq!(conv_with_msgs.messages[0].message.role, "user");
        assert_eq!(conv_with_msgs.messages[1].message.role, "assistant");
    }

    #[test]
    fn test_update_conversation() {
        let (db, _temp) = setup_db();
        let conn = db.conn.lock().unwrap();

        let conv = create_conversation(&conn, &[], Some("Original")).unwrap();
        let updated =
            update_conversation(&conn, &conv.conversation.id, Some("Updated"), None).unwrap();
        assert_eq!(updated.title, Some("Updated".to_string()));
    }

    #[test]
    fn test_delete_conversation() {
        let (db, _temp) = setup_db();
        let conn = db.conn.lock().unwrap();

        let conv = create_conversation(&conn, &[], Some("ToDelete")).unwrap();
        delete_conversation(&conn, &conv.conversation.id).unwrap();

        let result = get_conversation(&conn, &conv.conversation.id).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_scope_management() {
        let (db, _temp) = setup_db();
        let conn = db.conn.lock().unwrap();

        // Create tags
        let tag1_id = uuid::Uuid::new_v4().to_string();
        let tag2_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO tags (id, name, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![&tag1_id, "Tag1", &now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tags (id, name, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![&tag2_id, "Tag2", &now],
        )
        .unwrap();

        let conv = create_conversation(&conn, &[], None).unwrap();

        // Add tag
        let result = add_tag_to_scope(&conn, &conv.conversation.id, &tag1_id).unwrap();
        assert_eq!(result.tags.len(), 1);

        // Add another tag
        let result = add_tag_to_scope(&conn, &conv.conversation.id, &tag2_id).unwrap();
        assert_eq!(result.tags.len(), 2);

        // Remove tag
        let result = remove_tag_from_scope(&conn, &conv.conversation.id, &tag1_id).unwrap();
        assert_eq!(result.tags.len(), 1);

        // Set scope (replace all)
        let result = set_conversation_scope(
            &conn,
            &conv.conversation.id,
            &[tag1_id.clone(), tag2_id.clone()],
        )
        .unwrap();
        assert_eq!(result.tags.len(), 2);
    }

    #[test]
    fn test_truncate_preview_utf8_safe() {
        // ASCII shorter than limit — returned as-is
        assert_eq!(truncate_preview("hello", 100), "hello");

        // ASCII longer than limit — truncated with ellipsis
        let long_ascii = "a".repeat(150);
        let truncated = truncate_preview(&long_ascii, 100);
        assert_eq!(truncated.len(), 103); // 100 'a' + "..."
        assert!(truncated.ends_with("..."));

        // Cyrillic that would panic with byte slicing at 100 — must not panic
        // and must return a valid UTF-8 string.
        let cyrillic = "Это пример длинного текста на русском языке, который содержит только обычные кириллические символы и несколько предложений подряд. Он нужен только для воспроизведения ошибки.";
        let preview = truncate_preview(cyrillic, 100);
        assert!(preview.ends_with("..."));
        assert_eq!(preview.chars().count(), 103); // 100 chars + "..."

        // Emoji (4-byte UTF-8) — must not panic
        let emoji = "😀".repeat(150);
        let preview = truncate_preview(&emoji, 100);
        assert!(preview.ends_with("..."));
        assert_eq!(preview.chars().count(), 103);
    }

    #[test]
    fn test_get_conversations_with_cyrillic_message() {
        // Regression test for crash on /api/conversations when an assistant message
        // contains long Cyrillic text. The batch summary path used to byte-slice the
        // message content at index 100, which falls inside a multi-byte UTF-8
        // character and panicked.
        let (db, _temp) = setup_db();
        let conn = db.conn.lock().unwrap();

        let conv = create_conversation(&conn, &[], Some("Russian chat")).unwrap();
        save_message(&conn, &conv.conversation.id, "user", "Привет").unwrap();
        save_message(
            &conn,
            &conv.conversation.id,
            "assistant",
            "Это пример длинного текста на русском языке, который содержит только обычные кириллические символы и несколько предложений подряд. Он нужен только для воспроизведения ошибки в обработке preview строки. Если система пытается обрезать такой текст по байтам, а не по корректной UTF-8 границе символа, сервер падает при загрузке списка разговоров.",
        )
        .unwrap();

        // Must not panic.
        let conversations = get_conversations(&conn, None, 10, 0, false).unwrap();
        assert_eq!(conversations.len(), 1);
        let preview = conversations[0]
            .last_message_preview
            .as_ref()
            .expect("preview should be set");
        assert!(preview.ends_with("..."));
        // Preview must be valid UTF-8 (it is by Rust type guarantee, but assert
        // it parses cleanly as Cyrillic content).
        assert!(preview.starts_with("Это пример"));
    }

    #[test]
    fn test_save_message() {
        let (db, _temp) = setup_db();
        let conn = db.conn.lock().unwrap();

        let conv = create_conversation(&conn, &[], None).unwrap();
        let (msg_id, msg_idx) =
            save_message(&conn, &conv.conversation.id, "user", "Hello").unwrap();

        assert!(!msg_id.is_empty());
        assert_eq!(msg_idx, 0);

        let (_, msg_idx2) = save_message(&conn, &conv.conversation.id, "assistant", "Hi!").unwrap();
        assert_eq!(msg_idx2, 1);
    }
}
