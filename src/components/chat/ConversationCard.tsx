import { useState } from 'react';
import { Archive, ArchiveRestore, Pencil, Trash2 } from 'lucide-react';
import { ConversationWithTags, scopeModeOf } from '../../stores/chat';
import { formatRelativeDate } from '../../lib/date';

interface ConversationCardProps {
  conversation: ConversationWithTags;
  /// Marks the conversation the chat pane is currently showing. Only the
  /// sidebar list renders alongside an open conversation, so this is a no-op
  /// in the pane's own list.
  isActive?: boolean;
  onClick: () => void;
  onDelete: (e: React.MouseEvent) => void;
  onRename: (title: string) => void;
  onArchive: (isArchived: boolean) => void;
}

export function ConversationCard({
  conversation,
  isActive = false,
  onClick,
  onDelete,
  onRename,
  onArchive,
}: ConversationCardProps) {
  const title = conversation.title || 'New Conversation';
  const preview = conversation.last_message_preview || 'No messages yet';
  const messageCount = conversation.message_count;
  const updatedAt = formatRelativeDate(conversation.updated_at);

  const [isEditingTitle, setIsEditingTitle] = useState(false);
  const [editedTitle, setEditedTitle] = useState(conversation.title || '');

  // Enter and blur both commit; Escape restores. Same contract as the
  // header's title field, including the empty → 'Untitled' fallback.
  const handleTitleSave = () => {
    if (!isEditingTitle) return;
    setIsEditingTitle(false);
    const next = editedTitle.trim() || 'Untitled';
    if (next !== conversation.title) {
      onRename(next);
    }
  };

  const handleTitleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      handleTitleSave();
    } else if (e.key === 'Escape') {
      setEditedTitle(conversation.title || '');
      setIsEditingTitle(false);
    }
  };

  const startEditing = (e: React.MouseEvent) => {
    e.stopPropagation();
    setEditedTitle(conversation.title || '');
    setIsEditingTitle(true);
  };

  return (
    <div
      onClick={isEditingTitle ? undefined : onClick}
      className={`group relative px-4 py-3 cursor-pointer transition-colors ${
        isActive ? 'bg-[var(--color-accent)]/20' : 'hover:bg-[var(--color-bg-card)]'
      }`}
    >
      {/* Title. On touch the action overlay never hides, and the title row is
          what it covers — so that row, and only that row, gives up width. */}
      {isEditingTitle ? (
        <input
          type="text"
          value={editedTitle}
          onChange={(e) => setEditedTitle(e.target.value)}
          onClick={(e) => e.stopPropagation()}
          onBlur={handleTitleSave}
          onKeyDown={handleTitleKeyDown}
          autoComplete="off"
          autoCorrect="off"
          autoCapitalize="off"
          spellCheck={false}
          className="w-full mb-1 bg-[var(--color-bg-main)] border border-[var(--color-border)] rounded px-2 py-0.5 text-[var(--color-text-primary)] font-medium focus:outline-none focus:border-[var(--color-accent)]"
          autoFocus
        />
      ) : (
        <h3 className="flex items-center gap-2 text-[var(--color-text-primary)] font-medium truncate mb-1 max-md:pr-24">
          <span className="truncate">{title}</span>
          {conversation.is_archived && (
            <span className="flex-shrink-0 px-1.5 py-0.5 text-[10px] uppercase tracking-wide rounded bg-[var(--color-bg-hover)] text-[var(--color-text-tertiary)]">
              Archived
            </span>
          )}
        </h3>
      )}

      {/* Preview */}
      <p className="text-[var(--color-text-secondary)] text-sm line-clamp-2 mb-2">
        {preview}
      </p>

      {/* Meta info */}
      <div className="flex items-center gap-3 text-xs text-[var(--color-text-tertiary)]">
        <span>{updatedAt}</span>
        <span>•</span>
        <span>{messageCount} {messageCount === 1 ? 'message' : 'messages'}</span>
      </div>

      {/* Tags */}
      {conversation.tags.length > 0 && (
        <div className="flex flex-wrap gap-1 mt-2">
          {conversation.tags.slice(0, 3).map((tag) => (
            <span
              key={tag.id}
              className={`px-2 py-0.5 text-xs rounded ${
                scopeModeOf(tag) === 'exclude'
                  ? 'bg-red-500/10 text-red-300 line-through decoration-red-400/60'
                  : 'bg-[var(--color-accent)]/20 text-[var(--color-accent-light)]'
              }`}
            >
              {tag.name}
            </span>
          ))}
          {conversation.tags.length > 3 && (
            <span className="px-2 py-0.5 text-xs rounded bg-[var(--color-bg-hover)] text-[var(--color-text-secondary)]">
              +{conversation.tags.length - 3} more
            </span>
          )}
        </div>
      )}

      {/* Hover actions, overlaid rather than laid out beside the text: they
          are ~90px wide, which is a third of the fullscreen sidebar. Opaque so
          they stay legible over whatever they cover, always visible on touch
          (there is no hover there), and revealed by focus so they stay
          reachable by keyboard. Suppressed while renaming — the input they
          would sit on top of is the whole interaction. */}
      {!isEditingTitle && (
        <div
          onClick={(e) => e.stopPropagation()}
          className="absolute top-2 right-2 flex items-center gap-0.5 rounded-md border border-[var(--color-border)] bg-[var(--color-bg-elevated)] p-0.5 shadow-sm opacity-0 pointer-events-none transition-opacity group-hover:opacity-100 group-hover:pointer-events-auto focus-within:opacity-100 focus-within:pointer-events-auto max-md:opacity-100 max-md:pointer-events-auto"
        >
          <button
            onClick={startEditing}
            className="p-1.5 rounded text-[var(--color-text-tertiary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] transition-colors"
            aria-label="Rename conversation"
            title="Rename"
          >
            <Pencil className="w-4 h-4" strokeWidth={2} />
          </button>
          <button
            onClick={(e) => {
              e.stopPropagation();
              onArchive(!conversation.is_archived);
            }}
            className="p-1.5 rounded text-[var(--color-text-tertiary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] transition-colors"
            aria-label={conversation.is_archived ? 'Unarchive conversation' : 'Archive conversation'}
            title={conversation.is_archived ? 'Unarchive' : 'Archive'}
          >
            {conversation.is_archived ? (
              <ArchiveRestore className="w-4 h-4" strokeWidth={2} />
            ) : (
              <Archive className="w-4 h-4" strokeWidth={2} />
            )}
          </button>
          <button
            onClick={onDelete}
            className="p-1.5 rounded text-[var(--color-text-tertiary)] hover:text-red-400 hover:bg-[var(--color-bg-hover)] transition-colors"
            aria-label="Delete conversation"
            title="Delete"
          >
            <Trash2 className="w-4 h-4" strokeWidth={2} />
          </button>
        </div>
      )}
    </div>
  );
}
