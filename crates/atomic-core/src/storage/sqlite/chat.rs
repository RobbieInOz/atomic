use super::SqliteStorage;
use crate::error::AtomicCoreError;
use crate::models::*;
use crate::search::ScopeFilter;
use crate::storage::traits::*;
use async_trait::async_trait;

/// Sync helper methods for chat operations.
impl SqliteStorage {
    pub(crate) fn create_conversation_sync(
        &self,
        tag_ids: &[String],
        title: Option<&str>,
    ) -> StorageResult<ConversationWithTags> {
        let conn = self
            .db
            .conn
            .lock()
            .map_err(|e| AtomicCoreError::Lock(e.to_string()))?;
        crate::chat::create_conversation(&conn, tag_ids, title)
    }

    pub(crate) fn get_conversations_sync(
        &self,
        filter_tag_id: Option<&str>,
        limit: i32,
        offset: i32,
        include_archived: bool,
    ) -> StorageResult<Vec<ConversationWithTags>> {
        let conn = self.db.read_conn()?;
        crate::chat::get_conversations(&conn, filter_tag_id, limit, offset, include_archived)
    }

    pub(crate) fn get_conversation_sync(
        &self,
        conversation_id: &str,
    ) -> StorageResult<Option<ConversationWithMessages>> {
        let conn = self.db.read_conn()?;
        crate::chat::get_conversation(&conn, conversation_id)
    }

    pub(crate) fn update_conversation_sync(
        &self,
        id: &str,
        title: Option<&str>,
        is_archived: Option<bool>,
    ) -> StorageResult<Conversation> {
        let conn = self
            .db
            .conn
            .lock()
            .map_err(|e| AtomicCoreError::Lock(e.to_string()))?;
        crate::chat::update_conversation(&conn, id, title, is_archived)
    }

    pub(crate) fn set_conversation_title_if_unset_sync(
        &self,
        id: &str,
        title: &str,
    ) -> StorageResult<bool> {
        let conn = self
            .db
            .conn
            .lock()
            .map_err(|e| AtomicCoreError::Lock(e.to_string()))?;
        crate::chat::set_title_if_unset(&conn, id, title)
    }

    pub(crate) fn delete_conversation_sync(&self, id: &str) -> StorageResult<()> {
        let conn = self
            .db
            .conn
            .lock()
            .map_err(|e| AtomicCoreError::Lock(e.to_string()))?;
        conn.execute(
            "DELETE FROM chat_messages_fts WHERE conversation_id = ?1",
            [id],
        )?;
        crate::chat::delete_conversation(&conn, id)
    }

    pub(crate) fn set_conversation_scope_sync(
        &self,
        conversation_id: &str,
        entries: &[ScopeEntry],
    ) -> StorageResult<ConversationWithTags> {
        let conn = self
            .db
            .conn
            .lock()
            .map_err(|e| AtomicCoreError::Lock(e.to_string()))?;
        crate::chat::set_conversation_scope(&conn, conversation_id, entries)
    }

    pub(crate) fn add_tag_to_scope_sync(
        &self,
        conversation_id: &str,
        tag_id: &str,
        mode: ScopeMode,
    ) -> StorageResult<ConversationWithTags> {
        let conn = self
            .db
            .conn
            .lock()
            .map_err(|e| AtomicCoreError::Lock(e.to_string()))?;
        crate::chat::add_tag_to_scope(&conn, conversation_id, tag_id, mode)
    }

    pub(crate) fn remove_tag_from_scope_sync(
        &self,
        conversation_id: &str,
        tag_id: &str,
    ) -> StorageResult<ConversationWithTags> {
        let conn = self
            .db
            .conn
            .lock()
            .map_err(|e| AtomicCoreError::Lock(e.to_string()))?;
        crate::chat::remove_tag_from_scope(&conn, conversation_id, tag_id)
    }

