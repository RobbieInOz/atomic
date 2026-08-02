//! Chat / Conversation routes

use crate::db_extractor::{job_scope, Db};
use crate::error::{ok_or_error, ApiErrorResponse};
use crate::event_bridge::chat_event_callback;
use crate::event_channel::EventChannel;
use crate::state::AppState;
use actix_web::{web, HttpRequest, HttpResponse};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Deserialize, Serialize, ToSchema)]
pub struct CreateConversationBody {
    /// Tag IDs to scope the conversation
    #[serde(default)]
    pub tag_ids: Vec<String>,
    /// Optional conversation title
    pub title: Option<String>,
}

#[utoipa::path(post, path = "/api/conversations", request_body = CreateConversationBody, responses((status = 201, description = "Created conversation", body = atomic_core::ConversationWithTags)), tag = "chat")]
pub async fn create_conversation(db: Db, body: web::Json<CreateConversationBody>) -> HttpResponse {
    let req = body.into_inner();
    match db
        .0
        .create_conversation(&req.tag_ids, req.title.as_deref())
        .await
    {
        Ok(conv) => HttpResponse::Created().json(conv),
        Err(e) => crate::error::error_response(e),
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct GetConversationsQuery {
    /// Filter by tag ID
    pub filter_tag_id: Option<String>,
    /// Max results (default: 50)
    pub limit: Option<i32>,
    /// Offset for pagination
    pub offset: Option<i32>,
    /// Include archived conversations (default: false)
    pub include_archived: Option<bool>,
}

#[utoipa::path(get, path = "/api/conversations", params(GetConversationsQuery), responses((status = 200, description = "List of conversations", body = Vec<atomic_core::ConversationWithTags>)), tag = "chat")]
pub async fn get_conversations(db: Db, query: web::Query<GetConversationsQuery>) -> HttpResponse {
    let limit = query.limit.unwrap_or(50);
    let offset = query.offset.unwrap_or(0);
    let include_archived = query.include_archived.unwrap_or(false);
    ok_or_error(
        db.0.get_conversations(
            query.filter_tag_id.as_deref(),
            limit,
            offset,
            include_archived,
        )
        .await,
    )
}

#[utoipa::path(get, path = "/api/conversations/{id}", params(("id" = String, Path, description = "Conversation ID")), responses((status = 200, description = "Conversation with messages", body = atomic_core::ConversationWithMessages), (status = 404, description = "Not found", body = ApiErrorResponse)), tag = "chat")]
pub async fn get_conversation(db: Db, path: web::Path<String>) -> HttpResponse {
    let id = path.into_inner();
    match db.0.get_conversation(&id).await {
        Ok(Some(conv)) => HttpResponse::Ok().json(conv),
        Ok(None) => {
            HttpResponse::NotFound().json(serde_json::json!({"error": "Conversation not found"}))
        }
        Err(e) => crate::error::error_response(e),
    }
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct UpdateConversationBody {
    /// Updated title
    pub title: Option<String>,
    /// Archive/unarchive
    pub is_archived: Option<bool>,
}

#[utoipa::path(put, path = "/api/conversations/{id}", params(("id" = String, Path, description = "Conversation ID")), request_body = UpdateConversationBody, responses((status = 200, description = "Updated conversation")), tag = "chat")]
pub async fn update_conversation(
    db: Db,
    path: web::Path<String>,
    body: web::Json<UpdateConversationBody>,
) -> HttpResponse {
    let id = path.into_inner();
    let req = body.into_inner();
    ok_or_error(
        db.0.update_conversation(&id, req.title.as_deref(), req.is_archived)
            .await,
    )
}

#[utoipa::path(delete, path = "/api/conversations/{id}", params(("id" = String, Path, description = "Conversation ID")), responses((status = 200, description = "Conversation deleted")), tag = "chat")]
pub async fn delete_conversation(db: Db, path: web::Path<String>) -> HttpResponse {
    let id = path.into_inner();
    ok_or_error(db.0.delete_conversation(&id).await)
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct SetScopeBody {
    /// Tag IDs for the conversation scope
    #[serde(default)]
    pub tag_ids: Vec<String>,
}

#[utoipa::path(put, path = "/api/conversations/{id}/scope", params(("id" = String, Path, description = "Conversation ID")), request_body = SetScopeBody, responses((status = 200, description = "Scope updated")), tag = "chat")]
pub async fn set_conversation_scope(
    db: Db,
    path: web::Path<String>,
    body: web::Json<SetScopeBody>,
) -> HttpResponse {
    let id = path.into_inner();
    let tag_ids = body.into_inner().tag_ids;
    ok_or_error(db.0.set_conversation_scope(&id, &tag_ids).await)
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct AddTagBody {
    /// Tag ID to add to scope
    pub tag_id: String,
}

#[utoipa::path(post, path = "/api/conversations/{id}/scope/tags", params(("id" = String, Path, description = "Conversation ID")), request_body = AddTagBody, responses((status = 200, description = "Tag added to scope")), tag = "chat")]
pub async fn add_tag_to_scope(
    db: Db,
    path: web::Path<String>,
    body: web::Json<AddTagBody>,
) -> HttpResponse {
    let id = path.into_inner();
    let tag_id = body.into_inner().tag_id;
    ok_or_error(db.0.add_tag_to_scope(&id, &tag_id).await)
}

#[utoipa::path(delete, path = "/api/conversations/{id}/scope/tags/{tag_id}", params(("id" = String, Path, description = "Conversation ID"), ("tag_id" = String, Path, description = "Tag ID")), responses((status = 200, description = "Tag removed from scope")), tag = "chat")]
pub async fn remove_tag_from_scope(db: Db, path: web::Path<(String, String)>) -> HttpResponse {
    let (id, tag_id) = path.into_inner();
    ok_or_error(db.0.remove_tag_from_scope(&id, &tag_id).await)
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct SendMessageBody {
    /// Message content
    pub content: String,
    /// Optional canvas context for canvas-aware chat tools
    #[serde(default)]
    pub canvas_context: Option<atomic_core::CanvasContext>,
    /// Optional current UI context for page-aware chat tools
    #[serde(default)]
    pub page_context: Option<atomic_core::PageContext>,
}

#[utoipa::path(post, path = "/api/conversations/{id}/messages", params(("id" = String, Path, description = "Conversation ID")), request_body = SendMessageBody, responses((status = 200, description = "Assistant response (streaming events via WebSocket)", body = atomic_core::ChatMessageWithContext)), tag = "chat")]
pub async fn send_chat_message(
    req: HttpRequest,
    state: web::Data<AppState>,
    events: EventChannel,
    db: Db,
    path: web::Path<String>,
    body: web::Json<SendMessageBody>,
) -> HttpResponse {
    let conversation_id = path.into_inner();
    let body = body.into_inner();
    let events_tx = events.0.clone();
    let on_event = chat_event_callback(events_tx.clone());

    // Registered for the whole turn; the guard deregisters on every exit.
    let turn = state
        .chat_cancellations
        .begin(job_scope(&req), &conversation_id);

    let result = if body.canvas_context.is_some() || body.page_context.is_some() {
        db.0.send_chat_message_with_canvas(
            &conversation_id,
            &body.content,
            on_event,
            body.canvas_context,
            body.page_context,
            Some(turn.flag()),
        )
        .await
    } else {
        db.0.send_chat_message(&conversation_id, &body.content, on_event, Some(turn.flag()))
            .await
    };

    match result {
        Ok(message) => HttpResponse::Ok().json(message),
        Err(e) => {
            // The client watches WebSocket events during the turn and may
            // never read this body (navigation, disconnect, a proxy timing
            // out the long request) — put the failure on the event stream too
            // so the conversation surfaces it either way.
            let _ = events_tx.send(crate::state::ServerEvent::ChatError {
                conversation_id: conversation_id.clone(),
                error: e.to_string(),
            });
            crate::error::error_response(e)
        }
    }
}

#[utoipa::path(post, path = "/api/conversations/{id}/messages/cancel", params(("id" = String, Path, description = "Conversation ID")), responses((status = 202, description = "Cancellation requested")), tag = "chat")]
pub async fn cancel_chat_message(
    req: HttpRequest,
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> HttpResponse {
    let conversation_id = path.into_inner();
    let scope = job_scope(&req);
    // Idempotent: nothing running is a valid state to ask for a stop from,
    // and the caller's next move (clear the streaming UI) is the same either
    // way. `cancelled` reports whether a turn was actually signalled.
    let cancelled = state
        .chat_cancellations
        .cancel(scope.as_deref(), &conversation_id);
    HttpResponse::Accepted().json(serde_json::json!({ "cancelled": cancelled }))
}
