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

export function useChatEvents(conversationId: string | null) {
  const appendStreamContent = useChatStore(s => s.appendStreamContent);
  const startStreamingToolCall = useChatStore(s => s.startStreamingToolCall);
  const completeStreamingToolCall = useChatStore(s => s.completeStreamingToolCall);
  const completeMessage = useChatStore(s => s.completeMessage);
  const setStreamingError = useChatStore(s => s.setStreamingError);
  const applyConversationTitle = useChatStore(s => s.applyConversationTitle);

  useEffect(() => {
    if (!conversationId) return;

    const transport = getTransport();
    const unsubs: Array<() => void> = [];

    unsubs.push(transport.subscribe<ChatStreamDelta>('chat-stream-delta', (payload) => {
      if (payload.conversation_id === conversationId) {
        appendStreamContent(payload.content);
      }
    }));

    unsubs.push(transport.subscribe<ChatToolStart>('chat-tool-start', (payload) => {
      if (payload.conversation_id === conversationId) {
        startStreamingToolCall({
          tool_call_id: payload.tool_call_id,
          tool_name: payload.tool_name,
          tool_input: payload.tool_input,
        });
      }
    }));

    unsubs.push(transport.subscribe<ChatToolComplete>('chat-tool-complete', (payload) => {
      if (payload.conversation_id === conversationId) {
        completeStreamingToolCall({
          tool_call_id: payload.tool_call_id,
          results_count: payload.results_count,
          failed: payload.failed ?? false,
        });
      }
    }));

    unsubs.push(transport.subscribe<ChatComplete>('chat-complete', (payload) => {
      if (payload.conversation_id === conversationId) {
        completeMessage(payload.message);
      }
    }));

    unsubs.push(transport.subscribe<ChatError>('chat-error', (payload) => {
      if (payload.conversation_id === conversationId) {
        setStreamingError(payload.error);
      }
    }));

    // The auto-generated title lands after the turn completes; applying it
    // here updates the header and the list entry without a refetch.
    unsubs.push(transport.subscribe<ChatConversationUpdated>('chat-conversation-updated', (payload) => {
      if (payload.conversation_id === conversationId) {
        applyConversationTitle(payload.conversation_id, payload.title);
      }
    }));

    return () => {
      unsubs.forEach((unsub) => unsub());
    };
  }, [conversationId, appendStreamContent, startStreamingToolCall, completeStreamingToolCall, completeMessage, setStreamingError, applyConversationTitle]);
}