    pub(crate) fn save_message_sync(
        &self,
        conversation_id: &str,
        role: &str,
        content: &str,
    ) -> StorageResult<ChatMessage> {
        let conn = self
            .db
            .conn
            .lock()
            .map_err(|e| AtomicCoreError::Lock(e.to_string()))?;
        let (message_id, message_index) =
            crate::chat::save_message(&conn, conversation_id, role, content)?;
        conn.execute(
            "INSERT INTO chat_messages_fts(id, conversation_id, content) VALUES (?1, ?2, ?3)",
            rusqlite::params![&message_id, conversation_id, content],
        )?;
        // Reconstruct ChatMessage from the returned id and index
        Ok(ChatMessage {
            id: message_id,
            conversation_id: conversation_id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            message_index,
        })
    }

    pub(crate) fn save_tool_calls_sync(
        &self,
        message_id: &str,
        tool_calls: &[ChatToolCall],
    ) -> StorageResult<()> {
        let conn = self
            .db
            .conn
            .lock()
            .map_err(|e| AtomicCoreError::Lock(e.to_string()))?;
        crate::chat::save_tool_calls(&conn, message_id, tool_calls)
    }

    pub(crate) fn save_citations_sync(
        &self,
        message_id: &str,
        citations: &[ChatCitation],
    ) -> StorageResult<()> {
        let conn = self
            .db
            .conn
            .lock()
            .map_err(|e| AtomicCoreError::Lock(e.to_string()))?;
        crate::chat::save_citations(&conn, message_id, citations)
    }

    pub(crate) fn get_conversation_scope_sync(
        &self,
        conversation_id: &str,
    ) -> StorageResult<ScopeFilter> {
        let conn = self
            .db
            .conn
            .lock()
            .map_err(|e| AtomicCoreError::Lock(e.to_string()))?;
        crate::chat::get_conversation_scope(&conn, conversation_id)
    }

    pub(crate) fn get_scope_description_sync(&self, scope: &ScopeFilter) -> StorageResult<String> {
        let conn = self
            .db
            .conn
            .lock()
            .map_err(|e| AtomicCoreError::Lock(e.to_string()))?;
        Ok(crate::chat::get_scope_description(&conn, scope))
    }
}

#[async_trait]
impl ChatStore for SqliteStorage {
    async fn create_conversation(
        &self,
        tag_ids: &[String],
        title: Option<&str>,
    ) -> StorageResult<ConversationWithTags> {
        let storage = self.clone();
        let tag_ids = tag_ids.to_vec();
        let title = title.map(|s| s.to_string());
        tokio::task::spawn_blocking(move || {
            storage.create_conversation_sync(&tag_ids, title.as_deref())
        })
        .await
        .map_err(|e| AtomicCoreError::Lock(e.to_string()))?
    }

    async fn get_conversations(
        &self,
        filter_tag_id: Option<&str>,
        limit: i32,
        offset: i32,
        include_archived: bool,
    ) -> StorageResult<Vec<ConversationWithTags>> {
        let storage = self.clone();
        let filter_tag_id = filter_tag_id.map(|s| s.to_string());
        tokio::task::spawn_blocking(move || {
            storage.get_conversations_sync(
                filter_tag_id.as_deref(),
                limit,
                offset,
                include_archived,
            )
        })
        .await
        .map_err(|e| AtomicCoreError::Lock(e.to_string()))?
    }

    async fn get_conversation(
        &self,
        conversation_id: &str,
    ) -> StorageResult<Option<ConversationWithMessages>> {
        let storage = self.clone();
        let conversation_id = conversation_id.to_string();
        tokio::task::spawn_blocking(move || storage.get_conversation_sync(&conversation_id))
            .await
            .map_err(|e| AtomicCoreError::Lock(e.to_string()))?
    }

    async fn update_conversation(
        &self,
        id: &str,
        title: Option<&str>,
        is_archived: Option<bool>,
    ) -> StorageResult<Conversation> {
        let storage = self.clone();
        let id = id.to_string();
        let title = title.map(|s| s.to_string());
        tokio::task::spawn_blocking(move || {
            storage.update_conversation_sync(&id, title.as_deref(), is_archived)
        })
        .await
        .map_err(|e| AtomicCoreError::Lock(e.to_string()))?
    }

