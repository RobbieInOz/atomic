import { useEffect } from 'react';
import { getTransport } from '../lib/transport';
import { useChatStore, ChatMessageWithContext } from '../stores/chat';

interface ChatStreamDelta {
  conversation_id: string;
  content: string;
}

interface ChatToolStart {
  conversation_id: string;
  tool_call_id: string;
  tool_name: string;
  tool_input: unknown;
}

interface ChatToolComplete {
  conversation_id: string;
  tool_call_id: string;
  results_count: number;
  failed: boolean;
}

interface ChatComplete {
  conversation_id: string;
  message: ChatMessageWithContext;
}

interface ChatError {
  conversation_id: string;
  error: string;
}

interface ChatConversationUpdated {
  conversation_id: string;
  title: string;
}

/**
 * Subscribe to the chat event stream for the whole pane.
 *
 * Mounted once, at the always-present ChatViewer, and deliberately *not*
 * scoped to a conversation id. A turn outlives the view it was started from:
 * the user hits back, opens another conversation, archives this one — and the
 * completion event still has to land, or the streaming state it was going to
 * clear stays raised forever and every conversation renders a phantom
 * "Thinking…" with a dead input. Each payload carries the conversation it is
 * about and the store decides what applies to what (see
 * `streamingConversationId`), which is also why this hook has nothing to
 * filter.
 */
export function useChatEvents() {
  const appendStreamContent = useChatStore(s => s.appendStreamContent);
  const startStreamingToolCall = useChatStore(s => s.startStreamingToolCall);
  const completeStreamingToolCall = useChatStore(s => s.completeStreamingToolCall);
  const completeMessage = useChatStore(s => s.completeMessage);
  const setStreamingError = useChatStore(s => s.setStreamingError);
  const applyConversationTitle = useChatStore(s => s.applyConversationTitle);

  useEffect(() => {
    const transport = getTransport();
    const unsubs: Array<() => void> = [];

    unsubs.push(transport.subscribe<ChatStreamDelta>('chat-stream-delta', (payload) => {
      appendStreamContent(payload.conversation_id, payload.content);
    }));

    unsubs.push(transport.subscribe<ChatToolStart>('chat-tool-start', (payload) => {
      startStreamingToolCall({
        conversation_id: payload.conversation_id,
        tool_call_id: payload.tool_call_id,
        tool_name: payload.tool_name,
        tool_input: payload.tool_input,
      });
    }));

    unsubs.push(transport.subscribe<ChatToolComplete>('chat-tool-complete', (payload) => {
      completeStreamingToolCall({
        conversation_id: payload.conversation_id,
        tool_call_id: payload.tool_call_id,
        results_count: payload.results_count,
        failed: payload.failed ?? false,
      });
    }));

    unsubs.push(transport.subscribe<ChatComplete>('chat-complete', (payload) => {
      completeMessage(payload.conversation_id, payload.message);
    }));

    unsubs.push(transport.subscribe<ChatError>('chat-error', (payload) => {
      setStreamingError(payload.conversation_id, payload.error);
    }));

    // The auto-generated title lands after the turn completes; applying it
    // here updates the header and the list entry without a refetch.
    unsubs.push(transport.subscribe<ChatConversationUpdated>('chat-conversation-updated', (payload) => {
      applyConversationTitle(payload.conversation_id, payload.title);
    }));

    return () => {
      unsubs.forEach((unsub) => unsub());
    };
  }, [appendStreamContent, startStreamingToolCall, completeStreamingToolCall, completeMessage, setStreamingError, applyConversationTitle]);
}
