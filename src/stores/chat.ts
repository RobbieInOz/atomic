import { create } from 'zustand';
import { getTransport, isDemoInstance } from '../lib/transport';
import { useUIStore } from './ui';
import { useCanvasStore } from './canvas';

// ==================== Types ====================

export interface Tag {
  id: string;
  name: string;
  parent_id: string | null;
  created_at: string;
}

export interface Conversation {
  id: string;
  title: string | null;
  created_at: string;
  updated_at: string;
  is_archived: boolean;
}

export interface ConversationWithTags extends Conversation {
  tags: Tag[];
  message_count: number;
  last_message_preview: string | null;
}

export interface ChatMessage {
  id: string;
  conversation_id: string;
  role: 'user' | 'assistant' | 'system' | 'tool';
  content: string;
  created_at: string;
  message_index: number;
}

export interface ChatToolCall {
  id: string;
  message_id: string;
  tool_name: string;
  tool_input: unknown;
  tool_output: unknown | null;
  status: 'pending' | 'running' | 'complete' | 'failed';
  created_at: string;
  completed_at: string | null;
}

export interface ChatCitation {
  id: string;
  message_id: string;
  citation_index: number;
  atom_id: string;
  chunk_index: number | null;
  excerpt: string;
  relevance_score: number | null;
}

export interface ChatMessageWithContext extends ChatMessage {
  tool_calls: ChatToolCall[];
  citations: ChatCitation[];
}

export interface ConversationWithMessages extends Conversation {
  tags: Tag[];
  messages: ChatMessageWithContext[];
}

interface PageContext {
  view: string;
  atom_id?: string | null;
  atom_title?: string | null;
  atom_snippet?: string | null;
  wiki_tag_id?: string | null;
  wiki_tag_name?: string | null;
  selected_tag_id?: string | null;
}

function buildPageContext(): PageContext {
  const ui = useUIStore.getState();

  if (ui.readerState.atomId) {
    return {
      view: ui.readerState.editing ? 'atom_editor' : 'atom_reader',
      atom_id: ui.readerState.atomId,
      selected_tag_id: ui.selectedTagId,
    };
  }

  if (ui.wikiReaderState.tagId) {
    return {
      view: 'wiki_reader',
      wiki_tag_id: ui.wikiReaderState.tagId,
      wiki_tag_name: ui.wikiReaderState.tagName,
      selected_tag_id: ui.selectedTagId,
    };
  }

  if (ui.localGraph.isOpen && ui.localGraph.centerAtomId) {
    return {
      view: 'atom_graph',
      atom_id: ui.localGraph.centerAtomId,
      selected_tag_id: ui.selectedTagId,
    };
  }

  return {
    view: ui.viewMode,
    selected_tag_id: ui.selectedTagId,
  };
}

// ==================== Store ====================

type ChatView = 'list' | 'conversation';

interface ChatStore {
  // View state
  view: ChatView;

  // Current conversation (when view === 'conversation')
  currentConversation: ConversationWithTags | null;
  messages: ChatMessageWithContext[];

  // Conversations list
  conversations: ConversationWithTags[];
  listFilterTagId: string | null;
  /** Archived conversations are hidden until the list asks for them. */
  showArchived: boolean;

  /**
   * Conversations where the user dismissed the page-context chip. Client-side
   * and per-session by design: what the current page is has no meaning to the
   * server between requests, so persisting the choice would outlive the
   * context it was made about.
   */
  pageContextOff: Record<string, boolean>;

  // Streaming state
  isLoading: boolean;
  isStreaming: boolean;
  /**
   * A stop has been requested for the current turn. The backend finishes the
   * turn by persisting the partial answer, so streaming state stays up until
   * the completion event arrives — this only drives the button's pending look
   * and keeps repeat clicks from piling up requests.
   */
  isCancelling: boolean;
  streamingContent: string;
  streamingMessageId: string | null;
  /**
   * Tool calls fired during the current streaming turn. Populated from
   * chat-tool-start / chat-tool-complete events so the UI can render
   * collapsible cards that persist through streaming and get superseded
   * (not discarded) when the final ChatComplete message arrives.
   */
  streamingToolCalls: ChatToolCall[];

  // Error state
  error: string | null;

  // Actions - Navigation
  showList: (filterTagId?: string) => void;
  openConversation: (id: string) => Promise<void>;
  openOrCreateForTag: (tagId: string) => Promise<void>;
  goBack: () => void;

