---
title: Chat
description: Converse with your knowledge base using agentic RAG and scoped retrieval.
---

Chat is an agentic RAG system that lets you ask questions grounded in your Atomic knowledge base.

## How It Works

The chat agent has tools to search your atoms during conversation. When you ask a question:

1. The agent decides whether to search your notes.
2. It formulates search queries and retrieves relevant chunks.
3. It synthesizes an answer grounded in retrieved content.
4. Responses stream back in real time over WebSocket events.

Chat can emit tool-start and tool-complete events, citations, and canvas actions. The REST call that sends a message returns the final assistant message, while the UI updates from streaming events as the model responds.

## Scoped Conversations

Conversations can be scoped to specific tags. When scoped, the agent only searches atoms under those tags, giving you focused answers about a particular topic.

## Conversations

Chat conversations are persisted. You can revisit previous conversations and continue where you left off. Each conversation tracks messages and scoped tags.

Conversations can also be renamed, archived, or deleted through the API/UI. Archived conversations are hidden from the list until you ask for them (`include_archived=true`).

After the first exchange, an untitled conversation names itself: one short completion on the **tagging** model turns the opening question and answer into a title of at most six words. It runs in the background — a failure costs the exchange nothing and simply leaves the conversation untitled — and never overwrites a title that already exists, so a rename is final.

## Page Context

Messages sent from the app carry a small description of what you were looking at (the open atom, the wiki you were reading, the tag filter in effect) so the assistant can answer "this" questions. The composer shows what is attached, and dismissing that chip stops page context from being sent for the rest of the conversation.

## API and Events

The primary endpoints are:

- `POST /api/conversations`
- `GET /api/conversations`
- `GET /api/conversations/{id}`
- `PUT /api/conversations/{id}`
- `DELETE /api/conversations/{id}`
- `PUT /api/conversations/{id}/scope`
- `POST /api/conversations/{id}/messages`
- `POST /api/conversations/{id}/messages/cancel`

Streaming event names exposed to the frontend include `chat-stream-delta`, `chat-tool-start`, `chat-tool-complete`, `chat-complete`, `chat-canvas-action`, `chat-conversation-updated` (an auto-generated title landed), and `chat-error`. Each `chat-stream-delta` carries one incremental chunk of assistant text — clients append them; `chat-complete` carries the authoritative full message.

Cancelling is cooperative and idempotent: the endpoint answers `202` whether or not a turn was running, and a running turn stops at its next checkpoint, persisting the partial answer with a trailing `*(stopped)*` marker.

## Provider Notes

Chat requires an LLM provider and model that can handle the conversation and tool-use workload. If chat fails but embeddings work, check the chat model setting separately from the embedding model.

## Related

- [AI Providers](/getting-started/ai-providers/)
- [Tags](/concepts/tags/)
- [MCP Server](/guides/mcp-server/)