    async fn set_conversation_title_if_unset(
        &self,
        id: &str,
        title: &str,
    ) -> StorageResult<bool> {
        let storage = self.clone();
        let id = id.to_string();
        let title = title.to_string();
        tokio::task::spawn_blocking(move || {
            storage.set_conversation_title_if_unset_sync(&id, &title)
        })
        .await
        .map_err(|e| AtomicCoreError::Lock(e.to_string()))?
    }

    async fn delete_conversation(&self, id: &str) -> StorageResult<()> {
        let storage = self.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || storage.delete_conversation_sync(&id))
            .await
            .map_err(|e| AtomicCoreError::Lock(e.to_string()))?
    }

    async fn set_conversation_scope(
        &self,
        conversation_id: &str,
        entries: &[ScopeEntry],
    ) -> StorageResult<ConversationWithTags> {
        let storage = self.clone();
        let conversation_id = conversation_id.to_string();
        let entries = entries.to_vec();
        tokio::task::spawn_blocking(move || {
            storage.set_conversation_scope_sync(&conversation_id, &entries)
        })
        .await
        .map_err(|e| AtomicCoreError::Lock(e.to_string()))?
    }

    async fn add_tag_to_scope(
        &self,
        conversation_id: &str,
        tag_id: &str,
        mode: ScopeMode,
    ) -> StorageResult<ConversationWithTags> {
        let storage = self.clone();
        let conversation_id = conversation_id.to_string();
        let tag_id = tag_id.to_string();
        tokio::task::spawn_blocking(move || {
            storage.add_tag_to_scope_sync(&conversation_id, &tag_id, mode)
        })
        .await
        .map_err(|e| AtomicCoreError::Lock(e.to_string()))?
    }

    async fn remove_tag_from_scope(
        &self,
        conversation_id: &str,
        tag_id: &str,
    ) -> StorageResult<ConversationWithTags> {
        let storage = self.clone();
        let conversation_id = conversation_id.to_string();
        let tag_id = tag_id.to_string();
        tokio::task::spawn_blocking(move || {
            storage.remove_tag_from_scope_sync(&conversation_id, &tag_id)
        })
        .await
        .map_err(|e| AtomicCoreError::Lock(e.to_string()))?
    }

    async fn save_message(
        &self,
        conversation_id: &str,
        role: &str,
        content: &str,
    ) -> StorageResult<ChatMessage> {
        let storage = self.clone();
        let conversation_id = conversation_id.to_string();
        let role = role.to_string();
        let content = content.to_string();
        tokio::task::spawn_blocking(move || {
            storage.save_message_sync(&conversation_id, &role, &content)
        })
        .await
        .map_err(|e| AtomicCoreError::Lock(e.to_string()))?
    }

    async fn save_tool_calls(
        &self,
        message_id: &str,
        tool_calls: &[ChatToolCall],
    ) -> StorageResult<()> {
        let storage = self.clone();
        let message_id = message_id.to_string();
        let tool_calls = tool_calls.to_vec();
        tokio::task::spawn_blocking(move || storage.save_tool_calls_sync(&message_id, &tool_calls))
            .await
            .map_err(|e| AtomicCoreError::Lock(e.to_string()))?
    }

    async fn save_citations(
        &self,
        message_id: &str,
        citations: &[ChatCitation],
    ) -> StorageResult<()> {
        let storage = self.clone();
        let message_id = message_id.to_string();
        let citations = citations.to_vec();
        tokio::task::spawn_blocking(move || storage.save_citations_sync(&message_id, &citations))
            .await
            .map_err(|e| AtomicCoreError::Lock(e.to_string()))?
    }

    async fn get_conversation_scope(&self, conversation_id: &str) -> StorageResult<ScopeFilter> {
        let storage = self.clone();
        let conversation_id = conversation_id.to_string();
        tokio::task::spawn_blocking(move || storage.get_conversation_scope_sync(&conversation_id))
            .await
            .map_err(|e| AtomicCoreError::Lock(e.to_string()))?
    }

    async fn get_scope_description(&self, scope: &ScopeFilter) -> StorageResult<String> {
        let storage = self.clone();
        let scope = scope.clone();
        tokio::task::spawn_blocking(move || storage.get_scope_description_sync(&scope))
            .await
            .map_err(|e| AtomicCoreError::Lock(e.to_string()))?
    }
}