  // Actions - CRUD
  fetchConversations: (tagId?: string) => Promise<void>;
  setShowArchived: (show: boolean) => Promise<void>;
  createConversation: (tagIds?: string[]) => Promise<ConversationWithTags>;
  deleteConversation: (id: string) => Promise<void>;
  updateConversationTitle: (id: string, title: string) => Promise<void>;
  setConversationArchived: (id: string, isArchived: boolean) => Promise<void>;

  // Actions - Page context
  setPageContextEnabled: (conversationId: string, enabled: boolean) => void;

  // Actions - Scope Management
  setScope: (tagIds: string[]) => Promise<void>;
  addTagToScope: (tagId: string) => Promise<void>;
  removeTagFromScope: (tagId: string) => Promise<void>;

  // Actions - Messaging
  sendMessage: (content: string) => Promise<void>;
  cancelResponse: () => Promise<void>;

  // Actions - Streaming updates (called from event handlers)
  appendStreamContent: (delta: string) => void;
  startStreamingToolCall: (call: { tool_call_id: string; tool_name: string; tool_input: unknown }) => void;
  completeStreamingToolCall: (update: { tool_call_id: string; results_count: number; failed: boolean }) => void;
  completeMessage: (message: ChatMessageWithContext) => void;
  setStreamingError: (error: string) => void;
  applyConversationTitle: (id: string, title: string) => void;

  // Actions - Utilities
  clearError: () => void;
  reset: () => void;
}

