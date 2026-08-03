import { useEffect, useState } from 'react';
import { Plus } from 'lucide-react';
import { useChatStore, type ConversationWithTags } from '../../stores/chat';
import { useDatabasesStore } from '../../stores/databases';
import { useUIStore } from '../../stores/ui';
import { ConversationCard } from './ConversationCard';
import { Modal } from '../ui/Modal';

/// The left panel while chat is expanded to fullscreen: every conversation,
/// one click from the one on screen. The chat pane keeps its own list view —
/// this is an additional way in, not a replacement for it.
export function ConversationsSidebar() {
  const conversations = useChatStore(s => s.conversations);
  const currentConversationId = useChatStore(s => s.currentConversation?.id ?? null);
  const listFilterTagId = useChatStore(s => s.listFilterTagId);
  const fetchConversations = useChatStore(s => s.fetchConversations);
  const openConversation = useChatStore(s => s.openConversation);
  const createConversation = useChatStore(s => s.createConversation);
  const deleteConversation = useChatStore(s => s.deleteConversation);
  const updateConversationTitle = useChatStore(s => s.updateConversationTitle);
  const setConversationArchived = useChatStore(s => s.setConversationArchived);
  const selectedTagId = useUIStore(s => s.selectedTagId);
  const activeDatabaseId = useDatabasesStore(s => s.activeId);

  const [deleteTarget, setDeleteTarget] = useState<ConversationWithTags | null>(null);
  const [isDeleting, setIsDeleting] = useState(false);

  // The panel mounts when chat goes fullscreen, which can be long after the
  // pane last fetched the list — and a database switch invalidates it outright.
  useEffect(() => {
    void fetchConversations(listFilterTagId ?? undefined);
  }, [fetchConversations, listFilterTagId, activeDatabaseId]);

  const handleNewChat = async () => {
    try {
      // Same scoping suggestion the pane's list makes: whatever the user is
      // already looking at.
      const scopeTagId = listFilterTagId ?? selectedTagId;
      await createConversation(scopeTagId ? [scopeTagId] : []);
    } catch (e) {
      console.error('Failed to create conversation:', e);
    }
  };

  const handleConfirmDelete = async () => {
    if (!deleteTarget) return;
    setIsDeleting(true);
    try {
      await deleteConversation(deleteTarget.id);
    } catch (e) {
      console.error('Failed to delete conversation:', e);
    } finally {
      setIsDeleting(false);
      setDeleteTarget(null);
    }
  };

  return (
    <div className="h-full flex flex-col">
      <div className="flex-shrink-0 px-3 py-2">
        <button
          onClick={handleNewChat}
          className="w-full flex items-center justify-center gap-2 px-3 py-1.5 text-sm bg-[var(--color-bg-hover)] hover:bg-[var(--color-border)] text-[var(--color-text-primary)] rounded-md transition-colors"
        >
          <Plus className="w-4 h-4" strokeWidth={2} />
          New Conversation
        </button>
      </div>

      <div className="flex-1 overflow-y-auto scrollbar-hidden">
        {conversations.length === 0 ? (
          <p className="px-3 py-4 text-xs text-[var(--color-text-secondary)] leading-relaxed">
            No conversations yet. Ask a question in the chat pane and it shows up here.
          </p>
        ) : (
          <div className="divide-y divide-[var(--color-border)]">
            {conversations.map((conversation) => (
              <ConversationCard
                key={conversation.id}
                conversation={conversation}
                isActive={conversation.id === currentConversationId}
                onClick={() => openConversation(conversation.id)}
                onDelete={(e) => {
                  e.stopPropagation();
                  setDeleteTarget(conversation);
                }}
                onRename={(title) => updateConversationTitle(conversation.id, title)}
                onArchive={(isArchived) => setConversationArchived(conversation.id, isArchived)}
              />
            ))}
          </div>
        )}
      </div>

      <Modal
        isOpen={deleteTarget !== null}
        onClose={() => setDeleteTarget(null)}
        title="Delete Conversation"
        confirmLabel={isDeleting ? 'Deleting...' : 'Delete'}
        confirmVariant="danger"
        onConfirm={handleConfirmDelete}
      >
        <p>
          Are you sure you want to delete "{deleteTarget?.title || 'New Conversation'}"?
          This will remove all messages and cannot be undone.
        </p>
      </Modal>
    </div>
  );
}
