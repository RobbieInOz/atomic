use super::PostgresStorage;
use crate::chat::truncate_preview;
use crate::error::AtomicCoreError;
use crate::models::*;
use crate::search::ScopeFilter;
use crate::storage::traits::*;
use async_trait::async_trait;
use std::collections::HashMap;

/// Helper: fetch conversation tags for a single conversation.
async fn fetch_conversation_tags(
    pool: &sqlx::PgPool,
    conversation_id: &str,
    db_id: &str,
) -> StorageResult<Vec<ScopeTag>> {
    let rows = sqlx::query_as::<_, (String, String, Option<String>, String, bool, String, String)>(
        "SELECT t.id, t.name, t.parent_id, t.created_at, t.is_autotag_target, t.autotag_description, ct.mode
         FROM tags t
         JOIN conversation_tags ct ON ct.tag_id = t.id
         WHERE ct.conversation_id = $1 AND ct.db_id = $2 AND t.db_id = $2
         ORDER BY t.name",
    )
    .bind(conversation_id)
    .bind(db_id)
    .fetch_all(pool)
    .await
    .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;

    Ok(rows
        .into_iter()
        .map(
            |(id, name, parent_id, created_at, is_autotag_target, autotag_description, mode)| {
                ScopeTag {
                    tag: Tag {
                        id,
                        name,
                        parent_id,
                        created_at,
                        is_autotag_target,
                        autotag_description,
                    },
                    mode: ScopeMode::from_db(&mode),
                }
            },
        )
        .collect())
}

/// Helper: fetch conversation summary (message_count, last_message_preview).
async fn fetch_conversation_summary(
    pool: &sqlx::PgPool,
    conversation_id: &str,
    db_id: &str,
) -> StorageResult<(i32, Option<String>)> {
    let message_count: Option<i64> = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT COUNT(*) FROM chat_messages WHERE conversation_id = $1 AND db_id = $2",
    )
    .bind(conversation_id)
    .bind(db_id)
    .fetch_one(pool)
    .await
    .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;
    let message_count = message_count.unwrap_or(0);

    let last_message_preview: Option<String> = sqlx::query_scalar(
        "SELECT content FROM chat_messages
         WHERE conversation_id = $1 AND db_id = $2
         ORDER BY message_index DESC
         LIMIT 1",
    )
    .bind(conversation_id)
    .bind(db_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?
    .flatten();

    let preview = last_message_preview.map(|content| truncate_preview(&content, 100));

    Ok((message_count as i32, preview))
}

/// Helper: load a conversation row and build ConversationWithTags.
async fn load_conversation_with_tags(
    pool: &sqlx::PgPool,
    conversation_id: &str,
    db_id: &str,
) -> StorageResult<ConversationWithTags> {
    let row = sqlx::query_as::<_, (String, Option<String>, String, String, i32)>(
        "SELECT id, title, created_at, updated_at, is_archived
         FROM conversations WHERE id = $1 AND db_id = $2",
    )
    .bind(conversation_id)
    .bind(db_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?
    .ok_or_else(|| {
        AtomicCoreError::NotFound(format!("Conversation not found: {}", conversation_id))
    })?;

    let conversation = Conversation {
        id: row.0,
        title: row.1,
        created_at: row.2,
        updated_at: row.3,
        is_archived: row.4 != 0,
    };

    let tags = fetch_conversation_tags(pool, conversation_id, db_id).await?;
    let (message_count, last_message_preview) =
        fetch_conversation_summary(pool, conversation_id, db_id).await?;

    Ok(ConversationWithTags {
        conversation,
        tags,
        message_count,
        last_message_preview,
    })
}

/// Helper: batch fetch tags for multiple conversations.
async fn batch_fetch_conversation_tags(
    pool: &sqlx::PgPool,
    conv_ids: &[String],
    db_id: &str,
) -> StorageResult<HashMap<String, Vec<ScopeTag>>> {
    if conv_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let rows = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            Option<String>,
            String,
            bool,
            String,
            String,
        ),
    >(
        "SELECT ct.conversation_id, t.id, t.name, t.parent_id, t.created_at, t.is_autotag_target, t.autotag_description, ct.mode
         FROM conversation_tags ct
         JOIN tags t ON ct.tag_id = t.id
         WHERE ct.conversation_id = ANY($1) AND ct.db_id = $2 AND t.db_id = $2
         ORDER BY t.name",
    )
    .bind(conv_ids)
    .bind(db_id)
    .fetch_all(pool)
    .await
    .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;

    let mut map: HashMap<String, Vec<ScopeTag>> = HashMap::new();
    for (conv_id, id, name, parent_id, created_at, is_autotag_target, autotag_description, mode) in
        rows
    {
        map.entry(conv_id).or_default().push(ScopeTag {
            tag: Tag {
                id,
                name,
                parent_id,
                created_at,
                is_autotag_target,
                autotag_description,
            },
            mode: ScopeMode::from_db(&mode),
        });
    }

    Ok(map)
}

