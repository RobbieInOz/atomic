import { describe, it, expect, beforeEach, vi } from 'vitest';
import { useChatStore, type ChatMessageWithContext, type ConversationWithTags } from './chat';

const invoke = vi.fn();
vi.mock('../lib/transport', () => ({
  getTransport: () => ({ invoke, subscribe: () => () => {} }),
  isDemoInstance: () => false,
}));

/// The streaming state machine, exercised through the actions the event
/// subscription calls. The regression these cover: before the turn carried a
/// conversation id, leaving a conversation mid-stream dropped the completion
/// event on the floor, and `isStreaming` stayed raised forever — every
/// conversation then rendered a phantom "Thinking…" over a dead composer,
/// recoverable only by reloading the app.

const A = 'conversation-a';
const B = 'conversation-b';

function conversation(id: string): ConversationWithTags {
  return {
    id,
    title: null,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    is_archived: false,
    tags: [],
    message_count: 0,
    last_message_preview: null,
  };
}

function answer(id: string, conversationId: string): ChatMessageWithContext {
  return {
    id,
    conversation_id: conversationId,
    role: 'assistant',
    content: 'the answer',
    created_at: '2026-01-01T00:00:00Z',
    message_index: 1,
    tool_calls: [],
    citations: [],
  };
}

/// The store as it looks one message into a turn on `A`.
function streamingOn(conversationId: string, open = conversationId) {
  useChatStore.setState({
    view: 'conversation',
    currentConversation: conversation(open),
    messages: [],
    streamingConversationId: conversationId,
    isStreaming: true,
    isCancelling: false,
    streamingContent: '',
    streamingToolCalls: [],
    error: null,
  });
}

describe('chat streaming state', () => {
  beforeEach(() => {
    useChatStore.getState().reset();
  });

  it('completes a turn the user left, and leaves nothing streaming behind', () => {
    streamingOn(A);
    // The user goes back to the list mid-stream.
    useChatStore.setState({ view: 'list', currentConversation: null, messages: [] });
    expect(useChatStore.getState().isStreaming).toBe(true);

    useChatStore.getState().completeMessage(A, answer('m1', A));

    const state = useChatStore.getState();
    expect(state.isStreaming).toBe(false);
    expect(state.isCancelling).toBe(false);
    expect(state.streamingConversationId).toBeNull();
    expect(state.streamingContent).toBe('');
    expect(state.streamingToolCalls).toEqual([]);
    // The answer belongs to a conversation that isn't on screen, so it is not
    // spliced into whatever transcript happens to be.
    expect(state.messages).toEqual([]);
  });

  it('adds the completed answer when its conversation is the one on screen', () => {
    streamingOn(A);
    useChatStore.getState().completeMessage(A, answer('m1', A));
    expect(useChatStore.getState().messages.map((m) => m.id)).toEqual(['m1']);

    // The refetch that follows delivers the same message; it must not double.
    useChatStore.getState().completeMessage(A, answer('m1', A));
    expect(useChatStore.getState().messages.map((m) => m.id)).toEqual(['m1']);
  });

  it('ignores events from a conversation that is not the streaming one', () => {
    streamingOn(A);
    useChatStore.getState().appendStreamContent(B, 'not mine');
    useChatStore.getState().startStreamingToolCall({
      conversation_id: B,
      tool_call_id: 't1',
      tool_name: 'search_atoms',
      tool_input: {},
    });
    useChatStore.getState().completeMessage(B, answer('m9', B));

    const state = useChatStore.getState();
    expect(state.streamingContent).toBe('');
    expect(state.streamingToolCalls).toEqual([]);
    // B's completion must not release A's turn.
    expect(state.isStreaming).toBe(true);
    expect(state.streamingConversationId).toBe(A);
  });

  it('accumulates deltas and tool calls for the streaming conversation', () => {
    streamingOn(A);
    useChatStore.getState().appendStreamContent(A, 'Hello ');
    useChatStore.getState().appendStreamContent(A, 'world');
    useChatStore.getState().startStreamingToolCall({
      conversation_id: A,
      tool_call_id: 't1',
      tool_name: 'search_atoms',
      tool_input: { query: 'x' },
    });
    useChatStore
      .getState()
      .completeStreamingToolCall({ conversation_id: A, tool_call_id: 't1', results_count: 3, failed: false });

    const state = useChatStore.getState();
    expect(state.streamingContent).toBe('Hello world');
    expect(state.streamingToolCalls).toHaveLength(1);
    expect(state.streamingToolCalls[0].status).toBe('complete');
    expect(state.streamingToolCalls[0].tool_output).toEqual({ results_count: 3 });
  });

  it('clears the turn on an error, wherever the user is', () => {
    streamingOn(A, B);
    useChatStore.getState().setStreamingError(A, 'provider exploded');

    const state = useChatStore.getState();
    expect(state.isStreaming).toBe(false);
    expect(state.streamingConversationId).toBeNull();
    expect(state.error).toBe('provider exploded');
  });

  it('reset() puts the streaming slot back to empty', () => {
    streamingOn(A);
    useChatStore.getState().appendStreamContent(A, 'partial');
    useChatStore.getState().reset();

    const state = useChatStore.getState();
    expect(state.streamingConversationId).toBeNull();
    expect(state.isStreaming).toBe(false);
    expect(state.streamingContent).toBe('');
    expect(state.savingAnswerMessageIds).toEqual({});
  });
});

describe('cancelResponse', () => {
  beforeEach(() => {
    useChatStore.getState().reset();
    invoke.mockReset();
  });

  it('un-latches Stop when the server had no turn to cancel', async () => {
    streamingOn(A);
    // The cancel beat its own turn's registration (or the turn already
    // finished): nothing will arrive to clear the pending look.
    invoke.mockResolvedValue({ cancelled: false });

    await useChatStore.getState().cancelResponse();

    const state = useChatStore.getState();
    expect(state.isCancelling).toBe(false);
    expect(state.isStreaming).toBe(true);
    expect(invoke).toHaveBeenCalledWith('cancel_chat_message', { conversationId: A });
  });

  it('keeps Stop pending while a signalled turn finishes its partial answer', async () => {
    streamingOn(A);
    invoke.mockResolvedValue({ cancelled: true });

    await useChatStore.getState().cancelResponse();

    expect(useChatStore.getState().isCancelling).toBe(true);
    expect(useChatStore.getState().isStreaming).toBe(true);

    // The turn still finalizes, and that is what ends the streaming state.
    useChatStore.getState().completeMessage(A, answer('m1', A));
    expect(useChatStore.getState().isStreaming).toBe(false);
    expect(useChatStore.getState().isCancelling).toBe(false);
  });

  it('drops the streaming state when the cancel request itself fails', async () => {
    streamingOn(A);
    invoke.mockRejectedValue(new Error('offline'));

    await useChatStore.getState().cancelResponse();

    const state = useChatStore.getState();
    expect(state.isStreaming).toBe(false);
    expect(state.streamingConversationId).toBeNull();
    expect(state.error).toContain('offline');
  });

  it('cancels the streaming conversation even after the user navigated away', async () => {
    streamingOn(A, B);
    invoke.mockResolvedValue({ cancelled: true });

    await useChatStore.getState().cancelResponse();

    expect(invoke).toHaveBeenCalledWith('cancel_chat_message', { conversationId: A });
  });
});
