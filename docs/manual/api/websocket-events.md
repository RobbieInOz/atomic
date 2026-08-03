---
title: WebSocket Events
description: Subscribe to realtime Atomic server events for pipeline progress, chat streaming, ingestion, feeds, and reports.
---

Atomic broadcasts realtime events over a WebSocket endpoint:

```text
ws://localhost:8080/ws?token=<token>
```

Use `wss://` when Atomic is behind HTTPS.

## Authentication

The WebSocket token is passed as a query parameter:

```text
/ws?token=<your-token>
```

Use a dedicated token for long-running integrations where possible.

## Event Envelope

Server events are JSON objects with a `type` field:

```json
{
  "type": "EmbeddingComplete",
  "atom_id": "uuid"
}
```

The React frontend normalizes these to kebab-case event names, but raw WebSocket clients receive the original server event shape.

## Pipeline Events

Raw server event types:

- `EmbeddingStarted`
- `EmbeddingComplete`
- `EmbeddingFailed`
- `TaggingComplete`
- `TaggingFailed`
- `TaggingSkipped`
- `BatchProgress`
- `PipelineQueueStarted`
- `PipelineQueueProgress`
- `PipelineQueueCompleted`
- `EventsLagged`

Frontend-normalized names include `embedding-started`, `embedding-complete`, `tagging-complete`, `batch-progress`, `pipeline-queue-started`, `pipeline-queue-progress`, `pipeline-queue-completed`, and `server-events-lagged`.

## Atom Events

- `AtomCreated`
- `AtomUpdated`

These are emitted when atoms are created or updated through API, bulk create, or MCP paths that broadcast lifecycle events.

Report findings ride this same channel: a successful scheduled or manual report run broadcasts `AtomCreated` with the new finding atom (which has `kind = 'report'`). The UI filters on `kind` to react only to findings when that matters.

## Import and Ingestion Events

- `ImportProgress`
- `IngestionFetchStarted`
- `IngestionFetchComplete`
- `IngestionFetchFailed`
- `IngestionSkipped`
- `IngestionComplete`
- `IngestionFailed`
- `FeedPollComplete`
- `FeedPollFailed`

These power progress UI for Obsidian import, URL ingestion, browser clipping, iOS share ingestion, and feed polling.

## Chat Events

- `ChatStreamDelta`
- `ChatToolStart`
- `ChatToolComplete`
- `ChatComplete`
- `ChatCanvasAction`
- `ChatError`
- `ChatConversationUpdated`

The message send endpoint returns a final response, but the UI receives streaming deltas and tool events over WebSocket.

### `ChatStreamDelta.content` is incremental

Each `ChatStreamDelta` carries **only the new text** produced since the previous delta. Clients append them; the concatenation of every delta for a `conversation_id` is the assistant's answer, and `ChatComplete` carries the authoritative full message.

This is a behavior change under an unchanged event name. Earlier builds sent the **cumulative** text so far in each delta, so a client written against them could replace its buffer on every delta and still render correctly. That client now renders only the last fragment. If you maintain a raw WebSocket client, switch it from replace to append — there is no version flag to branch on, and no way to tell the two shapes apart from a single delta.

## Dashboard Events

- `DashboardFeaturedChanged`

Emitted when the per-database featured-report pointer changes — explicitly via `PUT /api/dashboard/featured-report`, or implicitly when the featured report is deleted (the backend auto-clears the pointer in that case). The payload carries the new `report_id` (or `null` when cleared) so the dashboard widget and any open report detail-view star can refetch without polling. Frontend-normalized name: `dashboard-featured-changed`.

## Lag Handling

The server uses a broadcast channel. If a client falls behind, it can receive an `EventsLagged` event with the number of skipped events. Clients should reconcile state by refetching the relevant resource — atoms, pipeline status, or the latest finding from the featured report.

## Related

- [API Overview](/api/overview/)
- [Reports](/concepts/reports/)
- [URL Ingestion and Feeds](/guides/url-ingestion-and-feeds/)