/// Helper: batch fetch summaries for multiple conversations.
async fn batch_fetch_conversation_summaries(
    pool: &sqlx::PgPool,
    conv_ids: &[String],
    db_id: &str,
) -> StorageResult<HashMap<String, (i32, Option<String>)>> {
    if conv_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let mut map: HashMap<String, (i32, Option<String>)> = HashMap::new();

    // Get counts
    let count_rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT conversation_id, COUNT(*)
         FROM chat_messages
         WHERE conversation_id = ANY($1) AND db_id = $2
         GROUP BY conversation_id",
    )
    .bind(conv_ids)
    .bind(db_id)
    .fetch_all(pool)
    .await
    .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;

    for (conv_id, count) in count_rows {
        map.insert(conv_id, (count as i32, None));
    }

    // Get last message previews
    let preview_rows = sqlx::query_as::<_, (String, String)>(
        "SELECT conversation_id, content FROM (
            SELECT conversation_id, content,
                   ROW_NUMBER() OVER (PARTITION BY conversation_id ORDER BY message_index DESC) as rn
            FROM chat_messages
            WHERE conversation_id = ANY($1) AND db_id = $2
        ) sub WHERE rn = 1",
    )
    .bind(conv_ids)
    .bind(db_id)
    .fetch_all(pool)
    .await
    .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;

    for (conv_id, content) in preview_rows {
        let preview = truncate_preview(&content, 100);
        map.entry(conv_id).or_insert((0, None)).1 = Some(preview);
    }

    // Ensure all conv_ids have entries
    for id in conv_ids {
        map.entry(id.clone()).or_insert((0, None));
    }

    Ok(map)
}

