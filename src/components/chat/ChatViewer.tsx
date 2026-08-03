import { useEffect, useRef } from 'react';
import { Maximize2, Minimize2, PanelLeft, X } from 'lucide-react';
import { useChatStore } from '../../stores/chat';
import { useUIStore } from '../../stores/ui';
import { useDatabasesStore } from '../../stores/databases';
import { isTypingTarget } from '../../lib/keyboard';
import { isTauri } from '../../lib/platform';
import { useIsMobile } from '../../hooks';
import { useChatEvents } from '../../hooks/useChatEvents';
import { ConversationsList } from './ConversationsList';
import { ChatView } from './ChatView';

export function ChatViewer() {
  const view = useChatStore(s => s.view);
  const showList = useChatStore(s => s.showList);
  const openConversation = useChatStore(s => s.openConversation);
  const openOrCreateForTag = useChatStore(s => s.openOrCreateForTag);
  const setChatSidebarOpen = useUIStore(s => s.setChatSidebarOpen);
  const isExpanded = useUIStore(s => s.chatSidebarExpanded);
  const setChatSidebarExpanded = useUIStore(s => s.setChatSidebarExpanded);
  const toggleChatSidebarExpanded = useUIStore(s => s.toggleChatSidebarExpanded);
  const initialTagId = useUIStore(s => s.chatSidebarInitialTagId);
  const initialConversationId = useUIStore(s => s.chatSidebarInitialConversationId);
  const lastConversationId = useUIStore(s => s.chatSidebarConversationId);
  const leftPanelOpen = useUIStore(s => s.leftPanelOpen);
  const toggleLeftPanel = useUIStore(s => s.toggleLeftPanel);
  const activeDbId = useDatabasesStore(s => s.activeId);
  const isMobile = useIsMobile();
  const initializedRef = useRef(false);

  // Fullscreen is a desktop-only mode — on mobile the pane already fills the
  // screen, and the main view behind it is never collapsed.
  const isFullscreen = isExpanded && !isMobile;

  // Subscribed here, not in ChatView: a turn survives the user leaving the
  // conversation it was started from, and its completion event has to be
  // heard from wherever they end up or the streaming state never clears.
  useChatEvents();

  // Re-initialize when database changes
  useEffect(() => {
    if (initializedRef.current) {
      showList();
    }
  }, [activeDbId, showList]);

  // Initialize on first mount, and navigate when new initial values are set
  useEffect(() => {
    if (initialConversationId) {
      openConversation(initialConversationId);
      useUIStore.getState().clearChatSidebarInitial();
    } else if (initialTagId) {
      openOrCreateForTag(initialTagId);
      useUIStore.getState().clearChatSidebarInitial();
    } else if (!initializedRef.current) {
      if (lastConversationId) {
        openConversation(lastConversationId);
      } else {
        showList();
      }
    }
    initializedRef.current = true;
  }, [initialTagId, initialConversationId, lastConversationId, showList, openConversation, openOrCreateForTag]);

  // Escape leaves fullscreen. Listening in the capture phase and stopping the
  // event there is what keeps it from also reaching the reader mounted behind
  // the expanded pane, whose own Escape handler would dismiss the very tab the
  // user is about to come back to. Typing targets are handed the key untouched
  // so the composer and the in-chat search bar keep their own Escape, and an
  // open modal owns it outright — the convention `useKeyboard` follows.
  useEffect(() => {
    if (!isExpanded) return;
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      if (document.querySelector('[data-modal="true"]')) return;
      if (isTypingTarget(e.target)) return;
      e.stopPropagation();
      setChatSidebarExpanded(false);
    };
    document.addEventListener('keydown', handleKeyDown, true);
    return () => document.removeEventListener('keydown', handleKeyDown, true);
  }, [isExpanded, setChatSidebarExpanded]);

  return (
    <div className="h-full flex flex-col bg-[var(--color-bg-panel)]">
      {/* Header. At fullscreen this row stands in for the main view's titlebar,
          which is collapsed away with the rest of that column, so it picks up
          the two jobs that bar was doing: toggling the left panel — now showing
          this chat's conversations — and, under Tauri, holding the traffic
          lights clear. That clearance must be carried by exactly one *visible*
          surface: the left panel's own titlebar owns it whenever the panel is
          open, and MainView's bar only ever competes for it when not expanded. */}
      <div
        data-tauri-drag-region={isFullscreen || undefined}
        className={`flex-shrink-0 flex items-center justify-between px-4 py-3 border-b border-[var(--color-border)] ${
          isFullscreen ? 'drag-region' : ''
        } ${isFullscreen && isTauri() && !leftPanelOpen ? 'pl-[78px]' : ''}`}
      >
        <div className="flex items-center gap-2">
          {isFullscreen && (
            <button
              onClick={toggleLeftPanel}
              className={`p-1.5 rounded-md transition-colors ${
                leftPanelOpen
                  ? 'text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]'
                  : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]'
              }`}
              title={leftPanelOpen ? 'Hide sidebar' : 'Show sidebar'}
              aria-label={leftPanelOpen ? 'Hide sidebar' : 'Show sidebar'}
            >
              <PanelLeft className="w-4 h-4" strokeWidth={2} />
            </button>
          )}
          <h2 className="text-lg font-semibold text-[var(--color-text-primary)]">
            {view === 'list' ? 'Conversations' : 'Chat'}
          </h2>
        </div>
        <div className="flex items-center gap-1">
          {/* Desktop only: on mobile the pane is already full-screen. */}
          <button
            onClick={toggleChatSidebarExpanded}
            className="hidden md:block p-1 text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] transition-colors"
            aria-label={isExpanded ? 'Minimize chat' : 'Expand chat'}
            title={isExpanded ? 'Minimize chat (Esc)' : 'Expand chat'}
          >
            {isExpanded
              ? <Minimize2 className="w-4 h-4" strokeWidth={2} />
              : <Maximize2 className="w-4 h-4" strokeWidth={2} />}
          </button>
          <button
            onClick={() => setChatSidebarOpen(false)}
            className="p-1 text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] transition-colors"
            aria-label="Close"
          >
            <X className="w-5 h-5" strokeWidth={2} />
          </button>
        </div>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-hidden">
        {view === 'list' ? <ConversationsList /> : <ChatView />}
      </div>
    </div>
  );
}
