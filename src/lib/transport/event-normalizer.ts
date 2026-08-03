interface NormalizedEvent {
  event: string;
  payload: unknown;
}

export function normalizeServerEvent(data: Record<string, unknown>): NormalizedEvent | null {
  const type = data.type as string;
  switch (type) {
    case 'EmbeddingStarted':
      return { event: 'embedding-started', payload: { atom_id: data.atom_id } };
    case 'EmbeddingComplete':
      return { event: 'embedding-complete', payload: { atom_id: data.atom_id, status: 'complete' } };
    case 'EmbeddingFailed':
      return { event: 'embedding-complete', payload: { atom_id: data.atom_id, status: 'failed', error: data.error } };
    case 'TaggingComplete':
      return { event: 'tagging-complete', payload: { atom_id: data.atom_id, status: 'complete', tags_extracted: data.tags_extracted, new_tags_created: data.new_tags_created } };
    case 'TaggingFailed':
      return { event: 'tagging-complete', payload: { atom_id: data.atom_id, status: 'failed', error: data.error, tags_extracted: [], new_tags_created: [] } };
    case 'TaggingSkipped':
      return { event: 'tagging-complete', payload: { atom_id: data.atom_id, status: 'skipped', tags_extracted: [], new_tags_created: [] } };
    case 'ChatStreamDelta':
      return { event: 'chat-stream-delta', payload: data };
    case 'ChatToolStart':
      return { event: 'chat-tool-start', payload: data };
    case 'ChatToolComplete':
      return { event: 'chat-tool-complete', payload: data };
    case 'ChatComplete':
      return { event: 'chat-complete', payload: data };
    case 'ChatCanvasAction':
      return { event: 'chat-canvas-action', payload: data };
    case 'ChatError':
      return { event: 'chat-error', payload: data };
    case 'ChatConversationUpdated':
      return { event: 'chat-conversation-updated', payload: data };
    case 'AtomCreated':
      return { event: 'atom-created', payload: data.atom };
    case 'AtomUpdated':
      return { event: 'atom-updated', payload: data.atom };
    case 'EmbeddingsReset':
      return { event: 'embeddings-reset', payload: data };
    case 'ImportProgress':
      return { event: 'import-progress', payload: { current: data.current, total: data.total, current_file: data.current_file, status: data.status } };
    case 'IngestionFetchStarted':
      return { event: 'ingestion-fetch-started', payload: { url: data.url, request_id: data.request_id } };
    case 'IngestionFetchComplete':
      return { event: 'ingestion-fetch-complete', payload: { url: data.url, request_id: data.request_id, content_length: data.content_length } };
    case 'IngestionFetchFailed':
      return { event: 'ingestion-fetch-failed', payload: { url: data.url, request_id: data.request_id, error: data.error } };
    case 'IngestionSkipped':
      return { event: 'ingestion-skipped', payload: { url: data.url, request_id: data.request_id, reason: data.reason } };
    case 'IngestionComplete':
      return { event: 'ingestion-complete', payload: { request_id: data.request_id, atom_id: data.atom_id, url: data.url, title: data.title } };
    case 'IngestionFailed':
      return { event: 'ingestion-failed', payload: { request_id: data.request_id, url: data.url, error: data.error } };
    case 'FeedPollComplete':
      return { event: 'feed-poll-complete', payload: { feed_id: data.feed_id, new_items: data.new_items, skipped: data.skipped, errors: data.errors } };
    case 'FeedPollFailed':
      return { event: 'feed-poll-failed', payload: { feed_id: data.feed_id, error: data.error } };
    case 'BatchProgress':
      return { event: 'batch-progress', payload: { batch_id: data.batch_id, phase: data.phase, completed: data.completed, total: data.total } };
    case 'PipelineQueueStarted':
      return { event: 'pipeline-queue-started', payload: { run_id: data.run_id, total_jobs: data.total_jobs, embedding_total: data.embedding_total } };
    case 'PipelineQueueProgress':
      return { event: 'pipeline-queue-progress', payload: { run_id: data.run_id, stage: data.stage, completed: data.completed, total: data.total } };
    case 'PipelineQueueCompleted':
      return { event: 'pipeline-queue-completed', payload: { run_id: data.run_id, total_jobs: data.total_jobs, failed_jobs: data.failed_jobs } };
    case 'EventsLagged':
      return { event: 'server-events-lagged', payload: { skipped: data.skipped } };
    case 'BriefingReady':
      return { event: 'briefing-ready', payload: { db_id: data.db_id, briefing_id: data.briefing_id } };
    case 'DashboardFeaturedChanged':
      return { event: 'dashboard-featured-changed', payload: { report_id: data.report_id ?? null } };
    default:
      console.warn('Unknown server event type:', type);
      return null;
  }
}
