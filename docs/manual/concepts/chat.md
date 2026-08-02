---
title: Chat
description: Converse with your knowledge base using agentic RAG and scoped retrieval.
---

Chat is an agentic RAG system that lets you ask questions grounded in your Atomic knowledge base.

## How It Works

The chat agent has tools to search and read your knowledge base during conversation. When you ask a question:

1. The agent decides whether to search your notes.
2. It formulates search queries and retrieves relevant chunks.
3. It synthesizes an answer grounded in retrieved content.
4. Responses stream back in real time over WebSocket events.

Chat can emit tool-start and tool-complete events, citations, and canvas actions. The REST call that sends a message returns the final assistant message, while the UI updates from streaming events as the model responds.

## Tools

Every turn has the same core tools — your atoms, the synthesized layers above them, and two write tools:

| Tool | What It Does |
|------|--------------|
| `search_atoms` | Hybrid keyword + semantic search over the atoms in scope. Optional `since_days` recency filter for time-sensitive questions |
| `get_atom` | Read one atom's full content by ID, paginated by line (500 at a time by default) |
| `list_tags` | The hierarchical tag tree with atom counts (counts include sub-tags) — the map of how your knowledge is organized, and the source of `tag_id` values for other tools. Returns the top of the tree and one level below it; `parent_id` expands a branch further |
| `get_wiki` | Read the wiki article for a tag by name or ID — the synthesized overview of everything stored under that topic, instead of stitching many search results together. Paginated like `get_atom` |
| `list_reports` | The scheduled research reports configured in this database, each with its most recent finding |
| `get_finding` | Read one report finding in full, with the report it came from. Paginated like `get_atom` |
| `create_atom` | Create a new atom — only when you explicitly ask for one |
| `edit_atom` | Targeted markdown edits to an existing atom: `replace`, `insert_after`, `append`, `replace_all`. Also only on request |

More tools appear conditionally: `get_current_page_context` when the app sent page context (see below), and `zoom_to_cluster` / `focus_atom` when you are chatting over the canvas.

## Citations

The Sources list under an answer holds only the sources the answer actually cited. Every tool result the agent sees is numbered, and the `[N]` markers in the finished text decide which of those numbered sources are stored — material the agent read but never cited leaves no trace, and a number the model invents is dropped. A source surfaced more than once in a turn keeps its first number, so `[2]` means the same thing everywhere in an answer.

Citations are typed by what they point at:

- **Atom** — from `search_atoms`, `get_atom`, or an atom the agent just created or edited.
- **Wiki article** — from `get_wiki`, labeled with the tag whose article it is.
- **Finding** — from `get_finding`.

Clicking a `[N]` marker, or a chip in the Sources list, opens a popover with the cited excerpt and a link to the source. Atoms and wiki articles open scrolled to that excerpt; findings open in the finding reader, at the top.

Wiki articles and findings carry `[N]` markers of their own, pointing at *their* sources. Those are stripped before the text reaches the model, so every number in an answer resolves against this turn's sources. Over the API, a citation row carries `source_type` (`atom`, `wiki`, or `finding`), a `source_title` for wiki citations, and an `atom_id` that is read according to the source type — the tag ID for a wiki article, the atom ID otherwise.

## Scoped Conversations

Conversations can be scoped to specific tags, giving you focused answers about a particular topic. Each scope tag plays one of three roles, and clicking a tag chip in the chat header cycles between them:

- **Include** — a result may carry any of the included tags. This is the default, and a scope of includes alone is a plain "any of these topics" filter.
- **Require** — every result must carry this tag. Use it to intersect: include *Rust*, require *Reading List*.
- **Exclude** — no result may carry this tag, even if another rule would have admitted it.

An atom is in scope when it satisfies all three rules at once. With nothing included, the base set is your whole knowledge base and the remaining rules narrow it — so an exclude-only scope means "everything except this". A tag always covers its child tags.

The scope is enforced by search itself, not by asking the model to behave: the assistant is told what the scope is, and cannot widen it. Reading a specific atom, wiki article, or report finding by id is deliberately unscoped — the scope shapes what the agent can *find*, not what you can point it at.

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
- `POST /api/conversations/{id}/scope/tags`
- `DELETE /api/conversations/{id}/scope/tags/{tag_id}`
- `POST /api/conversations/{id}/messages`
- `POST /api/conversations/{id}/messages/cancel`

Scope entries carry their mode. `PUT .../scope` takes `{"tag_ids": [{"tag_id": "...", "mode": "require"}, ...]}`, and a bare tag id in that array still means include, so clients written before modes existed keep working. `POST .../scope/tags` takes `{"tag_id": "...", "mode": "exclude"}` (mode optional, defaults to include) and re-posting a tag already in scope changes its mode rather than failing.

Streaming event names exposed to the frontend include `chat-stream-delta`, `chat-tool-start`, `chat-tool-complete`, `chat-complete`, `chat-canvas-action`, `chat-conversation-updated` (an auto-generated title landed), and `chat-error`. Each `chat-stream-delta` carries one incremental chunk of assistant text — clients append them; `chat-complete` carries the authoritative full message.

Cancelling is cooperative and idempotent: the endpoint answers `202` whether or not a turn was running, and a running turn stops at its next checkpoint, persisting the partial answer with a trailing `*(stopped)*` marker.

## Provider Notes

Chat requires an LLM provider and model that can handle the conversation and tool-use workload. If chat fails but embeddings work, check the chat model setting separately from the embedding model.

## Related

- [AI Providers](/getting-started/ai-providers/)
- [Tags](/concepts/tags/)
- [Wiki Synthesis](/concepts/wiki-synthesis/)
- [Reports](/concepts/reports/)
- [MCP Server](/guides/mcp-server/)