export const useChatStore = create<ChatStore>((set, get) => ({
  // Initial state
  view: 'list',
  currentConversation: null,
  messages: [],
  conversations: [],
  listFilterTagId: null,
  showArchived: false,
  pageContextOff: {},
  isLoading: false,
  isStreaming: false,
  isCancelling: false,
  streamingContent: '',
  streamingMessageId: null,
  streamingToolCalls: [],
  error: null,

  // Navigation
  showList: (filterTagId?: string) => {
    set({
      view: 'list',
      listFilterTagId: filterTagId ?? null,
      currentConversation: null,
      messages: [],
    });
    useUIStore.getState().setChatSidebarConversationId(null);
    get().fetchConversations(filterTagId);
  },

  openConversation: async (id: string) => {
    set({ isLoading: true, error: null });
    try {
      const result = await getTransport().invoke<ConversationWithMessages | null>('get_conversation', {
        conversationId: id,
      });

      if (result) {
        set({
          view: 'conversation',
          currentConversation: {
            ...result,
            message_count: result.messages.length,
            last_message_preview: result.messages.length > 0
              ? result.messages[result.messages.length - 1].content.slice(0, 100)
              : null,
          },
          messages: result.messages,
          isLoading: false,
        });
        useUIStore.getState().setChatSidebarConversationId(id);
      } else {
        // Conversation went missing (deleted, different DB, stale persisted
        // id from a prior session) — fall back to the list rather than
        // leaving the user staring at an error with no recovery path.
        set({ isLoading: false, error: null });
        get().showList();
      }
    } catch (e) {
      set({ error: String(e), isLoading: false });
    }
  },

  openOrCreateForTag: async (tagId: string) => {
    set({ isLoading: true, error: null, listFilterTagId: tagId });
    try {
      // Fetch conversations that contain this tag
      const conversations = await getTransport().invoke<ConversationWithTags[]>('get_conversations', {
        filterTagId: tagId,
        limit: 50,
        offset: 0,
      });

      // Find a conversation with exactly this one tag
      const exactMatch = conversations.find(
        (c) => c.tags.length === 1 && c.tags[0].id === tagId
      );

      if (exactMatch) {
        // Open the existing conversation
        await get().openConversation(exactMatch.id);
      } else {
        // Create a new conversation with this tag
        await get().createConversation([tagId]);
      }
    } catch (e) {
      set({ error: String(e), isLoading: false });
    }
  },

  goBack: () => {
    const { listFilterTagId } = get();
    set({
      view: 'list',
      currentConversation: null,
      messages: [],
      streamingContent: '',
      streamingToolCalls: [],
    });
    useUIStore.getState().setChatSidebarConversationId(null);
    get().fetchConversations(listFilterTagId ?? undefined);
  },

  // CRUD
  fetchConversations: async (tagId?: string) => {
    // Demo visitors: conversations are closed server-side (403); render an
    // empty list under the signup-CTA input instead of an error state.
    if (isDemoInstance()) {
      set({ conversations: [], isLoading: false, listFilterTagId: tagId ?? null });
      return;
    }
    set({ isLoading: true, error: null });
    try {
      const conversations = await getTransport().invoke<ConversationWithTags[]>('get_conversations', {
        filterTagId: tagId ?? null,
        limit: 50,
        offset: 0,
        includeArchived: get().showArchived,
      });
      set({ conversations, isLoading: false, listFilterTagId: tagId ?? null });
    } catch (e) {
      set({ error: String(e), isLoading: false });
    }
  },

  setShowArchived: async (show: boolean) => {
    set({ showArchived: show });
    await get().fetchConversations(get().listFilterTagId ?? undefined);
  },

  createConversation: async (tagIds?: string[]) => {
    set({ isLoading: true, error: null });
    try {
      const conversation = await getTransport().invoke<ConversationWithTags>('create_conversation', {
        tagIds: tagIds ?? [],
        title: null,
      });

      // Add to list and open it
      set((state) => ({
        conversations: [conversation, ...state.conversations],
        view: 'conversation',
        currentConversation: conversation,
        messages: [],
        isLoading: false,
      }));

      return conversation;
    } catch (e) {
      set({ error: String(e), isLoading: false });
      throw e;
    }
  },

  deleteConversation: async (id: string) => {
    try {
      await getTransport().invoke('delete_conversation', { id });
      set((state) => ({
        conversations: state.conversations.filter((c) => c.id !== id),
        // If we deleted the current conversation, go back to list
        ...(state.currentConversation?.id === id
          ? { view: 'list' as const, currentConversation: null, messages: [] }
          : {}),
      }));
    } catch (e) {
      set({ error: String(e) });
    }
  },

  updateConversationTitle: async (id: string, title: string) => {
    try {
      await getTransport().invoke('update_conversation', { id, title, isArchived: null });
      get().applyConversationTitle(id, title);
    } catch (e) {
      set({ error: String(e) });
    }
  },

  setConversationArchived: async (id: string, isArchived: boolean) => {
    try {
      await getTransport().invoke('update_conversation', { id, title: null, isArchived });
      set((state) => ({
        // The list only holds archived conversations while it is showing
        // them; otherwise archiving removes the card outright.
        conversations: state.showArchived
          ? state.conversations.map((c) => (c.id === id ? { ...c, is_archived: isArchived } : c))
          : state.conversations.filter((c) => c.id !== id),
        currentConversation:
          state.currentConversation?.id === id
            ? { ...state.currentConversation, is_archived: isArchived }
            : state.currentConversation,
      }));
    } catch (e) {
      set({ error: String(e) });
    }
  },

  // Page context
  setPageContextEnabled: (conversationId: string, enabled: boolean) => {
    set((state) => ({
      pageContextOff: { ...state.pageContextOff, [conversationId]: !enabled },
    }));
  },

  // Scope Management
  setScope: async (tagIds: string[]) => {
    const { currentConversation } = get();
    if (!currentConversation) return;

    try {
      const updated = await getTransport().invoke<ConversationWithTags>('set_conversation_scope', {
        conversationId: currentConversation.id,
        tagIds,
      });
      set({ currentConversation: updated });
    } catch (e) {
      set({ error: String(e) });
    }
  },

  addTagToScope: async (tagId: string) => {
    const { currentConversation } = get();
    if (!currentConversation) return;

    try {
      const updated = await getTransport().invoke<ConversationWithTags>('add_tag_to_scope', {
        conversationId: currentConversation.id,
        tagId,
      });
      set({ currentConversation: updated });
    } catch (e) {
      set({ error: String(e) });
    }
  },

  removeTagFromScope: async (tagId: string) => {
    const { currentConversation } = get();
    if (!currentConversation) return;

    try {
      const updated = await getTransport().invoke<ConversationWithTags>('remove_tag_from_scope', {
        conversationId: currentConversation.id,
        tagId,
      });
      set({ currentConversation: updated });
    } catch (e) {
      set({ error: String(e) });
    }
  },

  // Messaging
  sendMessage: async (content: string) => {
    const { currentConversation, messages, openConversation } = get();
    if (!currentConversation) {
      set({ error: 'No conversation selected' });
      return;
    }

    // Add user message optimistically
    const userMessage: ChatMessageWithContext = {
      id: `temp-user-${Date.now()}`,
      conversation_id: currentConversation.id,
      role: 'user',
      content,
      created_at: new Date().toISOString(),
      message_index: messages.length,
      tool_calls: [],
      citations: [],
    };

    set({
      messages: [...messages, userMessage],
      isStreaming: true,
      isCancelling: false,
      streamingContent: '',
      streamingToolCalls: [],
      error: null,
    });

    try {
      // Auto-inject canvas context when chatting from canvas view
      let canvasContext = undefined;
      if (useUIStore.getState().viewMode === 'canvas') {
        const canvasData = useCanvasStore.getState().canvasData;
        if (canvasData) {
          canvasContext = {
            clusters: canvasData.clusters.map((c) => ({
              label: c.label,
              atom_count: c.atom_count,
            })),
          };
        }
      }

      // Dismissing the context chip is a promise that nothing about the
      // current page leaves with the message.
      const sharesPageContext = !get().pageContextOff[currentConversation.id];

      await getTransport().invoke<ChatMessageWithContext>('send_chat_message', {
        conversationId: currentConversation.id,
        content,
        canvasContext,
        pageContext: sharesPageContext ? buildPageContext() : undefined,
      });

      // Refetch the conversation to get the properly saved messages
      // This ensures correct IDs and ordering from the database
      await openConversation(currentConversation.id);
    } catch (e) {
      // Remove the temp user message on error
      set((state) => ({
        messages: state.messages.filter((m) => !m.id.startsWith('temp-')),
        error: String(e),
        isStreaming: false,
        isCancelling: false,
        streamingContent: '',
      }));
    }
  },

  cancelResponse: async () => {
    const { currentConversation, isStreaming, isCancelling } = get();
    if (!currentConversation || !isStreaming || isCancelling) return;

    set({ isCancelling: true });
    try {
      await getTransport().invoke('cancel_chat_message', {
        conversationId: currentConversation.id,
      });
      // The turn keeps streaming until the backend finalizes the partial
      // answer; completeMessage clears the streaming state from the event.
    } catch (e) {
      // The request never landed — nothing is coming back to clear the UI,
      // so drop the streaming state here.
      set({ isStreaming: false, isCancelling: false, streamingContent: '', error: String(e) });
    }
  },

  // Streaming updates - deltas arrive one provider chunk at a time
  appendStreamContent: (delta: string) => {
    set((state) => ({ streamingContent: state.streamingContent + delta }));
  },

  startStreamingToolCall: ({ tool_call_id, tool_name, tool_input }) => {
    const now = new Date().toISOString();
    set((state) => ({
      streamingToolCalls: [
        ...state.streamingToolCalls,
        {
          id: tool_call_id,
          message_id: '',
          tool_name,
          tool_input,
          tool_output: null,
          status: 'running',
          created_at: now,
          completed_at: null,
        },
      ],
    }));
  },

  completeStreamingToolCall: ({ tool_call_id, results_count, failed }) => {
    const now = new Date().toISOString();
    set((state) => ({
      streamingToolCalls: state.streamingToolCalls.map((call) =>
        call.id === tool_call_id
          ? {
              ...call,
              status: failed ? ('failed' as const) : ('complete' as const),
              tool_output: { results_count },
              completed_at: now,
            }
          : call
      ),
    }));
  },

  completeMessage: (message: ChatMessageWithContext) => {
    set((state) => {
      // Don't add if message already exists (prevents duplicates from event + refetch)
      const messageExists = state.messages.some((m) => m.id === message.id);
      if (messageExists) {
        return {
          isStreaming: false,
          isCancelling: false,
          streamingContent: '',
          streamingMessageId: null,
          streamingToolCalls: [],
        };
      }
      return {
        messages: [...state.messages, message],
        isStreaming: false,
        isCancelling: false,
        streamingContent: '',
        streamingMessageId: null,
        streamingToolCalls: [],
      };
    });
  },

  setStreamingError: (error: string) => {
    set({
      error,
      isStreaming: false,
      isCancelling: false,
      streamingContent: '',
    });
  },

  applyConversationTitle: (id: string, title: string) => {
    set((state) => ({
      conversations: state.conversations.map((c) => (c.id === id ? { ...c, title } : c)),
      currentConversation:
        state.currentConversation?.id === id
          ? { ...state.currentConversation, title }
          : state.currentConversation,
    }));
  },

  // Utilities
  clearError: () => set({ error: null }),

  reset: () =>
    set({
      view: 'list',
      currentConversation: null,
      messages: [],
      conversations: [],
      listFilterTagId: null,
      showArchived: false,
      pageContextOff: {},
      isLoading: false,
      isStreaming: false,
      isCancelling: false,
      streamingContent: '',
      streamingMessageId: null,
      streamingToolCalls: [],
      error: null,
    }),
}));
