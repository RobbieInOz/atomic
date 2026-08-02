import { useState } from 'react';
import { Archive, ArchiveRestore, ChevronLeft, MoreVertical } from 'lucide-react';
import { ConversationWithTags, useChatStore } from '../../stores/chat';
import { ContextMenu } from '../ui/ContextMenu';
import { ScopeEditor } from './ScopeEditor';

interface ChatHeaderProps {
  conversation: ConversationWithTags;
  onBack: () => void;
}

export function ChatHeader({ conversation, onBack }: ChatHeaderProps) {
  const [isEditingTitle, setIsEditingTitle] = useState(false);
  const [editedTitle, setEditedTitle] = useState(conversation.title || '');
  const [menuPosition, setMenuPosition] = useState<{ x: number; y: number } | null>(null);
  const updateConversationTitle = useChatStore(s => s.updateConversationTitle);
  const setConversationArchived = useChatStore(s => s.setConversationArchived);

  const handleTitleSave = async () => {
    if (editedTitle.trim() !== conversation.title) {
      await updateConversationTitle(conversation.id, editedTitle.trim() || 'Untitled');
    }
    setIsEditingTitle(false);
  };

  const handleTitleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') {
      handleTitleSave();
    } else if (e.key === 'Escape') {
      setEditedTitle(conversation.title || '');
      setIsEditingTitle(false);
    }
  };

  const handleArchive = async () => {
    await setConversationArchived(conversation.id, !conversation.is_archived);
    // Archiving hides the conversation from the default list; staying in it
    // would leave the user in a view they can't navigate back to.
    if (!conversation.is_archived) {
      onBack();
    }
  };

  const menuItems = [
    {
      label: conversation.is_archived ? 'Unarchive' : 'Archive',
      onClick: handleArchive,
      icon: conversation.is_archived ? (
        <ArchiveRestore className="w-4 h-4" strokeWidth={2} />
      ) : (
        <Archive className="w-4 h-4" strokeWidth={2} />
      ),
    },
  ];

  return (
    <div className="flex-shrink-0 border-b border-[var(--color-border)]">
      {/* Top row: Back button and title */}
      <div className="flex items-center gap-3 px-4 py-3">
        <button
          onClick={onBack}
          className="p-1.5 text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] rounded transition-colors"
          aria-label="Back to conversations"
        >
          <ChevronLeft className="w-5 h-5" strokeWidth={2} />
        </button>

        {isEditingTitle ? (
          <input
            type="text"
            value={editedTitle}
            onChange={(e) => setEditedTitle(e.target.value)}
            onBlur={handleTitleSave}
            onKeyDown={handleTitleKeyDown}
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
            spellCheck={false}
            className="flex-1 bg-[var(--color-bg-main)] border border-[var(--color-border)] rounded px-2 py-1 text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)]"
            autoFocus
          />
        ) : (
          <h2
            onClick={() => {
              setEditedTitle(conversation.title || '');
              setIsEditingTitle(true);
            }}
            className="flex-1 text-[var(--color-text-primary)] font-medium cursor-pointer hover:text-[var(--color-accent-light)] transition-colors truncate"
            title="Click to edit title"
          >
            {conversation.title || 'New Conversation'}
          </h2>
        )}

        <button
          onClick={(e) => {
            const rect = e.currentTarget.getBoundingClientRect();
            setMenuPosition({ x: rect.right - 160, y: rect.bottom + 4 });
          }}
          className="p-1.5 text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] rounded transition-colors"
          aria-label="Conversation actions"
        >
          <MoreVertical className="w-4 h-4" strokeWidth={2} />
        </button>
      </div>

      <ContextMenu
        items={menuItems}
        position={menuPosition}
        onClose={() => setMenuPosition(null)}
        autoFocus
      />

      {/* Scope editor row */}
      <div className="px-4 pb-3">
        <ScopeEditor conversation={conversation} />
      </div>
    </div>
  );
}