#[async_trait]
impl ChatStore for PostgresStorage {
    async fn create_conversation(
        &self,
        tag_ids: &[String],
        title: Option<&str>,
    ) -> StorageResult<ConversationWithTags> {
        let now = chrono::Utc::now().to_rfc3339();
        let id = uuid::Uuid::new_v4().to_string();

        sqlx::query(
            "INSERT INTO conversations (id, title, created_at, updated_at, is_archived, db_id)
             VALUES ($1, $2, $3, $4, 0, $5)",
        )
        .bind(&id)
        .bind(title)
        .bind(&now)
        .bind(&now)
        .bind(&self.db_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;

        for tag_id in tag_ids {
            sqlx::query(
                "INSERT INTO conversation_tags (conversation_id, tag_id, db_id) VALUES ($1, $2, $3)",
            )
            .bind(&id)
            .bind(tag_id)
            .bind(&self.db_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;
        }

        let tags = fetch_conversation_tags(&self.pool, &id, &self.db_id).await?;

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

    async fn get_conversations(
        &self,
        filter_tag_id: Option<&str>,
        limit: i32,
        offset: i32,
        include_archived: bool,
    ) -> StorageResult<Vec<ConversationWithTags>> {
        let max_archived = i32::from(include_archived);
        let conversations: Vec<Conversation> = if let Some(tag_id) = filter_tag_id {
            let rows = sqlx::query_as::<_, (String, Option<String>, String, String, i32)>(
                "SELECT DISTINCT c.id, c.title, c.created_at, c.updated_at, c.is_archived
                 FROM conversations c
                 JOIN conversation_tags ct ON ct.conversation_id = c.id
                 WHERE ct.tag_id = $1 AND c.is_archived <= $5 AND c.db_id = $4 AND ct.db_id = $4
                 ORDER BY c.updated_at DESC
                 LIMIT $2 OFFSET $3",
            )
            .bind(tag_id)
            .bind(limit)
            .bind(offset)
            .bind(&self.db_id)
            .bind(max_archived)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;

            rows.into_iter()
                .map(
                    |(id, title, created_at, updated_at, is_archived)| Conversation {
                        id,
                        title,
                        created_at,
                        updated_at,
                        is_archived: is_archived != 0,
                    },
                )
                .collect()
        } else {
            let rows = sqlx::query_as::<_, (String, Option<String>, String, String, i32)>(
                "SELECT id, title, created_at, updated_at, is_archived
                 FROM conversations
                 WHERE is_archived <= $4 AND db_id = $3
                 ORDER BY updated_at DESC
                 LIMIT $1 OFFSET $2",
            )
            .bind(limit)
            .bind(offset)
            .bind(&self.db_id)
            .bind(max_archived)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;

            rows.into_iter()
                .map(
                    |(id, title, created_at, updated_at, is_archived)| Conversation {
                        id,
                        title,
                        created_at,
                        updated_at,
                        is_archived: is_archived != 0,
                    },
                )
                .collect()
        };

        if conversations.is_empty() {
            return Ok(Vec::new());
        }

        let conv_ids: Vec<String> = conversations.iter().map(|c| c.id.clone()).collect();
        let tag_map = batch_fetch_conversation_tags(&self.pool, &conv_ids, &self.db_id).await?;
        let summary_map =
            batch_fetch_conversation_summaries(&self.pool, &conv_ids, &self.db_id).await?;

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

    async fn get_conversation(
        &self,
        conversation_id: &str,
    ) -> StorageResult<Option<ConversationWithMessages>> {
        let row = sqlx::query_as::<_, (String, Option<String>, String, String, i32)>(
            "SELECT id, title, created_at, updated_at, is_archived
             FROM conversations WHERE id = $1 AND db_id = $2",
        )
        .bind(conversation_id)
        .bind(&self.db_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;

        let conv = match row {
            Some((id, title, created_at, updated_at, is_archived)) => Conversation {
                id,
                title,
                created_at,
                updated_at,
                is_archived: is_archived != 0,
            },
            None => return Ok(None),
        };

        let tags = fetch_conversation_tags(&self.pool, &conv.id, &self.db_id).await?;

        // Fetch messages
        let msg_rows = sqlx::query_as::<_, (String, String, String, String, String, i32)>(
            "SELECT id, conversation_id, role, content, created_at, message_index
             FROM chat_messages
             WHERE conversation_id = $1 AND db_id = $2
             ORDER BY message_index",
        )
        .bind(&conv.id)
        .bind(&self.db_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;

        let messages: Vec<ChatMessage> = msg_rows
            .into_iter()
            .map(
                |(id, conversation_id, role, content, created_at, message_index)| ChatMessage {
                    id,
                    conversation_id,
                    role,
                    content,
                    created_at,
                    message_index,
                },
            )
            .collect();

        if messages.is_empty() {
            return Ok(Some(ConversationWithMessages {
                conversation: conv,
                tags,
                messages: Vec::new(),
            }));
        }

        let msg_ids: Vec<String> = messages.iter().map(|m| m.id.clone()).collect();

        // Batch fetch tool calls
        let tc_rows = sqlx::query_as::<_, (String, String, String, String, Option<String>, String, String, Option<String>)>(
            "SELECT id, message_id, tool_name, tool_input, tool_result, 'complete', created_at, NULL
             FROM chat_tool_calls
             WHERE message_id = ANY($1) AND db_id = $2
             ORDER BY created_at",
        )
        .bind(&msg_ids)
        .bind(&self.db_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;

        let mut tool_calls_map: HashMap<String, Vec<ChatToolCall>> = HashMap::new();
        for (
            id,
            message_id,
            tool_name,
            tool_input,
            tool_output,
            status,
            created_at,
            completed_at,
        ) in tc_rows
        {
            let tc = ChatToolCall {
                id,
                message_id: message_id.clone(),
                tool_name,
                tool_input: serde_json::from_str(&tool_input).unwrap_or(serde_json::Value::Null),
                tool_output: tool_output
                    .map(|s| serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)),
                status,
                created_at,
                completed_at,
            };
            tool_calls_map.entry(message_id).or_default().push(tc);
        }

        // Batch fetch citations. The wiki citation's tag name is joined
        // rather than stored, so a renamed tag reads back under its
        // current name.
        let cit_rows = sqlx::query_as::<
            _,
            (
                String,
                String,
                i32,
                String,
                Option<i32>,
                String,
                Option<f32>,
                String,
                Option<String>,
            ),
        >(
            "SELECT c.id, c.message_id, c.citation_index, c.atom_id, c.chunk_index,
                    c.excerpt, c.relevance_score, c.source_type, t.name
             FROM chat_citations c
             LEFT JOIN tags t
                    ON t.id = c.atom_id AND t.db_id = c.db_id AND c.source_type = 'wiki'
             WHERE c.message_id = ANY($1) AND c.db_id = $2
             ORDER BY c.citation_index",
        )
        .bind(&msg_ids)
        .bind(&self.db_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;

        let mut citations_map: HashMap<String, Vec<ChatCitation>> = HashMap::new();
        for (
            id,
            message_id,
            citation_index,
            atom_id,
            chunk_index,
            excerpt,
            relevance_score,
            source_type,
            source_title,
        ) in cit_rows
        {
            let cit = ChatCitation {
                id,
                message_id: message_id.clone(),
                citation_index,
                atom_id,
                chunk_index,
                excerpt,
                relevance_score,
                source_type,
                source_title,
            };
            citations_map.entry(message_id).or_default().push(cit);
        }

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

        Ok(Some(ConversationWithMessages {
            conversation: conv,
            tags,
            messages: messages_with_context,
        }))
    }

    async fn update_conversation(
        &self,
        id: &str,
        title: Option<&str>,
        is_archived: Option<bool>,
    ) -> StorageResult<Conversation> {
        let now = chrono::Utc::now().to_rfc3339();

        if let Some(t) = title {
            sqlx::query(
                "UPDATE conversations SET title = $1, updated_at = $2 WHERE id = $3 AND db_id = $4",
            )
            .bind(t)
            .bind(&now)
            .bind(id)
            .bind(&self.db_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;
        }

        if let Some(archived) = is_archived {
            sqlx::query(
                "UPDATE conversations SET is_archived = $1, updated_at = $2 WHERE id = $3 AND db_id = $4",
            )
            .bind(if archived { 1i32 } else { 0i32 })
            .bind(&now)
            .bind(id)
            .bind(&self.db_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;
        }

        let row = sqlx::query_as::<_, (String, Option<String>, String, String, i32)>(
            "SELECT id, title, created_at, updated_at, is_archived
             FROM conversations WHERE id = $1 AND db_id = $2",
        )
        .bind(id)
        .bind(&self.db_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?
        .ok_or_else(|| AtomicCoreError::NotFound(format!("Conversation not found: {}", id)))?;

        Ok(Conversation {
            id: row.0,
            title: row.1,
            created_at: row.2,
            updated_at: row.3,
            is_archived: row.4 != 0,
        })
    }

    async fn delete_conversation(&self, id: &str) -> StorageResult<()> {
        sqlx::query("DELETE FROM conversations WHERE id = $1 AND db_id = $2")
            .bind(id)
            .bind(&self.db_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;
        Ok(())
    }

    async fn set_conversation_scope(
        &self,
        conversation_id: &str,
        entries: &[ScopeEntry],
    ) -> StorageResult<ConversationWithTags> {
        let now = chrono::Utc::now().to_rfc3339();

        sqlx::query("DELETE FROM conversation_tags WHERE conversation_id = $1 AND db_id = $2")
            .bind(conversation_id)
            .bind(&self.db_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;

        for entry in entries {
            sqlx::query(
                "INSERT INTO conversation_tags (conversation_id, tag_id, mode, db_id) VALUES ($1, $2, $3, $4)",
            )
            .bind(conversation_id)
            .bind(&entry.tag_id)
            .bind(entry.mode.as_str())
            .bind(&self.db_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;
        }

        sqlx::query("UPDATE conversations SET updated_at = $1 WHERE id = $2 AND db_id = $3")
            .bind(&now)
            .bind(conversation_id)
            .bind(&self.db_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;

        load_conversation_with_tags(&self.pool, conversation_id, &self.db_id).await
    }

    async fn add_tag_to_scope(
        &self,
        conversation_id: &str,
        tag_id: &str,
        mode: ScopeMode,
    ) -> StorageResult<ConversationWithTags> {
        let now = chrono::Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO conversation_tags (conversation_id, tag_id, mode, db_id) VALUES ($1, $2, $3, $4)
             ON CONFLICT (conversation_id, tag_id) DO UPDATE SET mode = EXCLUDED.mode",
        )
        .bind(conversation_id)
        .bind(tag_id)
        .bind(mode.as_str())
        .bind(&self.db_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;

        sqlx::query("UPDATE conversations SET updated_at = $1 WHERE id = $2 AND db_id = $3")
            .bind(&now)
            .bind(conversation_id)
            .bind(&self.db_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;

        load_conversation_with_tags(&self.pool, conversation_id, &self.db_id).await
    }

    async fn remove_tag_from_scope(
        &self,
        conversation_id: &str,
        tag_id: &str,
    ) -> StorageResult<ConversationWithTags> {
        let now = chrono::Utc::now().to_rfc3339();

        sqlx::query(
            "DELETE FROM conversation_tags WHERE conversation_id = $1 AND tag_id = $2 AND db_id = $3",
        )
        .bind(conversation_id)
        .bind(tag_id)
        .bind(&self.db_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;

        sqlx::query("UPDATE conversations SET updated_at = $1 WHERE id = $2 AND db_id = $3")
            .bind(&now)
            .bind(conversation_id)
            .bind(&self.db_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;

        load_conversation_with_tags(&self.pool, conversation_id, &self.db_id).await
    }

    async fn save_message(
        &self,
        conversation_id: &str,
        role: &str,
        content: &str,
    ) -> StorageResult<ChatMessage> {
        let message_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();

        let message_index: Option<i32> = sqlx::query_scalar::<_, Option<i32>>(
            "SELECT COALESCE(MAX(message_index), -1) + 1 FROM chat_messages WHERE conversation_id = $1 AND db_id = $2",
        )
        .bind(conversation_id)
        .bind(&self.db_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;
        let message_index = message_index.unwrap_or(0);

        sqlx::query(
            "INSERT INTO chat_messages (id, conversation_id, role, content, created_at, message_index, db_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&message_id)
        .bind(conversation_id)
        .bind(role)
        .bind(content)
        .bind(&now)
        .bind(message_index as i32)
        .bind(&self.db_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;

        sqlx::query("UPDATE conversations SET updated_at = $1 WHERE id = $2 AND db_id = $3")
            .bind(&now)
            .bind(conversation_id)
            .bind(&self.db_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;

        Ok(ChatMessage {
            id: message_id,
            conversation_id: conversation_id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            created_at: now,
            message_index: message_index as i32,
        })
    }

    async fn save_tool_calls(
        &self,
        message_id: &str,
        tool_calls: &[ChatToolCall],
    ) -> StorageResult<()> {
        for tool_call in tool_calls {
            let tool_input_str = serde_json::to_string(&tool_call.tool_input).unwrap_or_default();
            let tool_result_str = tool_call
                .tool_output
                .as_ref()
                .map(|v| serde_json::to_string(v).unwrap_or_default())
                .unwrap_or_default();

            sqlx::query(
                "INSERT INTO chat_tool_calls (id, message_id, tool_name, tool_input, tool_result, created_at, db_id)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(&tool_call.id)
            .bind(message_id)
            .bind(&tool_call.tool_name)
            .bind(&tool_input_str)
            .bind(&tool_result_str)
            .bind(&tool_call.created_at)
            .bind(&self.db_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;
        }
        Ok(())
    }

    async fn save_citations(
        &self,
        message_id: &str,
        citations: &[ChatCitation],
    ) -> StorageResult<()> {
        for citation in citations {
            sqlx::query(
                "INSERT INTO chat_citations (id, message_id, citation_index, atom_id, chunk_index, excerpt, relevance_score, source_type, db_id)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            )
            .bind(&citation.id)
            .bind(message_id)
            .bind(citation.citation_index)
            .bind(&citation.atom_id)
            .bind(citation.chunk_index)
            .bind(&citation.excerpt)
            .bind(citation.relevance_score)
            .bind(&citation.source_type)
            .bind(&self.db_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;
        }
        Ok(())
    }

    async fn get_conversation_scope(&self, conversation_id: &str) -> StorageResult<ScopeFilter> {
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT tag_id, mode FROM conversation_tags WHERE conversation_id = $1 AND db_id = $2",
        )
        .bind(conversation_id)
        .bind(&self.db_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;

        let mut scope = ScopeFilter::default();
        for (tag_id, mode) in rows {
            scope.push(tag_id, ScopeMode::from_db(&mode));
        }
        Ok(scope)
    }

    async fn get_scope_description(&self, scope: &ScopeFilter) -> StorageResult<String> {
        let roots = scope.roots();
        if roots.is_empty() {
            return Ok(crate::chat::describe_scope(scope, &HashMap::new()));
        }

        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT id, name FROM tags WHERE id = ANY($1) AND db_id = $2",
        )
        .bind(&roots)
        .bind(&self.db_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AtomicCoreError::DatabaseOperation(e.to_string()))?;

        let names: HashMap<String, String> = rows.into_iter().collect();
        if names.is_empty() {
            Ok("You have access to a scoped set of atoms.".to_string())
        } else {
            Ok(crate::chat::describe_scope(scope, &names))
        }
    }
}
