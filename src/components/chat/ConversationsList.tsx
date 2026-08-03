import { useState } from 'react';
import { Plus, MessageCircle } from 'lucide-react';
import { useChatStore, ConversationWithTags } from '../../stores/chat';
import { useUIStore } from '../../stores/ui';
import { ConversationCard } from './ConversationCard';
import { Modal } from '../ui/Modal';

export function ConversationsList() {
  const conversations = useChatStore(s => s.conversations);
  const isLoading = useChatStore(s => s.isLoading);
  const error = useChatStore(s => s.error);
  const listFilterTagId = useChatStore(s => s.listFilterTagId);
  const showArchived = useChatStore(s => s.showArchived);
  const setShowArchived = useChatStore(s => s.setShowArchived);
  const createConversation = useChatStore(s => s.createConversation);
  const openConversation = useChatStore(s => s.openConversation);
  const deleteConversation = useChatStore(s => s.deleteConversation);
  const updateConversationTitle = useChatStore(s => s.updateConversationTitle);
  const setConversationArchived = useChatStore(s => s.setConversationArchived);
  const selectedTagId = useUIStore(s => s.selectedTagId);

  const [deleteTarget, setDeleteTarget] = useState<ConversationWithTags | null>(null);
  const [isDeleting, setIsDeleting] = useState(false);

  const handleNewChat = async () => {
    try {
      // Scope the new conversation to whatever the user is already looking
      // at: the list's own tag filter, or the tag selected in the sidebar.
      // The chip is removable in the header's ScopeEditor, so this is a
      // suggestion, not a commitment.
      const scopeTagId = listFilterTagId ?? selectedTagId;
      await createConversation(scopeTagId ? [scopeTagId] : []);
    } catch (e) {
      console.error('Failed to create conversation:', e);
    }
  };

  const handleOpenConversation = (conversation: ConversationWithTags) => {
    openConversation(conversation.id);
  };

  const handleDeleteClick = (conversation: ConversationWithTags, e: React.MouseEvent) => {
    e.stopPropagation();
    setDeleteTarget(conversation);
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

  if (isLoading && conversations.length === 0) {
    return (
      <div className="flex items-center justify-center h-full text-[var(--color-text-secondary)]">
        Loading conversations...
      </div>
    );
  }

  if (error) {
    return (
      <div className="flex flex-col items-center justify-center h-full gap-4 p-4">
        <p className="text-red-400">{error}</p>
      </div>
    );
  }

  return (
    <div className="h-full flex flex-col">
      {/* New Chat Button */}
      <div className="flex-shrink-0 p-4 border-b border-[var(--color-border)]">
        <button
          onClick={handleNewChat}
          className="w-full flex items-center justify-center gap-2 px-4 py-2.5 bg-[var(--color-bg-hover)] hover:bg-[var(--color-border)] text-[var(--color-text-primary)] rounded-lg transition-colors"
        >
          <Plus className="w-5 h-5" strokeWidth={2} />
          New Conversation
        </button>
      </div>

      {/* Conversations List */}
      <div className="flex-1 overflow-y-auto">
        {conversations.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full gap-4 p-8 text-center">
            <div className="w-16 h-16 rounded-full bg-[var(--color-bg-card)] flex items-center justify-center">
              <MessageCircle className="w-8 h-8 text-[var(--color-text-secondary)]" strokeWidth={2} />
            </div>
            <div>
              <p className="text-[var(--color-text-primary)] font-medium mb-1">
                {showArchived ? 'Nothing here' : 'No conversations yet'}
              </p>
              <p className="text-[var(--color-text-secondary)] text-sm">
                {showArchived
                  ? 'No conversations, archived or otherwise'
                  : 'Start a new conversation to chat with your knowledge base'}
              </p>
            </div>
          </div>
        ) : (
          <div className="divide-y divide-[var(--color-border)]">
            {conversations.map((conversation) => (
              <ConversationCard
                key={conversation.id}
                conversation={conversation}
                onClick={() => handleOpenConversation(conversation)}
                onDelete={(e) => handleDeleteClick(conversation, e)}
                onRename={(title) => updateConversationTitle(conversation.id, title)}
                onArchive={(isArchived) => setConversationArchived(conversation.id, isArchived)}
              />
            ))}
          </div>
        )}
      </div>

      {/* Archive visibility */}
      <div className="flex-shrink-0 px-4 py-2 border-t border-[var(--color-border)]">
        <button
          onClick={() => setShowArchived(!showArchived)}
          className="text-xs text-[var(--color-text-tertiary)] hover:text-[var(--color-text-primary)] transition-colors"
        >
          {showArchived ? 'Hide archived' : 'Show archived'}
        </button>
      </div>

      {/* Delete Confirmation Modal */}
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
